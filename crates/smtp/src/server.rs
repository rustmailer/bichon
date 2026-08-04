//
// Copyright (c) 2025-2026 rustmailer.com (https://rustmailer.com)
//
// This file is part of the Bichon Email Archiving Project
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <http://www.gnu.org/licenses/>.

use std::io;
use std::net::SocketAddr;
use std::time::Duration;

use base64::{prelude::BASE64_STANDARD, Engine as _};
use bichon_core::account::migration::AccountType;
use bichon_core::cache::imap::mailbox::{Attribute, AttributeEnum};
use bichon_core::common::signal::SIGNAL_MANAGER;
use bichon_core::envelope::extractor::extract_envelope_from_smtp;
use bichon_core::error::BichonResult;
use bichon_core::settings::cli::{EncryptionMode, SETTINGS};
use bichon_core::utils::create_hash;
use bichon_core::{
    account::migration::AccountModel,
    cache::imap::mailbox::MailBox,
    common::auth::ClientContext,
    token::AccessTokenModel,
    users::{permissions::Permission, UserModel},
};
use tokio::time::timeout;
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::broadcast,
};
use tokio_rustls::TlsAcceptor;

use crate::stream::BufStream;
use crate::tls::create_acceptor;

const MAX_MAIL_SIZE: usize = 50 * 1024 * 1024; //50MB
const SMTP_IDLE_TIMEOUT: Duration = Duration::from_secs(60);
const GLOBAL_SESSION_TIMEOUT: Duration = Duration::from_secs(600);

pub async fn run_smtp_server(
    listener: TcpListener,
    config: SmtpConfig,
    mut shutdown: broadcast::Receiver<()>,
) {
    loop {
        tokio::select! {
            result = listener.accept() => {
                match result {
                    Ok((stream, addr)) => {
                        tracing::debug!("SMTP connection from {addr}");
                        let config = config.clone();
                        tokio::spawn(async move {
                            let res = timeout(GLOBAL_SESSION_TIMEOUT, handle_connection(stream, config)).await;
                            match res {
                                Ok(Ok(_)) => tracing::debug!("SMTP session from {addr} finished"),
                                Ok(Err(e)) => tracing::debug!("SMTP session error from {addr}: {e}"),
                                Err(_) => tracing::warn!("SMTP session from {addr} timed out after {}s", GLOBAL_SESSION_TIMEOUT.as_secs()),
                            }
                        });
                    }
                    Err(e) => {
                        tracing::error!("Failed to accept connection: {e}");
                    }
                }
            }
            _ = shutdown.recv() => {
                break;
            }
        }
    }
}

pub async fn run_smtps_server(
    listener: TcpListener,
    config: SmtpConfig,
    tls_acceptor: TlsAcceptor,
    mut shutdown: broadcast::Receiver<()>,
) {
    loop {
        tokio::select! {
            result = listener.accept() => {
                match result {
                    Ok((stream, addr)) => {
                        tracing::debug!("SMTPS connection from {addr}");
                        let config = config.clone();
                        let acceptor = tls_acceptor.clone();
                        tokio::spawn(async move {
                            match acceptor.accept(stream).await {
                                Ok(tls_stream) => {
                                    let res = timeout(GLOBAL_SESSION_TIMEOUT, handle_tls_connection(tls_stream, config)).await;
                                    match res {
                                        Ok(Ok(_)) => tracing::debug!("SMTPS session from {addr} finished"),
                                        Ok(Err(e)) => tracing::debug!("SMTPS session error from {addr}: {e}"),
                                        Err(_) => tracing::warn!("SMTPS session from {addr} timed out after {}s", GLOBAL_SESSION_TIMEOUT.as_secs()),
                                    }
                                }
                                Err(e) => {
                                    tracing::debug!("TLS handshake failed: {e}");
                                }
                            }
                        });
                    }
                    Err(e) => {
                        tracing::error!("Failed to accept connection: {e}");
                    }
                }
            }
            _ = shutdown.recv() => {
                break;
            }
        }
    }
}

