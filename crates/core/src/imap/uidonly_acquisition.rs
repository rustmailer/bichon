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

//! Streaming, fail-closed acquisition for RFC 9586 UIDONLY mailboxes.
//!
//! There is deliberately no secondary ledger. The canonical envelope plus
//! verified exact-raw blob is the per-UID receipt. Transient disconnects retry
//! the same operation after re-proving the fixed mailbox epoch; a failed run
//! leaves the mailbox checkpoint unchanged.

use crate::account::entity::{AuthType, Encryption};
use crate::account::migration::AccountModel;
use crate::cache::imap::mailbox::MailBox;
use crate::envelope::extractor::{
    project_uidonly_messages, verify_uidonly_projections, UidOnlyMessage,
    UIDONLY_PROJECTION_BATCH_BYTES, UIDONLY_PROJECTION_BATCH_MESSAGES,
};
use crate::error::code::ErrorCode;
use crate::error::BichonResult;
use crate::imap::executor::DEFAULT_MAX_EMAIL_SIZE;
use crate::imap::manager::ImapConnectionManager;
use crate::imap::session::SessionStream;
use crate::imap::uidonly::{exact_args, inventory_args, UidOnlyHandle, UidOnlyLimits};
use crate::raise_error;
use async_imap::Session;
use futures::{FutureExt, TryStreamExt};
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

const UIDONLY_DEFAULT_PAGE_SIZE: u32 = 1_000;
const MAX_UIDONLY_RECONNECTS: u32 = 3;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct AcquisitionLimits {
    pub max_literal_bytes: u64,
    pub max_operation_runtime: Duration,
    pub page_size: u32,
}

impl AcquisitionLimits {
    pub(crate) fn bounded(max_literal_bytes: u64) -> Self {
        Self {
            max_literal_bytes,
            // Bound each network or durable-storage operation, not the whole
            // archive: a valid large mailbox may need to run for days.
            max_operation_runtime: Duration::from_secs(10 * 60),
            page_size: 1_000,
        }
    }

