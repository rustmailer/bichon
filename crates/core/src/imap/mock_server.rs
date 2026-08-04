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

//! A minimal scriptable IMAP server for integration testing.
//!
//! Each instance listens on a random localhost port and responds to a
//! pre-configured script of (expected_command, response) pairs. Commands
//! are matched by substring — the first matching pattern wins.
//!
//! # Example
//! ```ignore
//! let server = MockImapServer::new()
//!     .greeting("* OK ready\r\n")
//!     .respond("LOGIN", "A0 OK logged in\r\n")
//!     .respond("CAPABILITY", "* CAPABILITY IMAP4rev1\r\nA0 OK done\r\n")
//!     .respond("STATUS", "* STATUS INBOX (MESSAGES 10 UIDVALIDITY 42)\r\nA0 OK\r\n")
//!     .respond("LOGOUT", "* BYE\r\nA0 OK\r\n")
//!     .start()
//!     .await;
//!
//! let (host, port) = server.addr();
//! // connect to host:port with Encryption::None
//! ```

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

type Response = Vec<u8>;

pub struct MockImapServer {
    greeting: Vec<u8>,
    script: Vec<(String, Response)>,
    commands: Arc<Mutex<Vec<String>>>,
}

impl MockImapServer {
    pub fn new() -> Self {
        Self {
            greeting: b"* OK Mock IMAP server ready\r\n".to_vec(),
            script: Vec::new(),
            commands: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Set the greeting banner sent immediately after connection.
    pub fn greeting(mut self, banner: impl Into<Vec<u8>>) -> Self {
        self.greeting = banner.into();
        self
    }

    /// Add a script step: when a client command *contains* `pattern` (case-insensitive),
    /// respond with `response`. Steps are checked in insertion order.
    pub fn respond(mut self, pattern: impl Into<String>, response: impl Into<Vec<u8>>) -> Self {
        self.script.push((pattern.into(), response.into()));
        self
    }

    /// Start the server on a random port. Returns a handle whose `addr()` gives
    /// the `(host, port)` to connect to.
    pub async fn start(self) -> MockImapServerHandle {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local_addr");

        let server = Arc::new(self);
        let commands = Arc::clone(&server.commands);

        tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, _)) => {
                        let srv = server.clone();
                        tokio::spawn(async move {
                            srv.handle_connection(stream).await;
                        });
                    }
                    Err(_) => break,
                }
            }
        });

        MockImapServerHandle { addr, commands }
    }

    async fn handle_connection(&self, mut stream: TcpStream) {
        let (reader, mut writer) = stream.split();
        let mut reader = BufReader::new(reader);

        // Send greeting
        if writer.write_all(&self.greeting).await.is_err() {
            return;
        }

        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line).await {
                Ok(0) => break, // EOF
                Ok(_) => {}
                Err(_) => break,
            }
            self.commands
                .lock()
                .expect("commands poisoned")
                .push(line.trim_end_matches(['\r', '\n']).to_string());

            let tag = extract_tag(&line).unwrap_or("A0");
            let matched = self.find_match(&line);
            if let Some(response) = matched {
                let substituted = substitute_tag(response, tag);
                if writer.write_all(&substituted).await.is_err() {
                    break;
                }
            } else {
                // Default: send tagged OK for commands we don't handle
                let fallback = format!("{tag} OK done\r\n");
                if writer.write_all(fallback.as_bytes()).await.is_err() {
                    break;
                }
            }
        }
    }

    fn find_match(&self, line: &str) -> Option<&[u8]> {
        let line_lower = line.to_lowercase();
        for (pattern, response) in &self.script {
            if line_lower.contains(&pattern.to_lowercase()) {
                return Some(response);
            }
        }
        None
    }
}

impl Default for MockImapServer {
    fn default() -> Self {
        Self::new()
    }
}

/// Handle to a running mock IMAP server. The server stops when this handle
/// is dropped.
pub struct MockImapServerHandle {
    addr: SocketAddr,
    commands: Arc<Mutex<Vec<String>>>,
}

impl MockImapServerHandle {
    pub fn host(&self) -> String {
        self.addr.ip().to_string()
    }

    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    pub fn commands(&self) -> Vec<String> {
        self.commands.lock().expect("commands poisoned").clone()
    }
}