enum CommandResult {
    Continue,
    Quit,
    StartTls,
}

struct Session {
    mail_from: Option<String>,
    rcpt_to: Vec<AccountModel>,
    authenticated: bool,
    user: Option<UserModel>,
    auth_required: bool,
    tls_active: bool,
    auth_state: AuthState,
}

impl Session {
    const fn new(auth_required: bool, tls_active: bool) -> Self {
        Self {
            mail_from: None,
            rcpt_to: Vec::new(),
            authenticated: false,
            user: None,
            auth_required,
            tls_active,
            auth_state: AuthState::None,
        }
    }

    fn reset(&mut self) {
        self.mail_from = None;
        self.rcpt_to.clear();
    }
}

#[derive(Default)]
enum AuthState {
    #[default]
    None,
    WaitingForPlain,
    WaitingForLoginUsername,
    WaitingForLoginPassword(String),
}

/// Handle a plain TCP connection with optional STARTTLS upgrade.
async fn handle_connection(stream: TcpStream, config: SmtpConfig) -> io::Result<()> {
    let mut session = Session::new(config.auth_required, false);

    // Use buffered I/O over the raw stream
    let mut stream = BufStream::new(stream);

    stream
        .write_all(b"220 localhost ESMTP (Bichon Email Archiver)\r\n")
        .await?;
    stream.flush().await?;

    loop {
        match process_command(&mut stream, &mut session, &config).await? {
            CommandResult::Continue => {}
            CommandResult::Quit => break,
            CommandResult::StartTls => {
                if let Some(ref acceptor) = config.tls_acceptor {
                    tracing::debug!("Upgrading connection to TLS");
                    let inner = stream.into_inner();
                    match acceptor.clone().accept(inner).await {
                        Ok(tls_stream) => {
                            session.tls_active = true;
                            session.reset();
                            return handle_tls_session(tls_stream, session, config).await;
                        }
                        Err(e) => {
                            tracing::debug!("STARTTLS handshake failed: {e}");
                            return Err(io::Error::other(format!("TLS handshake failed: {e}")));
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

async fn handle_tls_connection<S>(stream: S, config: SmtpConfig) -> io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let session = Session::new(config.auth_required, true);
    handle_tls_session(stream, session, config).await
}

async fn handle_tls_session<S>(
    stream: S,
    mut session: Session,
    config: SmtpConfig,
) -> io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut stream = BufStream::new(stream);
    loop {
        match process_command(&mut stream, &mut session, &config).await? {
            CommandResult::Continue => {}
            CommandResult::Quit => break,
            CommandResult::StartTls => {
                stream.write_all(b"503 TLS already active\r\n").await?;
                stream.flush().await?;
            }
        }
    }

    Ok(())
}

#[allow(clippy::too_many_lines)]
async fn process_command<S>(
    stream: &mut BufStream<S>,
    session: &mut Session,
    config: &SmtpConfig,
) -> io::Result<CommandResult>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut line = String::new();
    let bytes_read = match timeout(SMTP_IDLE_TIMEOUT, stream.inner.read_line(&mut line)).await {
        Ok(res) => res?,
        Err(_) => return Err(io::Error::new(io::ErrorKind::TimedOut, "Command timeout")),
    };

    if bytes_read == 0 {
        return Ok(CommandResult::Quit);
    }

    let trimmed = line.trim();
    let cmd = trimmed.to_uppercase();

    match &session.auth_state {
        AuthState::WaitingForPlain => {
            verify_plain_auth(trimmed, session, stream).await?;
            session.auth_state = AuthState::None;
            return Ok(CommandResult::Continue);
        }
        AuthState::WaitingForLoginUsername => {
            if let Ok(decoded) = BASE64_STANDARD.decode(trimmed) {
                let username = String::from_utf8_lossy(&decoded).to_string();
                stream.write_all(b"334 UGFzc3dvcmQ6\r\n").await?;
                stream.flush().await?;
                session.auth_state = AuthState::WaitingForLoginPassword(username);
            } else {
                stream.write_all(b"501 Cannot decode\r\n").await?;
                stream.flush().await?;
                session.auth_state = AuthState::None;
            }
            return Ok(CommandResult::Continue);
        }
        AuthState::WaitingForLoginPassword(username) => {
            let username = username.clone();
            if let Ok(decoded) = BASE64_STANDARD.decode(trimmed) {
                let password = String::from_utf8_lossy(&decoded);
                match AccessTokenModel::resolve_user_from_token(&password) {
                    Ok(user) => {
                        session.authenticated = true;
                        session.user = Some(user);
                        stream
                            .write_all(b"235 Authentication successful\r\n")
                            .await?;
                    }
                    Err(error) => {
                        tracing::error!(
                            "SMTP Auth failed for user '{}' (AUTH LOGIN): {:?}",
                            username,
                            error
                        );
                        stream.write_all(b"535 Authentication failed\r\n").await?;
                    }
                }
            } else {
                stream.write_all(b"501 Cannot decode\r\n").await?;
            }
            stream.flush().await?;
            session.auth_state = AuthState::None;
            return Ok(CommandResult::Continue);
        }
        AuthState::None => {}
    }

    if cmd.starts_with("EHLO") || cmd.starts_with("HELO") {
        let mut response = String::from("250-Bichon Hello\r\n");
        response.push_str("250-SIZE 52428800\r\n"); // 50MB
        response.push_str("250-8BITMIME\r\n");

        if config.tls_acceptor.is_some() && !session.tls_active {
            response.push_str("250-STARTTLS\r\n");
        }

        response.push_str("250-AUTH PLAIN LOGIN\r\n");
        response.push_str("250 OK\r\n");

        stream.write_all(response.as_bytes()).await?;
        stream.flush().await?;
    } else if cmd.starts_with("STARTTLS") {
        if config.tls_acceptor.is_none() {
            stream.write_all(b"454 TLS not available\r\n").await?;
        } else if session.tls_active {
            stream.write_all(b"503 TLS already active\r\n").await?;
        } else {
            stream.write_all(b"220 Ready to start TLS\r\n").await?;
            stream.flush().await?;
            return Ok(CommandResult::StartTls);
        }
    } else if cmd.starts_with("AUTH ") {
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.len() >= 2 {
            let mechanism = parts[1].to_uppercase();
            match mechanism.as_str() {
                "PLAIN" => {
                    if parts.len() > 2 {
                        verify_plain_auth(parts[2], session, stream).await?;
                    } else {
                        stream.write_all(b"334 \r\n").await?;
                        session.auth_state = AuthState::WaitingForPlain;
                    }
                }
                "LOGIN" => {
                    stream.write_all(b"334 VXNlcm5hbWU6\r\n").await?;
                    session.auth_state = AuthState::WaitingForLoginUsername;
                }
                _ => {
                    stream.write_all(b"504 Unrecognized auth type\r\n").await?;
                }
            }
        } else {
            stream.write_all(b"501 Syntax error\r\n").await?;
        }
    } else if cmd.starts_with("MAIL FROM:") {
        if session.auth_required && !session.authenticated {
            stream.write_all(b"530 Authentication required\r\n").await?;
        } else {
            let addr = extract_address(&trimmed[10..]);
            let mut allowed = true;
            if let Some(ref whitelist) = config.whitelist {
                if !whitelist.is_empty() && !whitelist.contains(&addr) {
                    allowed = false;
                }
            }

            if allowed {
                session.mail_from = Some(addr);
                stream.write_all(b"250 OK\r\n").await?;
            } else {
                stream.write_all(b"550 Sender not allowed\r\n").await?;
            }
        }
    } else if cmd.starts_with("RCPT TO:") {
        if !session.rcpt_to.is_empty() {
            stream
                .write_all(b"452 4.5.3 Too many recipients, try again in a new transaction\r\n")
                .await?;
            return Ok(CommandResult::Continue);
        }

        if session.auth_required && !session.authenticated {
            stream.write_all(b"530 Authentication required\r\n").await?;
        } else if session.mail_from.is_none() {
            stream
                .write_all(b"503 MAIL FROM required first\r\n")
                .await?;
        } else {
            let addr = extract_address(&trimmed[8..]);
            //println!("DEBUG: SMTP RCPT TO extracted address -> '{}'", addr);
            let account_result = AccountModel::find_by_email(addr.as_str());

            match account_result {
                Ok(Some(account)) => {
                    let mut is_allowed = true;
                    if session.auth_required {
                        if let Some(user) = &session.user {
                            let has_perm = ClientContext::check_has_permission(
                                user,
                                Some(account.id),
                                Permission::DATA_SMTP_INGEST,
                            );

                            if !has_perm {
                                tracing::warn!(
                                    "SMTP: Access denied for User {} to Account <{}>",
                                    user.id,
                                    addr
                                );
                                stream
                                    .write_all(
                                        b"554 5.7.1 Access denied: Insufficient permissions\r\n",
                                    )
                                    .await?;
                                is_allowed = false;
                            }
                        } else {
                            stream
                                .write_all(b"530 5.7.0 Authentication required\r\n")
                                .await?;
                            is_allowed = false;
                        }
                    }

                    if is_allowed {
                        if !matches!(account.account_type, AccountType::NoSync) {
                            tracing::warn!(
                                "SMTP: Rejected journaling attempt to IMAP account <{}>",
                                addr
                            );
                            let err = format!(
                                "550 5.7.1 <{}>: Not a Bichon local account, journaling is not supported\r\n",
                                account.email
                            );
                            stream.write_all(err.as_bytes()).await?;
                        } else {
                            session.rcpt_to.push(account);
                            stream.write_all(b"250 OK\r\n").await?;
                        }
                    }
                }
                Ok(None) => {
                    let err = format!("550 5.1.1 <{}>: Bichon account not found\r\n", addr);
                    stream.write_all(err.as_bytes()).await?;
                }
                Err(e) => {
                    tracing::error!("SMTP: Account query error for {}: {:?}", addr, e);
                    stream
                        .write_all(
                            b"451 4.3.0 Requested action aborted: local error in processing\r\n",
                        )
                        .await?;
                }
            }
        }
    } else if cmd == "DATA" {
        if session.auth_required && !session.authenticated {
            stream.write_all(b"530 Authentication required\r\n").await?;
        } else if session.mail_from.is_none() {
            stream
                .write_all(b"503 MAIL FROM required first\r\n")
                .await?;
        } else if session.rcpt_to.is_empty() {
            stream.write_all(b"503 RCPT TO required first\r\n").await?;
        } else {
            stream
                .write_all(b"354 End data with <CR><LF>.<CR><LF>\r\n")
                .await?;
            stream.flush().await?;

            let data = match read_data(&mut stream.inner).await {
                Ok(d) => d,
                Err(e) => {
                    if e.to_string().contains("552") {
                        let error_msg = format!(
                            "552 5.3.4 Message size exceeds limit of {} bytes ({}MB)\r\n",
                            MAX_MAIL_SIZE,
                            MAX_MAIL_SIZE / 1024 / 1024
                        );
                        stream.write_all(error_msg.as_bytes()).await?;
                        stream.flush().await?;
                        return Ok(CommandResult::Continue);
                    }
                    return Err(e);
                }
            };
            match parse_email(&data, session).await {
                Ok(_) => {
                    stream
                        .write_all(b"250 2.0.0 OK: queued in Bichon\r\n")
                        .await?;
                    tracing::debug!(
                        "SMTP: Message accepted and archived for {} recipients",
                        session.rcpt_to.len()
                    );
                    session.reset();
                }
                Err(e) => {
                    tracing::error!("SMTP: Critical error during parse_email: {:?}", e);
                    stream
                        .write_all(
                            b"451 4.3.0 Error: local error in processing, try again later\r\n",
                        )
                        .await?;
                }
            }
        }
    } else if cmd == "RSET" {
        session.reset();
        stream.write_all(b"250 OK\r\n").await?;
    } else if cmd == "NOOP" {
        stream.write_all(b"250 OK\r\n").await?;
    } else if cmd == "QUIT" {
        stream.write_all(b"221 Bye\r\n").await?;
        stream.flush().await?;
        return Ok(CommandResult::Quit);
    } else {
        stream.write_all(b"500 Command not recognized\r\n").await?;
    }

    stream.flush().await?;
    Ok(CommandResult::Continue)
}

async fn verify_plain_auth<S: AsyncRead + AsyncWrite + Unpin>(
    encoded: &str,
    session: &mut Session,
    stream: &mut BufStream<S>,
) -> io::Result<()> {
    if let Ok(decoded) = BASE64_STANDARD.decode(encoded.trim()) {
        let parts: Vec<&[u8]> = decoded.split(|&b| b == 0).collect();
        if parts.len() >= 3 {
            let username = String::from_utf8_lossy(parts[1]);
            let password = String::from_utf8_lossy(parts[2]);

            match AccessTokenModel::resolve_user_from_token(&password) {
                Ok(user) => {
                    session.authenticated = true;
                    session.user = Some(user);
                    stream
                        .write_all(b"235 Authentication successful\r\n")
                        .await?;
                    stream.flush().await?;
                    return Ok(());
                }
                Err(error) => {
                    tracing::error!("SMTP Auth failed for user '{}': {:?}", username, error);
                }
            }
        }
    }

    stream.write_all(b"535 Authentication failed\r\n").await?;
    stream.flush().await?;
    Ok(())
}

fn extract_address(s: &str) -> String {
    let s = s.trim();
    if let (Some(start), Some(end)) = (s.find('<'), s.find('>')) {
        return s[start + 1..end].to_string();
    }
    s.to_string()
}

async fn read_data<R: AsyncBufReadExt + Unpin>(reader: &mut R) -> io::Result<Vec<u8>> {
    let mut data = Vec::with_capacity(65536);
    let mut line = String::new();
    let mut total_bytes = 0;
    let line_timeout = Duration::from_secs(30);

    loop {
        line.clear();
        let bytes_read = match timeout(line_timeout, reader.read_line(&mut line)).await {
            Ok(res) => res?,
            Err(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Data transmission timeout",
                ))
            }
        };

        if bytes_read == 0 {
            break;
        }
        total_bytes += bytes_read;

        if total_bytes > MAX_MAIL_SIZE {
            tracing::warn!(
                "SMTP: Message rejected. Size {} bytes exceeds limit of {}MB",
                total_bytes,
                MAX_MAIL_SIZE / 1024 / 1024
            );
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "552 5.3.4 Message size exceeds fixed maximum message size",
            ));
        }
        if line.trim() == "." {
            break;
        }
        let content = if line.starts_with("..") {
            &line[1..]
        } else {
            &line
        };
        data.extend_from_slice(content.as_bytes());
    }

    Ok(data)
}