    fn validate(self) -> BichonResult<Self> {
        if self.max_literal_bytes == 0
            || self.max_operation_runtime.is_zero()
            || self.page_size == 0
        {
            return Err(raise_error!(
                "UIDONLY acquisition limits must be nonzero".into(),
                ErrorCode::InvalidParameter
            ));
        }
        Ok(self)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct MailboxSnapshot {
    pub exists: u32,
    pub uid_validity: u32,
    pub uid_next: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct InventoryItem {
    pub uid: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct AcquisitionProgress {
    pub planned: u64,
    pub resolved: u64,
    pub downloaded: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct AcquisitionReport {
    pub uid_validity: u32,
    pub uid_next: u32,
    pub exists: u32,
    pub checkpoint: Option<u32>,
    pub inventoried: u64,
    pub archived: u64,
}

#[allow(async_fn_in_trait)]
pub(crate) trait UidOnlyTransport {
    async fn snapshot(&mut self, mailbox: &str) -> BichonResult<MailboxSnapshot>;

    async fn inventory_page(
        &mut self,
        cursor: u32,
        high: u32,
        page_size: u32,
    ) -> BichonResult<Vec<InventoryItem>>;

    /// Fetch exactly one full message. `literal_budget` is a pre-read ceiling,
    /// not a post-read accounting hint.
    async fn fetch_exact(&mut self, uid: u32, literal_budget: u64) -> BichonResult<UidOnlyMessage>;

    async fn reconnect(&mut self, _page_size: u32) -> BichonResult<()> {
        Err(raise_error!(
            "UIDONLY transport cannot reconnect".into(),
            ErrorCode::NetworkError
        ))
    }
}

#[allow(async_fn_in_trait)]
pub(crate) trait CanonicalArchive {
    fn resume_after(&self) -> Option<u32> {
        None
    }

    fn begin_epoch(&mut self, _uid_validity: u32) -> BichonResult<()> {
        Ok(())
    }

    async fn verify_many(&mut self, uids: &[u32]) -> BichonResult<Vec<bool>>;

    /// A successful result means every stored message has passed the durable
    /// raw readback and final envelope-marker commit barrier.
    async fn project_many(&mut self, messages: Vec<UidOnlyMessage>) -> BichonResult<()>;
}

async fn bounded<F, T>(future: F, runtime: Duration, token: &CancellationToken) -> BichonResult<T>
where
    F: Future<Output = BichonResult<T>>,
{
    tokio::select! {
        _ = token.cancelled() => Err(raise_error!(
            "UIDONLY acquisition cancelled".into(),
            ErrorCode::InternalError
        )),
        result = tokio::time::timeout(runtime, future) => result.map_err(|_| raise_error!(
            "UIDONLY operation runtime ceiling exceeded".into(),
            ErrorCode::RequestTimeout
        ))?,
    }
}

fn retryable_transport_error(code: ErrorCode) -> bool {
    matches!(
        code,
        ErrorCode::NetworkError | ErrorCode::ConnectionTimeout | ErrorCode::RequestTimeout
    )
}

async fn recover_transport<T: UidOnlyTransport>(
    transport: &mut T,
    mailbox: &str,
    initial: MailboxSnapshot,
    limits: AcquisitionLimits,
    token: &CancellationToken,
    reconnects: &mut u32,
) -> BichonResult<()> {
    let mut last_error = raise_error!(
        "UIDONLY transport recovery failed".into(),
        ErrorCode::NetworkError
    );
    while *reconnects < MAX_UIDONLY_RECONNECTS {
        *reconnects += 1;
        let delay = if cfg!(test) {
            Duration::ZERO
        } else {
            Duration::from_secs(1 << (*reconnects - 1))
        };
        let reconnected = bounded(
            async {
                tokio::time::sleep(delay).await;
                transport.reconnect(limits.page_size).await
            },
            limits.max_operation_runtime,
            token,
        )
        .await;
        if let Err(error) = reconnected {
            if retryable_transport_error(error.code()) {
                last_error = error;
                continue;
            }
            return Err(error);
        }

        match bounded(
            transport.snapshot(mailbox),
            limits.max_operation_runtime,
            token,
        )
        .await
        {
            Ok(snapshot)
                if snapshot.uid_validity == initial.uid_validity
                    && snapshot.uid_next >= initial.uid_next =>
            {
                return Ok(())
            }
            Ok(_) => {
                return Err(raise_error!(
                    "UIDONLY mailbox epoch changed while reconnecting".into(),
                    ErrorCode::Incompatible
                ))
            }
            Err(error) if retryable_transport_error(error.code()) => last_error = error,
            Err(error) => return Err(error),
        }
    }
    Err(last_error)
}

async fn inventory_with_reconnect<T: UidOnlyTransport>(
    transport: &mut T,
    mailbox: &str,
    initial: MailboxSnapshot,
    cursor: u32,
    high: u32,
    limits: AcquisitionLimits,
    token: &CancellationToken,
) -> BichonResult<Vec<InventoryItem>> {
    let mut reconnects = 0;
    loop {
        match bounded(
            transport.inventory_page(cursor, high, limits.page_size),
            limits.max_operation_runtime,
            token,
        )
        .await
        {
            Err(error) if retryable_transport_error(error.code()) => {
                recover_transport(transport, mailbox, initial, limits, token, &mut reconnects)
                    .await?;
            }
            result => return result,
        }
    }
}

async fn fetch_with_reconnect<T: UidOnlyTransport>(
    transport: &mut T,
    mailbox: &str,
    initial: MailboxSnapshot,
    uid: u32,
    limits: AcquisitionLimits,
    token: &CancellationToken,
) -> BichonResult<UidOnlyMessage> {
    let mut reconnects = 0;
    loop {
        match bounded(
            transport.fetch_exact(uid, limits.max_literal_bytes),
            limits.max_operation_runtime,
            token,
        )
        .await
        {
            Err(error) if retryable_transport_error(error.code()) => {
                recover_transport(transport, mailbox, initial, limits, token, &mut reconnects)
                    .await?;
            }
            result => return result,
        }
    }
}

async fn snapshot_with_reconnect<T: UidOnlyTransport>(
    transport: &mut T,
    mailbox: &str,
    initial: MailboxSnapshot,
    limits: AcquisitionLimits,
    token: &CancellationToken,
) -> BichonResult<MailboxSnapshot> {
    let mut reconnects = 0;
    loop {
        match bounded(
            transport.snapshot(mailbox),
            limits.max_operation_runtime,
            token,
        )
        .await
        {
            Err(error) if retryable_transport_error(error.code()) => {
                recover_transport(transport, mailbox, initial, limits, token, &mut reconnects)
                    .await?;
            }
            result => return result,
        }
    }
}

async fn flush_projection_batch<A: CanonicalArchive>(
    archive: &mut A,
    pending: &mut Vec<UidOnlyMessage>,
    limits: AcquisitionLimits,
    token: &CancellationToken,
) -> BichonResult<u64> {
    if pending.is_empty() {
        return Ok(0);
    }
    let count = pending.len() as u64;
    bounded(
        archive.project_many(std::mem::take(pending)),
        limits.max_operation_runtime,
        token,
    )
    .await?;
    Ok(count)
}

/// Reconciles one immutable UID range. Any ambiguity is an error, so callers
/// must persist `checkpoint` only from a returned report.
pub(crate) async fn run_acquisition<T, A, P>(
    transport: &mut T,
    archive: &mut A,
    mailbox: &str,
    expected_uid_validity: Option<u32>,
    limits: AcquisitionLimits,
    token: CancellationToken,
    mut progress: P,
) -> BichonResult<AcquisitionReport>
where
    T: UidOnlyTransport,
    A: CanonicalArchive,
    P: FnMut(AcquisitionProgress) -> BichonResult<()>,
{
    let limits = limits.validate()?;
    let snapshot = bounded(
        transport.snapshot(mailbox),
        limits.max_operation_runtime,
        &token,
    )
    .await?;
    if snapshot.uid_validity == 0 || snapshot.uid_next == 0 {
        return Err(raise_error!(
            "UIDONLY EXAMINE omitted a valid UIDVALIDITY or UIDNEXT".into(),
            ErrorCode::ImapUnexpectedResult
        ));
    }
    if expected_uid_validity.is_some_and(|expected| expected != snapshot.uid_validity) {
        return Err(raise_error!(
            "UIDVALIDITY changed between mailbox discovery and UIDONLY EXAMINE".into(),
            ErrorCode::Incompatible
        ));
    }
    archive.begin_epoch(snapshot.uid_validity)?;
    let planned = u64::from(snapshot.exists);
    let high = snapshot.uid_next - 1;
    let resume_after = archive.resume_after();
    if resume_after.is_some_and(|checkpoint| checkpoint > high) {
        return Err(raise_error!(
            "UIDONLY UIDNEXT moved behind the proven checkpoint".into(),
            ErrorCode::Incompatible
        ));
    }
    let full_snapshot = resume_after.is_none();
    let mut cursor = resume_after.map_or(1, |uid| uid.saturating_add(1));
    let mut inventoried = 0_u64;
    let mut archived = 0_u64;
    let mut downloaded = 0_u64;
    let mut pending = Vec::with_capacity(UIDONLY_PROJECTION_BATCH_MESSAGES);
    let mut pending_bytes = 0_u64;

    progress(AcquisitionProgress {
        planned,
        resolved: 0,
        downloaded: 0,
    })?;

    while high > 0 && cursor <= high {
        let page =
            inventory_with_reconnect(transport, mailbox, snapshot, cursor, high, limits, &token)
                .await?;
        if page.len() > limits.page_size as usize {
            return Err(raise_error!(
                "UIDONLY inventory page exceeded its requested bound".into(),
                ErrorCode::ImapUnexpectedResult
            ));
        }
        if page.is_empty() {
            break;
        }

        let mut previous = cursor - 1;
        for item in &page {
            if token.is_cancelled() {
                return Err(raise_error!(
                    "UIDONLY acquisition cancelled".into(),
                    ErrorCode::InternalError
                ));
            }
            if item.uid < cursor || item.uid > high || item.uid <= previous {
                return Err(raise_error!(
                    "UIDONLY inventory was duplicate, unordered, or outside the fixed range".into(),
                    ErrorCode::ImapUnexpectedResult
                ));
            }
            previous = item.uid;
            inventoried = inventoried.checked_add(1).ok_or_else(|| {
                raise_error!(
                    "UIDONLY inventory count overflow".into(),
                    ErrorCode::PayloadTooLarge
                )
            })?;
        }
        let uids = page.iter().map(|item| item.uid).collect::<Vec<_>>();
        let verified = bounded(
            archive.verify_many(&uids),
            limits.max_operation_runtime,
            &token,
        )
        .await?;
        if verified.len() != page.len() {
            return Err(raise_error!(
                "UIDONLY receipt lookup returned the wrong result count".into(),
                ErrorCode::InternalError
            ));
        }

        for (item, is_verified) in page.into_iter().zip(verified) {
            if is_verified {
                archived += 1;
                continue;
            }

            let message =
                fetch_with_reconnect(transport, mailbox, snapshot, item.uid, limits, &token)
                    .await?;
            if message.uid != item.uid {
                return Err(raise_error!(
                    "UIDONLY exact fetch returned the wrong UID".into(),
                    ErrorCode::ImapUnexpectedResult
                ));
            }
            let actual = message.body.len() as u64;
            if actual > limits.max_literal_bytes {
                return Err(raise_error!(
                    "UIDONLY exact body exceeded its pre-read budget".into(),
                    ErrorCode::PayloadTooLarge
                ));
            }
            if !pending.is_empty()
                && (pending.len() >= UIDONLY_PROJECTION_BATCH_MESSAGES
                    || pending_bytes.saturating_add(actual) > UIDONLY_PROJECTION_BATCH_BYTES as u64)
            {
                let stored = flush_projection_batch(archive, &mut pending, limits, &token).await?;
                archived += stored;
                downloaded += stored;
                pending_bytes = 0;
            }
            pending_bytes += actual;
            pending.push(message);
        }

        let stored = flush_projection_batch(archive, &mut pending, limits, &token).await?;
        archived += stored;
        downloaded += stored;
        pending_bytes = 0;

        cursor = previous.checked_add(1).unwrap_or(u32::MAX);
        progress(AcquisitionProgress {
            planned,
            resolved: inventoried,
            downloaded,
        })?;
        if previous == u32::MAX {
            break;
        }
    }

    if (full_snapshot && inventoried != planned) || archived != inventoried {
        return Err(raise_error!(
            "UIDONLY inventory did not reconcile the EXAMINE message count".into(),
            ErrorCode::ImapUnexpectedResult
        ));
    }
    let final_snapshot =
        snapshot_with_reconnect(transport, mailbox, snapshot, limits, &token).await?;
    if final_snapshot.uid_validity != snapshot.uid_validity
        || final_snapshot.uid_next < snapshot.uid_next
    {
        return Err(raise_error!(
            "UIDONLY mailbox epoch changed during acquisition".into(),
            ErrorCode::Incompatible
        ));
    }
    Ok(AcquisitionReport {
        uid_validity: snapshot.uid_validity,
        uid_next: snapshot.uid_next,
        exists: snapshot.exists,
        checkpoint: (high > 0).then_some(high),
        inventoried,
        archived,
    })
}

pub(crate) enum AcquisitionRoute {
    Acquired {
        report: AcquisitionReport,
        source_scope: String,
    },
    Legacy(Session<Box<dyn SessionStream>>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CapabilityRoute {
    Legacy,
    UidOnly { message_limit: u32 },
}

fn is_capability(value: &str, expected: &str) -> bool {
    value.trim().eq_ignore_ascii_case(expected)
}

fn is_message_limit_marker(value: &str) -> bool {
    value
        .trim()
        .get(.."MESSAGELIMIT".len())
        .is_some_and(|head| head.eq_ignore_ascii_case("MESSAGELIMIT"))
}

fn classify_capabilities(capabilities: &[String]) -> BichonResult<CapabilityRoute> {
    let uidonly = capabilities
        .iter()
        .filter(|value| is_capability(value, "UIDONLY"))
        .count();
    let partial = capabilities
        .iter()
        .filter(|value| is_capability(value, "PARTIAL"))
        .count();
    let limits: Vec<_> = capabilities
        .iter()
        .filter(|value| is_message_limit_marker(value))
        .collect();

    // PARTIAL is a standalone RFC 9394 extension. It is not by itself
    // evidence that ordinary mailbox views are limited.
    if uidonly == 0 && limits.is_empty() {
        return Ok(CapabilityRoute::Legacy);
    }
    if uidonly != 1 || partial != 1 || limits.len() != 1 {
        return Err(raise_error!(
            "Server advertised an incomplete or ambiguous UIDONLY capability set".into(),
            ErrorCode::Incompatible
        ));
    }
    let value = limits[0].trim();
    let prefix = "MESSAGELIMIT=";
    let number = value
        .get(prefix.len()..)
        .filter(|_| {
            value
                .get(..prefix.len())
                .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
        })
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|value| *value > 0)
        .ok_or_else(|| {
            raise_error!(
                "Server advertised an invalid UIDONLY MESSAGELIMIT".into(),
                ErrorCode::Incompatible
            )
        })?;
    Ok(CapabilityRoute::UidOnly {
        message_limit: number,
    })
}

fn require_uidonly_opt_in(
    account: &AccountModel,
    capability_route: CapabilityRoute,
) -> BichonResult<()> {
    if matches!(capability_route, CapabilityRoute::UidOnly { .. }) && !account.uidonly_enabled {
        return Err(raise_error!(
            "Server requires UIDONLY acquisition; enable uidonly_enabled for this account after reviewing the storage transition".into(),
            ErrorCode::Incompatible
        ));
    }
    Ok(())
}

/// Cached capabilities only decide whether legacy reconciliation may be
/// bypassed. The acquisition connection always reclassifies fresh capabilities.
pub(crate) fn account_requires_uidonly(account: &AccountModel) -> bool {
    account.capabilities.as_ref().is_some_and(|capabilities| {
        capabilities
            .iter()
            .any(|value| is_capability(value, "UIDONLY") || is_message_limit_marker(value))
    })
}

pub(crate) fn mailbox_has_uidonly_proof(account: &AccountModel, mailbox: &MailBox) -> bool {
    source_scope(account)
        .ok()
        .as_deref()
        .is_some_and(|scope| mailbox.uidonly_source_scope.as_deref() == Some(scope))
}

fn source_scope(account: &AccountModel) -> BichonResult<String> {
    let imap = account.imap.as_ref().ok_or_else(|| {
        raise_error!(
            "UIDONLY account has no IMAP configuration".into(),
            ErrorCode::MissingConfiguration
        )
    })?;
    let host = imap.host.trim().trim_end_matches('.').to_ascii_lowercase();
    let principal = account.login_name.as_ref().unwrap_or(&account.email);
    if host.is_empty() || principal.is_empty() || imap.port == 0 {
        return Err(raise_error!(
            "UIDONLY source identity is incomplete".into(),
            ErrorCode::MissingConfiguration
        ));
    }
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"bichon-uidonly-source-v2\0");
    for field in [host.as_bytes(), principal.as_bytes()] {
        hasher.update(&(field.len() as u64).to_be_bytes());
        hasher.update(field);
    }
    hasher.update(&imap.port.to_be_bytes());
    Ok(hasher.finalize().to_hex().to_string())
}

fn connection_scope(account: &AccountModel) -> BichonResult<String> {
    let imap = account.imap.as_ref().ok_or_else(|| {
        raise_error!(
            "UIDONLY account has no IMAP configuration".into(),
            ErrorCode::MissingConfiguration
        )
    })?;
    let encryption = match imap.encryption {
        Encryption::Ssl => 1_u8,
        Encryption::StartTls => 2,
        Encryption::None => 3,
    };
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"bichon-uidonly-connection-v1\0");
    hasher.update(source_scope(account)?.as_bytes());
    hasher.update(&[encryption, u8::from(account.use_dangerous)]);
    match imap.use_proxy {
        Some(proxy) => {
            hasher.update(&[1]);
            hasher.update(&proxy.to_be_bytes());
        }
        None => {
            hasher.update(&[0]);
        }
    }
    hasher.update(&[match imap.auth.auth_type {
        AuthType::Password => 1,
        AuthType::OAuth2 => 2,
    }]);
    Ok(hasher.finalize().to_hex().to_string())
}

fn protocol_limits(max_literal_bytes: u64) -> BichonResult<UidOnlyLimits> {
    if max_literal_bytes == 0 || max_literal_bytes > u64::from(u32::MAX) {
        return Err(raise_error!(
            "UIDONLY message-size limit is outside the supported range".into(),
            ErrorCode::InvalidParameter
        ));
    }
    let literal = usize::try_from(max_literal_bytes).map_err(|_| {
        raise_error!(
            "UIDONLY message-size limit is not representable".into(),
            ErrorCode::InvalidParameter
        )
    })?;
    let response = literal.checked_add(128 * 1024).ok_or_else(|| {
        raise_error!(
            "UIDONLY response-size limit overflow".into(),
            ErrorCode::InvalidParameter
        )
    })?;
    let command = response.checked_add(128 * 1024).ok_or_else(|| {
        raise_error!(
            "UIDONLY command-size limit overflow".into(),
            ErrorCode::InvalidParameter
        )
    })?;
    Ok(UidOnlyLimits {
        max_control_line_bytes: 64 * 1024,
        max_literal_bytes: literal,
        max_response_bytes: response,
        max_command_literal_bytes: literal,
        max_command_response_bytes: command,
        // A 1,000-message inventory page plus bounded unsolicited mailbox
        // updates must still fit without making the response count unbounded.
        max_command_responses: 2_048,
        max_command_runtime: Duration::from_secs(5 * 60),
    })
}

fn protocol_probe_literal(max_literal_bytes: u64) -> u64 {
    let representable = u64::from(u32::MAX).min(usize::MAX.saturating_sub(256 * 1024) as u64);
    max_literal_bytes.clamp(1, representable)
}

fn imap_error_code(error: &async_imap::error::Error) -> ErrorCode {
    match error {
        async_imap::error::Error::ConnectionLost => ErrorCode::NetworkError,
        async_imap::error::Error::Io(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::BrokenPipe
                    | std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::ConnectionAborted
                    | std::io::ErrorKind::NotConnected
                    | std::io::ErrorKind::UnexpectedEof
                    | std::io::ErrorKind::TimedOut
                    | std::io::ErrorKind::WriteZero
            ) =>
        {
            ErrorCode::NetworkError
        }
        _ => ErrorCode::ImapUnexpectedResult,
    }
}

struct SessionUidOnlyTransport {
    session: Session<Box<dyn SessionStream>>,
    handle: UidOnlyHandle,
    account: AccountModel,
    limits: UidOnlyLimits,
    connection_scope: String,
}

impl SessionUidOnlyTransport {
    fn healthy_handle(handle: &UidOnlyHandle) -> BichonResult<()> {
        if let Some(reason) = handle.poison_reason() {
            return Err(raise_error!(reason, ErrorCode::ImapUnexpectedResult));
        }
        handle.ensure_active().map_err(|_| {
            raise_error!(
                "UIDONLY transport is not active".into(),
                ErrorCode::ImapUnexpectedResult
            )
        })
    }

    fn healthy(&self) -> BichonResult<()> {
        Self::healthy_handle(&self.handle)
    }

    async fn enable(
        session: &mut Session<Box<dyn SessionStream>>,
        handle: &UidOnlyHandle,
    ) -> BichonResult<()> {
        session
            .run_command_and_check_ok("ENABLE UIDONLY")
            .await
            .map_err(|error| {
                raise_error!(
                    "Server did not enable UIDONLY".into(),
                    imap_error_code(&error)
                )
            })?;
        Self::healthy_handle(handle)
    }

    async fn collect_fetches(
        &mut self,
        set: String,
        query: impl AsRef<str>,
    ) -> BichonResult<Vec<async_imap::types::Fetch>> {
        let query = query.as_ref().to_string();
        let operation = async {
            let stream = self.session.uid_fetch(set, query).await.map_err(|error| {
                raise_error!(
                    "UIDONLY fetch command failed".into(),
                    imap_error_code(&error)
                )
            })?;
            stream.try_collect::<Vec<_>>().await.map_err(|error| {
                raise_error!(
                    "UIDONLY fetch response failed".into(),
                    imap_error_code(&error)
                )
            })
        };
        let result = AssertUnwindSafe(operation)
            .catch_unwind()
            .await
            .map_err(|_| {
                raise_error!(
                    "UIDONLY parser rejected a malformed response".into(),
                    ErrorCode::ImapUnexpectedResult
                )
            })??;
        self.healthy()?;
        Ok(result)
    }
}

impl UidOnlyTransport for SessionUidOnlyTransport {
    async fn snapshot(&mut self, mailbox: &str) -> BichonResult<MailboxSnapshot> {
        self.healthy()?;
        let mailbox = AssertUnwindSafe(self.session.examine(mailbox))
            .catch_unwind()
            .await
            .map_err(|_| {
                raise_error!(
                    "UIDONLY parser rejected EXAMINE".into(),
                    ErrorCode::ImapUnexpectedResult
                )
            })?
            .map_err(|error| {
                raise_error!("UIDONLY EXAMINE failed".into(), imap_error_code(&error))
            })?;
        self.healthy()?;
        Ok(MailboxSnapshot {
            exists: mailbox.exists,
            uid_validity: mailbox.uid_validity.ok_or_else(|| {
                raise_error!(
                    "UIDONLY EXAMINE omitted UIDVALIDITY".into(),
                    ErrorCode::ImapUnexpectedResult
                )
            })?,
            uid_next: mailbox.uid_next.ok_or_else(|| {
                raise_error!(
                    "UIDONLY EXAMINE omitted UIDNEXT".into(),
                    ErrorCode::ImapUnexpectedResult
                )
            })?,
        })
    }

    async fn inventory_page(
        &mut self,
        cursor: u32,
        high: u32,
        page_size: u32,
    ) -> BichonResult<Vec<InventoryItem>> {
        let (set, query) = inventory_args(cursor, high, page_size).map_err(|_| {
            raise_error!(
                "Invalid UIDONLY inventory bounds".into(),
                ErrorCode::InvalidParameter
            )
        })?;
        self.collect_fetches(set, query)
            .await?
            .into_iter()
            .map(|fetch| {
                let uid = fetch.uid.ok_or_else(|| {
                    raise_error!(
                        "UIDONLY inventory omitted UID".into(),
                        ErrorCode::ImapUnexpectedResult
                    )
                })?;
                if fetch.message != uid {
                    return Err(raise_error!(
                        "UIDONLY inventory leading UID did not match UID data item".into(),
                        ErrorCode::ImapUnexpectedResult
                    ));
                }
                fetch.size.ok_or_else(|| {
                    raise_error!(
                        "UIDONLY inventory omitted RFC822.SIZE".into(),
                        ErrorCode::ImapUnexpectedResult
                    )
                })?;
                Ok(InventoryItem { uid })
            })
            .collect()
    }

    async fn fetch_exact(&mut self, uid: u32, literal_budget: u64) -> BichonResult<UidOnlyMessage> {
        let limit = usize::try_from(literal_budget).map_err(|_| {
            raise_error!(
                "UIDONLY literal budget is not representable".into(),
                ErrorCode::InvalidParameter
            )
        })?;
        self.handle
            .arm_next_fetch_literal_limit(limit)
            .map_err(|_| {
                raise_error!(
                    "UIDONLY exact fetch could not arm its pre-read limit".into(),
                    ErrorCode::ImapUnexpectedResult
                )
            })?;
        let before = self.handle.literal_bytes_received();
        let (set, query) = exact_args(uid).map_err(|_| {
            raise_error!(
                "Invalid UIDONLY exact UID".into(),
                ErrorCode::InvalidParameter
            )
        })?;
        let mut fetched = self.collect_fetches(set, query).await?;
        if fetched.len() != 1 {
            return Err(raise_error!(
                "UIDONLY exact fetch did not return exactly one message".into(),
                ErrorCode::ImapUnexpectedResult
            ));
        }
        let fetch = fetched.pop().expect("length checked");
        if fetch.message != uid || fetch.uid != Some(uid) || fetch.size.is_none() {
            return Err(raise_error!(
                "UIDONLY exact fetch returned mismatched metadata".into(),
                ErrorCode::ImapUnexpectedResult
            ));
        }
        let raw = fetch.body().ok_or_else(|| {
            raise_error!(
                "UIDONLY exact fetch omitted its full literal body".into(),
                ErrorCode::ImapUnexpectedResult
            )
        })?;
        let after = self.handle.literal_bytes_received();
        if after.checked_sub(before) != Some(raw.len() as u64) {
            return Err(raise_error!(
                "UIDONLY exact body did not match literal accounting".into(),
                ErrorCode::ImapUnexpectedResult
            ));
        }
        Ok(UidOnlyMessage {
            uid,
            body: raw.to_vec(),
        })
    }

    async fn reconnect(&mut self, page_size: u32) -> BichonResult<()> {
        let current = AccountModel::get(self.account.id)?;
        if connection_scope(&current)? != self.connection_scope {
            return Err(raise_error!(
                "UIDONLY account connection changed while reconnecting".into(),
                ErrorCode::Incompatible
            ));
        }
        let connection =
            ImapConnectionManager::build_uidonly(&self.account, self.limits.clone()).await?;
        let CapabilityRoute::UidOnly { message_limit } =
            classify_capabilities(&connection.capabilities)?
        else {
            return Err(raise_error!(
                "UIDONLY reconnect lost required capabilities".into(),
                ErrorCode::Incompatible
            ));
        };
        if message_limit < page_size {
            return Err(raise_error!(
                "UIDONLY reconnect reduced MESSAGELIMIT below the fixed page size".into(),
                ErrorCode::Incompatible
            ));
        }
        let mut session = connection.session;
        Self::enable(&mut session, &connection.handle).await?;
        self.session = session;
        self.handle = connection.handle;
        Ok(())
    }
}

struct BichonCanonicalArchive {
    account_id: u64,
    mailbox_id: u64,
    source_scope: String,
    uid_validity: Option<u32>,
    resume_after: Option<u32>,
}

impl CanonicalArchive for BichonCanonicalArchive {
    fn resume_after(&self) -> Option<u32> {
        self.resume_after
    }

    fn begin_epoch(&mut self, uid_validity: u32) -> BichonResult<()> {
        if self
            .uid_validity
            .is_some_and(|existing| existing != uid_validity)
        {
            return Err(raise_error!(
                "UIDONLY archive epoch changed".into(),
                ErrorCode::Incompatible
            ));
        }
        self.uid_validity = Some(uid_validity);
        Ok(())
    }

    async fn verify_many(&mut self, uids: &[u32]) -> BichonResult<Vec<bool>> {
        verify_uidonly_projections(
            self.account_id,
            self.mailbox_id,
            self.uid_validity.expect("begin_epoch is called first"),
            uids,
            &self.source_scope,
        )
    }

    async fn project_many(&mut self, messages: Vec<UidOnlyMessage>) -> BichonResult<()> {
        project_uidonly_messages(
            messages,
            self.account_id,
            self.mailbox_id,
            self.uid_validity.expect("begin_epoch is called first"),
            &self.source_scope,
        )
        .await
    }
}

pub(crate) async fn connect_and_acquire_or_legacy<P>(
    account: &AccountModel,
    mailbox: &MailBox,
    force_uidonly: bool,
    token: CancellationToken,
    progress: P,
) -> BichonResult<AcquisitionRoute>
where
    P: FnMut(AcquisitionProgress) -> BichonResult<()>,
{
    let configured_scope = source_scope(account)?;
    let known_limited = force_uidonly
        || account_requires_uidonly(account)
        || mailbox.uidonly_source_scope.as_deref() == Some(&configured_scope);
    if known_limited
        && account
            .archive_rules
            .as_ref()
            .is_some_and(|rules| rules.enabled)
    {
        return Err(raise_error!(
            "UIDONLY acquisition does not yet support enabled archive rules".into(),
            ErrorCode::Incompatible
        ));
    }
    let max_literal = account
        .max_email_size_bytes
        .unwrap_or(DEFAULT_MAX_EMAIL_SIZE);
    let wire_limits = protocol_limits(protocol_probe_literal(max_literal))?;
    let connection = ImapConnectionManager::build_uidonly(account, wire_limits.clone()).await?;
    let capability_route = classify_capabilities(&connection.capabilities)?;
    require_uidonly_opt_in(account, capability_route)?;
    if capability_route == CapabilityRoute::Legacy {
        if known_limited {
            return Err(raise_error!(
                "Known limited server omitted UIDONLY capabilities on the acquisition connection"
                    .into(),
                ErrorCode::Incompatible
            ));
        }
        return Ok(AcquisitionRoute::Legacy(connection.session));
    }
    let CapabilityRoute::UidOnly { message_limit } = capability_route else {
        unreachable!("legacy returned above")
    };
    let wire_limits = protocol_limits(max_literal)?;
    if account
        .archive_rules
        .as_ref()
        .is_some_and(|rules| rules.enabled)
    {
        return Err(raise_error!(
            "UIDONLY acquisition does not yet support enabled archive rules".into(),
            ErrorCode::Incompatible
        ));
    }

    let frozen_connection = connection_scope(account)?;
    let mut transport = SessionUidOnlyTransport {
        session: connection.session,
        handle: connection.handle,
        account: account.clone(),
        limits: wire_limits,
        connection_scope: frozen_connection.clone(),
    };
    SessionUidOnlyTransport::enable(&mut transport.session, &transport.handle).await?;

    let frozen_scope = configured_scope;
    let resume_after = (mailbox.uidonly_source_scope.as_deref() == Some(&frozen_scope))
        .then_some(mailbox.highest_uid)
        .flatten();
    let mut archive = BichonCanonicalArchive {
        account_id: account.id,
        mailbox_id: mailbox.id,
        source_scope: frozen_scope.clone(),
        uid_validity: None,
        resume_after,
    };
    let mut limits = AcquisitionLimits::bounded(max_literal);
    limits.page_size = UIDONLY_DEFAULT_PAGE_SIZE.min(message_limit).max(1);
    let result = run_acquisition(
        &mut transport,
        &mut archive,
        &mailbox.encoded_name(),
        mailbox.uid_validity,
        limits,
        token,
        progress,
    )
    .await;
    let report = match result {
        Ok(report) => report,
        Err(error) => return Err(error),
    };

    let current = AccountModel::get(account.id)?;
    if source_scope(&current)? != frozen_scope
        || connection_scope(&current)? != frozen_connection
        || current
            .archive_rules
            .as_ref()
            .is_some_and(|rules| rules.enabled)
    {
        return Err(raise_error!(
            "UIDONLY account source, connection policy, or archive rules changed during acquisition"
                .into(),
            ErrorCode::Incompatible
        ));
    }
    transport.session.logout().await.ok();
    Ok(AcquisitionRoute::Acquired {
        report,
        source_scope: frozen_scope,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account::entity::{AuthConfig, ImapConfig};
    use crate::imap::client::Client;
    use crate::imap::mock_server::{examine_response, MockImapServer, MockImapServerHandle};
    use std::collections::{BTreeMap, BTreeSet, VecDeque};

    fn limits() -> AcquisitionLimits {
        AcquisitionLimits {
            max_literal_bytes: 64 * 1024,
            max_operation_runtime: Duration::from_secs(20),
            page_size: 2,
        }
    }

    #[test]
    fn capability_routing_ignores_standalone_partial_and_rejects_partial_uidonly_sets() {
        assert_eq!(
            classify_capabilities(&["IMAP4rev1".into(), "PARTIAL".into()]).unwrap(),
            CapabilityRoute::Legacy
        );
        assert_eq!(
            classify_capabilities(&[
                "IMAP4rev1".into(),
                "uidonly".into(),
                "partial".into(),
                "messagelimit=10000".into(),
            ])
            .unwrap(),
            CapabilityRoute::UidOnly {
                message_limit: 10_000
            }
        );
        for capabilities in [
            vec!["UIDONLY".into(), "PARTIAL".into()],
            vec!["MESSAGELIMIT=1000".into(), "PARTIAL".into()],
            vec!["UIDONLY".into(), "PARTIAL".into(), "MESSAGELIMIT=0".into()],
        ] {
            assert!(classify_capabilities(&capabilities).is_err());
        }
        let cached = |capability: &str| AccountModel {
            capabilities: Some(vec![capability.into()]),
            ..Default::default()
        };
        assert!(!account_requires_uidonly(&cached("PARTIAL")));
        assert!(account_requires_uidonly(&cached("UIDONLY")));
        assert!(account_requires_uidonly(&cached("MESSAGELIMIT=10000")));
    }

    #[test]
    fn uidonly_route_requires_explicit_account_opt_in() {
        let route = CapabilityRoute::UidOnly {
            message_limit: 10_000,
        };
        let error = require_uidonly_opt_in(&AccountModel::default(), route).unwrap_err();
        assert_eq!(error.code(), ErrorCode::Incompatible);
        assert!(error.to_string().contains("enable uidonly_enabled"));

        let enabled = AccountModel {
            uidonly_enabled: true,
            ..Default::default()
        };
        assert!(require_uidonly_opt_in(&enabled, route).is_ok());
        assert!(require_uidonly_opt_in(&AccountModel::default(), CapabilityRoute::Legacy).is_ok());
    }

    #[test]
    fn capability_probe_accepts_limits_that_only_uidonly_rejects() {
        for configured in [0, u64::from(u32::MAX) + 1] {
            assert!(protocol_limits(protocol_probe_literal(configured)).is_ok());
            assert!(protocol_limits(configured).is_err());
        }
    }

    #[test]
    fn source_identity_is_stable_across_connection_policy_changes() {
        let account = AccountModel {
            email: "alice@example.invalid".into(),
            imap: Some(ImapConfig {
                host: "IMAP.EXAMPLE.INVALID.".into(),
                port: 993,
                encryption: Encryption::Ssl,
                auth: AuthConfig {
                    auth_type: AuthType::Password,
                    password: None,
                },
                use_proxy: None,
            }),
            ..Default::default()
        };
        let source = source_scope(&account).unwrap();
        let connection = connection_scope(&account).unwrap();
        for changed in [
            AccountModel {
                use_dangerous: true,
                ..account.clone()
            },
            AccountModel {
                imap: account.imap.clone().map(|mut imap| {
                    imap.use_proxy = Some(7);
                    imap
                }),
                ..account.clone()
            },
            AccountModel {
                imap: account.imap.clone().map(|mut imap| {
                    imap.auth.auth_type = AuthType::OAuth2;
                    imap
                }),
                ..account.clone()
            },
        ] {
            assert_eq!(source_scope(&changed).unwrap(), source);
            assert_ne!(connection_scope(&changed).unwrap(), connection);
        }
        let mut other_principal = account;
        other_principal.login_name = Some("other@example.invalid".into());
        assert_ne!(source_scope(&other_principal).unwrap(), source);
    }

    struct FakeTransport {
        snapshot: MailboxSnapshot,
        pages: VecDeque<Vec<InventoryItem>>,
        messages: BTreeMap<u32, Vec<u8>>,
        fetched: Vec<u32>,
        expected_cursor: Option<u32>,
    }

    impl FakeTransport {
        fn sparse() -> Self {
            let body = |uid| {
                format!("From: sender@invalid\r\nMessage-ID: <{uid}@invalid>\r\n\r\nbody")
                    .into_bytes()
            };
            Self {
                snapshot: MailboxSnapshot {
                    exists: 3,
                    uid_validity: 77,
                    uid_next: 51,
                },
                pages: VecDeque::from([
                    vec![InventoryItem { uid: 2 }, InventoryItem { uid: 30 }],
                    vec![InventoryItem { uid: 50 }],
                    vec![],
                ]),
                messages: BTreeMap::from([(2, body(2)), (30, body(30)), (50, body(50))]),
                fetched: Vec::new(),
                expected_cursor: None,
            }
        }
    }

    impl UidOnlyTransport for FakeTransport {
        async fn snapshot(&mut self, _mailbox: &str) -> BichonResult<MailboxSnapshot> {
            Ok(self.snapshot)
        }

        async fn inventory_page(
            &mut self,
            cursor: u32,
            _high: u32,
            _page_size: u32,
        ) -> BichonResult<Vec<InventoryItem>> {
            if let Some(expected) = self.expected_cursor.take() {
                assert_eq!(cursor, expected);
            }
            Ok(self.pages.pop_front().unwrap_or_default())
        }

        async fn fetch_exact(
            &mut self,
            uid: u32,
            literal_budget: u64,
        ) -> BichonResult<UidOnlyMessage> {
            self.fetched.push(uid);
            let raw = self.messages.get(&uid).cloned().ok_or_else(|| {
                raise_error!(
                    "synthetic missing body".into(),
                    ErrorCode::ImapUnexpectedResult
                )
            })?;
            if raw.len() as u64 > literal_budget {
                return Err(raise_error!(
                    "synthetic literal budget".into(),
                    ErrorCode::PayloadTooLarge
                ));
            }
            Ok(UidOnlyMessage { uid, body: raw })
        }
    }

    struct FlakyTransport {
        inner: FakeTransport,
        inventory_failures: u32,
        fetch_failures: u32,
        reconnects: u32,
        inventory_attempts: u32,
        fetch_attempts: Vec<u32>,
        snapshot_after_reconnect: Option<MailboxSnapshot>,
    }

    impl FlakyTransport {
        fn sparse() -> Self {
            Self {
                inner: FakeTransport::sparse(),
                inventory_failures: 0,
                fetch_failures: 0,
                reconnects: 0,
                inventory_attempts: 0,
                fetch_attempts: Vec::new(),
                snapshot_after_reconnect: None,
            }
        }
    }

    impl UidOnlyTransport for FlakyTransport {
        async fn snapshot(&mut self, mailbox: &str) -> BichonResult<MailboxSnapshot> {
            self.inner.snapshot(mailbox).await
        }

        async fn inventory_page(
            &mut self,
            cursor: u32,
            high: u32,
            page_size: u32,
        ) -> BichonResult<Vec<InventoryItem>> {
            self.inventory_attempts += 1;
            if self.inventory_failures > 0 {
                self.inventory_failures -= 1;
                return Err(raise_error!(
                    "synthetic disconnect".into(),
                    ErrorCode::NetworkError
                ));
            }
            self.inner.inventory_page(cursor, high, page_size).await
        }

        async fn fetch_exact(
            &mut self,
            uid: u32,
            literal_budget: u64,
        ) -> BichonResult<UidOnlyMessage> {
            self.fetch_attempts.push(uid);
            if self.fetch_failures > 0 {
                self.fetch_failures -= 1;
                return Err(raise_error!(
                    "synthetic disconnect".into(),
                    ErrorCode::NetworkError
                ));
            }
            self.inner.fetch_exact(uid, literal_budget).await
        }

        async fn reconnect(&mut self, _page_size: u32) -> BichonResult<()> {
            self.reconnects += 1;
            if let Some(snapshot) = self.snapshot_after_reconnect.take() {
                self.inner.snapshot = snapshot;
            }
            Ok(())
        }
    }

    #[derive(Default)]
    struct FakeArchive {
        verified: BTreeSet<u32>,
        projected: Vec<u32>,
        fail_project: bool,
        resume_after: Option<u32>,
    }

    impl CanonicalArchive for FakeArchive {
        fn resume_after(&self) -> Option<u32> {
            self.resume_after
        }

        async fn verify_many(&mut self, uids: &[u32]) -> BichonResult<Vec<bool>> {
            Ok(uids.iter().map(|uid| self.verified.contains(uid)).collect())
        }

        async fn project_many(&mut self, messages: Vec<UidOnlyMessage>) -> BichonResult<()> {
            if self.fail_project {
                return Err(raise_error!(
                    "synthetic projection failure".into(),
                    ErrorCode::InternalError
                ));
            }
            for message in messages {
                self.projected.push(message.uid);
                self.verified.insert(message.uid);
            }
            Ok(())
        }
    }

    async fn run_test<T: UidOnlyTransport, A: CanonicalArchive>(
        transport: &mut T,
        archive: &mut A,
    ) -> BichonResult<AcquisitionReport> {
        run_acquisition(
            transport,
            archive,
            "synthetic",
            Some(77),
            limits(),
            CancellationToken::new(),
            |_| Ok(()),
        )
        .await
    }

    #[tokio::test]
    async fn sparse_snapshot_streams_and_checkpoints_fixed_high() {
        let mut transport = FakeTransport::sparse();
        let mut archive = FakeArchive::default();
        let report = run_test(&mut transport, &mut archive).await.unwrap();
        assert_eq!(report.checkpoint, Some(50));
        assert_eq!((report.inventoried, report.archived), (3, 3));
        assert_eq!(transport.fetched, [2, 30, 50]);
    }

    #[tokio::test]
    async fn reconnect_retries_the_same_inventory_cursor() {
        let mut transport = FlakyTransport::sparse();
        transport.inventory_failures = 1;
        let report = run_test(&mut transport, &mut FakeArchive::default())
            .await
            .unwrap();

        assert_eq!(report.checkpoint, Some(50));
        assert_eq!(transport.reconnects, 1);
        assert_eq!(transport.inventory_attempts, 3);
    }

    #[tokio::test]
    async fn reconnect_retries_one_exact_uid_without_double_projection() {
        let mut transport = FlakyTransport::sparse();
        transport.fetch_failures = 1;
        let mut archive = FakeArchive::default();
        let report = run_test(&mut transport, &mut archive).await.unwrap();

        assert_eq!(report.archived, 3);
        assert_eq!(transport.reconnects, 1);
        assert_eq!(transport.fetch_attempts, [2, 2, 30, 50]);
        assert_eq!(archive.projected, [2, 30, 50]);
    }

    #[tokio::test]
    async fn reconnect_rejects_changed_uidvalidity_or_lower_uidnext() {
        for changed in [
            MailboxSnapshot {
                uid_validity: 78,
                ..FakeTransport::sparse().snapshot
            },
            MailboxSnapshot {
                uid_next: 50,
                ..FakeTransport::sparse().snapshot
            },
        ] {
            let mut transport = FlakyTransport::sparse();
            transport.inventory_failures = 1;
            transport.snapshot_after_reconnect = Some(changed);
            let error = run_test(&mut transport, &mut FakeArchive::default())
                .await
                .unwrap_err();

            assert_eq!(error.code(), ErrorCode::Incompatible);
            assert!(transport.inner.fetched.is_empty());
        }
    }

    #[tokio::test]
    async fn persistent_disconnect_uses_only_three_reconnects() {
        let mut transport = FlakyTransport::sparse();
        transport.inventory_failures = 4;
        let error = run_test(&mut transport, &mut FakeArchive::default())
            .await
            .unwrap_err();

        assert_eq!(error.code(), ErrorCode::NetworkError);
        assert_eq!(transport.reconnects, 3);
        assert_eq!(transport.inventory_attempts, 4);
    }

    #[tokio::test]
    async fn proven_checkpoint_scans_only_new_uids() {
        let mut transport = FakeTransport::sparse();
        transport.snapshot.exists = 4;
        transport.snapshot.uid_next = 60;
        transport.pages = VecDeque::from([vec![InventoryItem { uid: 55 }]]);
        transport
            .messages
            .insert(55, b"Subject: new\r\n\r\nbody".to_vec());
        transport.expected_cursor = Some(51);
        let report = run_test(
            &mut transport,
            &mut FakeArchive {
                resume_after: Some(50),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!((report.inventoried, report.archived), (1, 1));
        assert_eq!(report.checkpoint, Some(59));
        assert_eq!(transport.fetched, [55]);
    }

    #[tokio::test]
    async fn uidnext_behind_proven_checkpoint_fails_closed() {
        let mut transport = FakeTransport::sparse();
        let error = run_test(
            &mut transport,
            &mut FakeArchive {
                resume_after: Some(51),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
        assert_eq!(error.code(), ErrorCode::Incompatible);
        assert!(transport.fetched.is_empty());
    }

    #[tokio::test]
    async fn restart_skips_verified_receipts_without_a_secondary_ledger() {
        let mut transport = FakeTransport::sparse();
        let mut archive = FakeArchive {
            verified: BTreeSet::from([2, 30]),
            ..Default::default()
        };
        let report = run_test(&mut transport, &mut archive).await.unwrap();
        assert_eq!(report.archived, 3);
        assert_eq!(transport.fetched, [50]);
    }

    #[tokio::test]
    async fn short_inventory_never_returns_a_checkpoint() {
        let mut transport = FakeTransport::sparse();
        transport.snapshot.exists = 4;
        let error = run_test(&mut transport, &mut FakeArchive::default())
            .await
            .unwrap_err();
        assert_eq!(error.code(), ErrorCode::ImapUnexpectedResult);
    }

    #[tokio::test]
    async fn cancellation_and_projection_failure_abort_the_run() {
        let token = CancellationToken::new();
        let cancel = token.clone();
        let mut transport = FakeTransport::sparse();
        let error = run_acquisition(
            &mut transport,
            &mut FakeArchive::default(),
            "synthetic",
            Some(77),
            limits(),
            token,
            move |progress| {
                if progress.resolved > 0 {
                    cancel.cancel();
                }
                Ok(())
            },
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("cancelled"));

        let mut transport = FakeTransport::sparse();
        let error = run_test(
            &mut transport,
            &mut FakeArchive {
                fail_project: true,
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("synthetic projection failure"));
    }

    struct LogicalMillion {
        cursor: u32,
    }

    impl UidOnlyTransport for LogicalMillion {
        async fn snapshot(&mut self, _mailbox: &str) -> BichonResult<MailboxSnapshot> {
            Ok(MailboxSnapshot {
                exists: 1_000_000,
                uid_validity: 9,
                uid_next: 1_000_001,
            })
        }

        async fn inventory_page(
            &mut self,
            cursor: u32,
            high: u32,
            page_size: u32,
        ) -> BichonResult<Vec<InventoryItem>> {
            assert_eq!(cursor, self.cursor);
            let end = high.min(cursor + page_size - 1);
            self.cursor = end + 1;
            Ok((cursor..=end).map(|uid| InventoryItem { uid }).collect())
        }

        async fn fetch_exact(
            &mut self,
            _uid: u32,
            _literal_budget: u64,
        ) -> BichonResult<UidOnlyMessage> {
            unreachable!("every logical receipt verifies")
        }
    }

    struct AllVerified;

    impl CanonicalArchive for AllVerified {
        async fn verify_many(&mut self, uids: &[u32]) -> BichonResult<Vec<bool>> {
            Ok(vec![true; uids.len()])
        }

        async fn project_many(&mut self, _messages: Vec<UidOnlyMessage>) -> BichonResult<()> {
            unreachable!("every logical receipt verifies")
        }
    }

    #[tokio::test]
    async fn million_message_inventory_is_page_bounded() {
        let mut transport = LogicalMillion { cursor: 1 };
        let mut bounded = limits();
        bounded.page_size = 1_000;
        bounded.max_operation_runtime = Duration::from_secs(30);
        let report = run_acquisition(
            &mut transport,
            &mut AllVerified,
            "synthetic",
            Some(9),
            bounded,
            CancellationToken::new(),
            |_| Ok(()),
        )
        .await
        .unwrap();
        assert_eq!(report.inventoried, 1_000_000);
        assert_eq!(report.checkpoint, Some(1_000_000));
    }

    fn inventory_response(entries: &[(u32, u32)]) -> Vec<u8> {
        let mut response =
            b"* 3 EXISTS\r\n* OK [UIDNEXT 10] unchanged\r\n* 9 UIDFETCH (FLAGS (\\Seen))\r\n"
                .to_vec();
        for (uid, size) in entries {
            response.extend_from_slice(
                format!("* {uid} UIDFETCH (UID {uid} RFC822.SIZE {size})\r\n").as_bytes(),
            );
        }
        response.extend_from_slice(b"{TAG} OK inventory completed\r\n");
        response
    }

    fn exact_response(uid: u32, reported_size: u32, raw: &[u8]) -> Vec<u8> {
        let mut response = format!(
            "* {uid} UIDFETCH (UID {uid} RFC822.SIZE {reported_size} BODY[] {{{}}}\r\n",
            raw.len()
        )
        .into_bytes();
        response.extend_from_slice(raw);
        response.extend_from_slice(b")\r\n{TAG} OK exact fetch completed\r\n");
        response
    }

    async fn connect_fake_transport(server: &MockImapServerHandle) -> SessionUidOnlyTransport {
        let account = AccountModel {
            email: "synthetic-user".into(),
            imap: Some(ImapConfig {
                host: server.host(),
                port: server.port(),
                encryption: Encryption::None,
                auth: AuthConfig {
                    auth_type: AuthType::Password,
                    password: None,
                },
                use_proxy: None,
            }),
            ..Default::default()
        };
        let limits = protocol_limits(64 * 1024).expect("bounded protocol limits");
        let client = Client::connection(
            &server.host(),
            &Encryption::None,
            server.port(),
            None,
            false,
        )
        .await
        .expect("localhost connection");
        let (client, handle) = client
            .with_uidonly(limits.clone())
            .expect("install UIDONLY guard");
        let mut session = client
            .login("synthetic-user", "synthetic-secret")
            .await
            .expect("synthetic login");
        let capabilities = session.capabilities().await.expect("capabilities");
        assert!(capabilities.has_str("UIDONLY"));
        assert!(capabilities.has_str("PARTIAL"));
        assert!(capabilities.has_str("MESSAGELIMIT=2"));
        session
            .run_command_and_check_ok("ENABLE UIDONLY")
            .await
            .expect("enable UIDONLY");
        handle.ensure_active().expect("UIDONLY confirmed");
        SessionUidOnlyTransport {
            session,
            handle,
            connection_scope: connection_scope(&account).unwrap(),
            account,
            limits,
        }
    }

    #[tokio::test]
    async fn tcp_fake_yahoo_uidonly_pages_sparse_uids_and_checkpoints_after_verification() {
        let messages = [
            (2, b"".as_slice()),
            (7, b"Subject: seven\r\n\r\nseven".as_slice()),
            (9, b"Subject: nine\r\n\r\nnine".as_slice()),
        ];
        let server = MockImapServer::new()
            .greeting("* OK synthetic Yahoo-like IMAP ready\r\n")
            .respond("LOGIN", "{TAG} OK LOGIN completed\r\n")
            .respond(
                "CAPABILITY",
                "* CAPABILITY IMAP4rev1 ENABLE UIDONLY PARTIAL MESSAGELIMIT=2\r\n{TAG} OK CAPABILITY completed\r\n",
            )
            .respond(
                "ENABLE UIDONLY",
                "* ENABLED UIDONLY\r\n{TAG} OK ENABLE completed\r\n",
            )
            .respond("EXAMINE", examine_response("Synthetic", 3, 77, 10))
            .respond("UID FETCH 1:9", inventory_response(&[(2, 999), (7, 1)]))
            .respond("UID FETCH 8:9", inventory_response(&[(9, 0)]))
            // The reported sizes are deliberately advisory and wrong. The
            // literal byte count is the acquisition/accounting authority.
            .respond("UID FETCH 2 (", exact_response(2, 1, messages[0].1))
            .respond("UID FETCH 7 (", exact_response(7, 2, messages[1].1))
            .respond("UID FETCH 9 (", exact_response(9, 3, messages[2].1))
            .start()
        .await;
        let mut transport = connect_fake_transport(&server).await;
        let mut archive = FakeArchive::default();
        let report = run_acquisition(
            &mut transport,
            &mut archive,
            "Synthetic",
            Some(77),
            limits(),
            CancellationToken::new(),
            |_| Ok(()),
        )
        .await
        .expect("complete fixed snapshot");

        assert_eq!(report.checkpoint, Some(9));
        assert_eq!((report.inventoried, report.archived), (3, 3));
        assert_eq!(archive.projected, [2, 7, 9]);

        let commands = server.commands();
        let commands = commands.join("\n");
        assert_eq!(commands.matches(" EXAMINE ").count(), 2);
        assert!(commands.contains(" UID FETCH 1:9 (UID RFC822.SIZE) (PARTIAL 1:2)"));
        assert!(commands.contains(" UID FETCH 8:9 (UID RFC822.SIZE) (PARTIAL 1:2)"));
        assert_eq!(commands.matches("BODY.PEEK[]").count(), 3);
        let commands = commands.to_ascii_uppercase();
        assert!(
            ![" STORE ", " MOVE ", " COPY ", " DELETE ", " EXPUNGE", " CLOSE",]
                .iter()
                .any(|forbidden| commands.contains(forbidden))
        );
    }
}