fn extract_tag(line: &str) -> Option<&str> {
    line.split_whitespace().next()
}

/// Replace `{TAG}` placeholders in `response` with `tag`.
fn substitute_tag(response: &[u8], tag: &str) -> Vec<u8> {
    let placeholder = b"{TAG}";
    if response.is_empty() || !contains_slice(response, placeholder) {
        return response.to_vec();
    }
    let tag_bytes = tag.as_bytes();
    let mut result = Vec::with_capacity(response.len());
    let mut pos = 0;
    while let Some(idx) = find_slice(&response[pos..], placeholder) {
        result.extend_from_slice(&response[pos..pos + idx]);
        result.extend_from_slice(tag_bytes);
        pos += idx + placeholder.len();
    }
    result.extend_from_slice(&response[pos..]);
    result
}

fn contains_slice(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

fn find_slice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|w| w == needle)
}

// ============================================================
// Pre-built response helpers
// ============================================================

/// Build a tagged OK response.
pub fn ok(tag: impl AsRef<str>, msg: impl AsRef<str>) -> Vec<u8> {
    format!("{} OK {}\r\n", tag.as_ref(), msg.as_ref()).into_bytes()
}

/// Build a STATUS response line.
pub fn status_response(
    mailbox: &str,
    messages: u32,
    unseen: u32,
    uid_next: u32,
    uid_validity: Option<u32>,
) -> Vec<u8> {
    let uv = uid_validity
        .map(|v| format!(" UIDVALIDITY {v}"))
        .unwrap_or_default();
    let text = format!(
        "* STATUS \"{mailbox}\" (MESSAGES {messages} UNSEEN {unseen} UIDNEXT {uid_next}{uv})\r\n"
    );
    // Clients expect a tagged response after the untagged STATUS line.
    // We produce a generic OK that works for any tag.
    let mut out = text.into_bytes();
    out.extend_from_slice(b"{TAG} OK STATUS completed\r\n");
    out
}

/// Build an EXAMINE response with mailbox data.
pub fn examine_response(
    _mailbox: &str,
    exists: u32,
    uid_validity: u32,
    uid_next: u32,
) -> Vec<u8> {
    format!(
        "* FLAGS (\\Seen \\Answered \\Flagged \\Deleted \\Draft)\r\n\
         * OK [PERMANENTFLAGS ()]\r\n\
         * {exists} EXISTS\r\n\
         * 0 RECENT\r\n\
         * OK [UIDVALIDITY {uid_validity}]\r\n\
         * OK [UIDNEXT {uid_next}]\r\n\
         * OK [HIGHESTMODSEQ 1]\r\n\
         {{TAG}} OK [READ-ONLY] EXAMINE completed\r\n"
    )
    .into_bytes()
}

/// Build a UID SEARCH response for the given UID list.
pub fn uid_search_response(uids: &[u32]) -> Vec<u8> {
    let uid_str = uids
        .iter()
        .map(|u| u.to_string())
        .collect::<Vec<_>>()
        .join(" ");
    format!("* SEARCH {uid_str}\r\n{{TAG}} OK SEARCH completed\r\n").into_bytes()
}

/// Build a UID FETCH response returning full headers (for BODY[HEADER]).
/// Each entry: (uid, message_id)
pub fn uid_fetch_metadata_response(entries: &[(u32, &str)]) -> Vec<u8> {
    let mut out = Vec::new();
    for (uid, msg_id) in entries {
        // Build a minimal header that contains the Message-ID line.
        let header_data = format!(
            "From: sender@example.com\r\n\
To: recipient@example.com\r\n\
Date: Thu, 01 Jan 2025 00:00:00 +0000\r\n\
Subject: test\r\n\
Message-ID: {msg_id}\r\n\r\n"
        );
        let header_len = header_data.len();
        let line = format!(
            "* {uid} FETCH (UID {uid} BODY[HEADER] {{{header_len}}}\r\n\
{header_data}\
)\r\n",
        );
        out.extend_from_slice(line.as_bytes());
    }
    out.extend_from_slice(b"{TAG} OK FETCH completed\r\n");
    out
}