async fn parse_email(data: &[u8], session: &Session) -> BichonResult<()> {
    let rcpt = match session.rcpt_to.first() {
        Some(r) => r,
        None => {
            tracing::warn!(
                "SMTP: parse_email called with empty recipient list. Skipping processing."
            );
            return Ok(());
        }
    };
    let mailbox_id = create_hash(rcpt.id, "INBOX");

    // The INBOX row is owned by the IMAP sync, which maintains `uid_validity`,
    // `highest_uid` and `uid_next` on it. `batch_upsert` replaces the *whole*
    // row, so blindly upserting here (with those fields = None) clobbers the
    // IMAP-maintained state back to None. The next reconcile then sees
    // `uid_validity` change from Some -> None, treats the mailbox as invalid,
    // and wipes + rebuilds it — silently losing the local copy of a large
    // mailbox when that rebuild is interrupted (see #297).
    //
    // We only need the row to *exist* so the journaled envelope can attach to
    // it, so create it only when it is missing and otherwise leave the
    // IMAP-owned row untouched.
    if MailBox::find_mailbox(rcpt.id, mailbox_id)?.is_none() {
        let mailbox = MailBox {
            id: mailbox_id,
            account_id: rcpt.id,
            name: "INBOX".into(),
            delimiter: Some("/".to_string()),
            attributes: vec![Attribute {
                attr: AttributeEnum::Extension,
                extension: Some("CreatedByBichon".into()),
            }],
            exists: 0,
            unseen: None,
            uid_next: None,
            uid_validity: None,
            highest_uid: None,
            uidonly_source_scope: None,
        };

        if let Err(e) = MailBox::batch_upsert(&[mailbox]) {
            tracing::error!("SMTP: Failed to upsert mailbox for {}: {:?}", rcpt.email, e);
            return Err(e.into());
        }
    }

    extract_envelope_from_smtp(data, rcpt.id, mailbox_id)
        .await
        .map_err(|e| {
            tracing::error!(
                "SMTP: Envelope extraction failed for {}: {:?}",
                rcpt.email,
                e
            );
            e
        })
}

#[derive(Clone, Default)]
pub struct SmtpConfig {
    pub whitelist: Option<Vec<String>>,
    pub tls_acceptor: Option<TlsAcceptor>,
    pub auth_required: bool,
}

pub struct SmtpServer {
    pub smtp_addr: SocketAddr,
    smtp_handle: tokio::task::JoinHandle<()>,
}

impl SmtpServer {
    pub async fn stop(self) {
        let _ = self.smtp_handle.await;
    }
}

pub async fn start_smtp_server() -> std::io::Result<SmtpServer> {
    let smtp_port = SETTINGS.bichon_smtp_port;

    let tls_acceptor: Option<TlsAcceptor> = match SETTINGS.bichon_smtp_encryption {
        EncryptionMode::None => None,
        EncryptionMode::Starttls | EncryptionMode::Tls => Some(create_acceptor().await?),
    };

    let smtp_listener = TcpListener::bind((
        SETTINGS.bichon_bind_ip.clone().unwrap_or("0.0.0.0".into()),
        smtp_port,
    ))
    .await
    .map_err(|e| {
        if e.kind() == std::io::ErrorKind::AddrInUse {
            std::io::Error::other(format!(
                "SMTP port {smtp_port} is already in use. Is another instance running?"
            ))
        } else {
            e
        }
    })?;
    let smtp_addr = smtp_listener.local_addr()?;

    let smtp_config = SmtpConfig {
        whitelist: None,
        tls_acceptor: match SETTINGS.bichon_smtp_encryption {
            EncryptionMode::None | EncryptionMode::Tls => None,
            EncryptionMode::Starttls => tls_acceptor.clone(),
        },
        auth_required: SETTINGS.bichon_smtp_auth_required,
    };

    let smtp_shutdown = SIGNAL_MANAGER.subscribe();

    let smtp_handle = if matches!(SETTINGS.bichon_smtp_encryption, EncryptionMode::Tls) {
        let acceptor = tls_acceptor
            .clone()
            .expect("TLS acceptor required when tls=true");
        tokio::spawn(async move {
            run_smtps_server(smtp_listener, smtp_config, acceptor, smtp_shutdown).await;
        })
    } else {
        tokio::spawn(async move {
            run_smtp_server(smtp_listener, smtp_config, smtp_shutdown).await;
        })
    };

    Ok(SmtpServer {
        smtp_addr,
        smtp_handle,
    })
}