/// Build a UID FETCH RFC822 response with a full email body.
pub fn uid_fetch_rfc822_response(uid: u32, eml: &[u8]) -> Vec<u8> {
    let header = format!(
        "* {uid} FETCH (UID {uid} RFC822 {{{len}}}\r\n",
        len = eml.len()
    );
    let mut out = header.into_bytes();
    out.extend_from_slice(eml);
    out.extend_from_slice(b")\r\n{TAG} OK FETCH completed\r\n");
    out
}

/// A minimal RFC822 email fixture for testing.
pub fn minimal_eml(subject: &str, message_id: &str) -> Vec<u8> {
    format!(
        "From: sender@example.com\r\n\
         To: recipient@example.com\r\n\
         Subject: {subject}\r\n\
         Message-ID: <{message_id}>\r\n\
         Date: Thu, 01 Jan 2025 00:00:00 +0000\r\n\
         MIME-Version: 1.0\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\
         \r\n\
         This is a test email: {subject}.\r\n"
    )
    .into_bytes()
}

// ============================================================
// Self-tests for the mock server itself
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    async fn connect_and_read_greeting(host: &str, port: u16) -> String {
        let mut stream = TcpStream::connect((host, port)).await.unwrap();
        let (reader, _writer) = stream.split();
        let mut reader = BufReader::new(reader);
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        line
    }

    async fn send_and_recv(host: &str, port: u16, cmd: &str) -> String {
        let mut stream = TcpStream::connect((host, port)).await.unwrap();
        let (reader, mut writer) = stream.split();
        let mut reader = BufReader::new(reader);

        // Read greeting
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();

        // Send command
        writer.write_all(cmd.as_bytes()).await.unwrap();
        writer.write_all(b"\r\n").await.unwrap();

        // Read response (may be multi-line; read until tagged response)
        let mut out = String::new();
        loop {
            line.clear();
            reader.read_line(&mut line).await.unwrap();
            out.push_str(&line);
            if line.starts_with("A0") || line.starts_with("A1") {
                break;
            }
        }
        out
    }

    #[tokio::test]
    async fn test_mock_greeting() {
        let handle = MockImapServer::new().start().await;
        let greeting = connect_and_read_greeting(&handle.host(), handle.port()).await;
        assert!(greeting.starts_with("* OK"));
    }

    #[tokio::test]
    async fn test_mock_scripted_response() {
        let handle = MockImapServer::new()
            .respond(
                "LOGIN",
                "A0 OK LOGIN completed\r\n",
            )
            .start()
            .await;

        let resp = send_and_recv(&handle.host(), handle.port(), "A0 LOGIN u p").await;
        assert!(resp.contains("LOGIN completed"));
    }

    #[tokio::test]
    async fn test_mock_fallback_on_unmatched() {
        let handle = MockImapServer::new().start().await;

        // Send a command that has no scripted response
        let resp = send_and_recv(&handle.host(), handle.port(), "A0 NOOP").await;
        assert!(resp.contains("OK done"), "unmatched command should get fallback OK");
    }

    #[tokio::test]
    async fn test_status_response_helper() {
        let resp = status_response("INBOX", 10, 2, 11, Some(42));
        let text = String::from_utf8(resp).unwrap();
        assert!(text.contains("MESSAGES 10"));
        assert!(text.contains("UNSEEN 2"));
        assert!(text.contains("UIDNEXT 11"));
        assert!(text.contains("UIDVALIDITY 42"));
    }

    #[tokio::test]
    async fn test_status_response_without_uidvalidity() {
        let resp = status_response("INBOX", 10, 2, 11, None);
        let text = String::from_utf8(resp).unwrap();
        assert!(!text.contains("UIDVALIDITY"));
        assert!(text.contains("MESSAGES 10"));
    }

    #[tokio::test]
    async fn test_examine_response() {
        let resp = examine_response("INBOX", 10, 42, 11);
        let text = String::from_utf8(resp).unwrap();
        assert!(text.contains("UIDVALIDITY 42"));
        assert!(text.contains("10 EXISTS"));
    }

    #[tokio::test]
    async fn test_uid_search_response() {
        let resp = uid_search_response(&[1, 3, 5]);
        let text = String::from_utf8(resp).unwrap();
        assert!(text.contains("SEARCH 1 3 5"));
    }

    #[tokio::test]
    async fn test_uid_fetch_metadata_response() {
        let resp = uid_fetch_metadata_response(&[(1, "msg-a@x.com"), (2, "msg-b@x.com")]);
        let text = String::from_utf8(resp).unwrap();
        assert!(text.contains("Message-ID: msg-a@x.com"));
        assert!(text.contains("Message-ID: msg-b@x.com"));
    }

    #[tokio::test]
    async fn test_multiple_commands_in_sequence() {
        let handle = MockImapServer::new()
            .respond("LOGIN", "A0 OK LOGIN\r\n")
            .respond("STATUS", status_response("INBOX", 5, 1, 6, Some(99)))
            .respond("LOGOUT", "* BYE\r\nA0 OK\r\n")
            .start()
            .await;

        let mut stream = TcpStream::connect((handle.host(), handle.port()))
            .await
            .unwrap();
        let (reader, mut writer) = stream.split();
        let mut reader = BufReader::new(reader);

        // Read greeting
        let mut buf = String::new();
        reader.read_line(&mut buf).await.unwrap();

        // LOGIN
        writer.write_all(b"A0 LOGIN u p\r\n").await.unwrap();
        buf.clear();
        reader.read_line(&mut buf).await.unwrap();
        assert!(buf.contains("LOGIN"));

        // STATUS
        writer
            .write_all(b"A0 STATUS INBOX (MESSAGES UNSEEN UIDNEXT UIDVALIDITY)\r\n")
            .await
            .unwrap();
        buf.clear();
        // Read multi-line STATUS response (untagged line + tagged OK)
        loop {
            reader.read_line(&mut buf).await.unwrap();
            if buf.contains("UIDVALIDITY 99") {
                // Consume the tagged OK line that follows
                buf.clear();
                reader.read_line(&mut buf).await.unwrap();
                break;
            }
        }

        // LOGOUT
        writer.write_all(b"A0 LOGOUT\r\n").await.unwrap();
        buf.clear();
        reader.read_line(&mut buf).await.unwrap();
        assert!(buf.contains("BYE"));
    }

    #[tokio::test]
    async fn test_tag_substitution_in_response() {
        // Use {TAG} placeholder in the response and verify it gets the
        // client's actual tag ("A5") substituted in.
        let handle = MockImapServer::new()
            .respond("LOGIN", "{TAG} OK LOGIN succeeded\r\n")
            .start()
            .await;

        let mut stream = TcpStream::connect((handle.host(), handle.port()))
            .await
            .unwrap();
        let (reader, mut writer) = stream.split();
        let mut reader = BufReader::new(reader);

        // Read greeting
        let mut buf = String::new();
        reader.read_line(&mut buf).await.unwrap();

        // Send LOGIN with non-standard tag
        writer.write_all(b"A5 LOGIN u p\r\n").await.unwrap();
        buf.clear();
        reader.read_line(&mut buf).await.unwrap();

        assert!(
            buf.contains("A5 OK LOGIN succeeded"),
            "expected 'A5 OK LOGIN succeeded', got '{buf}'"
        );
    }

    #[tokio::test]
    async fn test_tag_substitution_multiple_placeholders() {
        let handle = MockImapServer::new()
            .respond("NOOP", "* 0 RECENT\r\n{TAG} OK NOOP done\r\n")
            .start()
            .await;

        let mut stream = TcpStream::connect((handle.host(), handle.port()))
            .await
            .unwrap();
        let (reader, mut writer) = stream.split();
        let mut reader = BufReader::new(reader);

        // Read greeting
        let mut buf = String::new();
        reader.read_line(&mut buf).await.unwrap();

        // Send with tag "B99"
        writer.write_all(b"B99 NOOP\r\n").await.unwrap();

        // Read all lines
        let mut all = String::new();
        loop {
            buf.clear();
            reader.read_line(&mut buf).await.unwrap();
            all.push_str(&buf);
            if buf.starts_with("B99") {
                break;
            }
        }

        assert!(all.contains("* 0 RECENT\r\n"));
        assert!(all.contains("B99 OK NOOP done\r\n"));
    }
}
