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
//
// SMTP journaling extension point.
//
// Community edition: NoopHandler — journaling is not available and the SMTP
// server behaves exactly as it did before this module existed.
// Enterprise: an implementation that accepts deliveries from allow-listed
// peers into a dedicated journal account.
//
// **Why an extension point rather than a community feature.** Journaling is a
// compliance capability: it decides what mail is *provably complete*, and it
// must be configured by a compliance administrator, audited, and kept separate
// from the IMAP-synced mailboxes. All of that belongs to the Enterprise
// layer; the community SMTP server only needs to ask three questions (is this
// peer allowed / is this the journal target / archive these bytes) and must
// keep its old behaviour byte for byte when the answers are "no".
//
// **Why the raw bytes and not a parsed message.** The handler has to be able to
// store exactly what the tenant delivered — the caller's body is the only
// artifact that can support that claim. Parsing happens inside the handler,
// where the format differences between providers (an Exchange journal report
// wraps the original, Gmail's routing rule does not) can be dealt with in one
// place.

use std::future::Future;
use std::net::IpAddr;
use std::pin::Pin;
use std::sync::{Arc, LazyLock, RwLock};

use crate::error::{code::ErrorCode, BichonResult};
use crate::raise_error;

/// What archiving one journal delivery did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JournalIngest {
    /// The account the delivery was filed into.
    pub account_id: u64,
    /// True when the same content was already archived (an SMTP retry, or the
    /// tenant delivering the same report twice). The delivery is still
    /// acknowledged with 250: from the sender's point of view it succeeded,
    /// and re-requesting it would loop forever.
    pub duplicate: bool,
}

/// The Enterprise implementation of journal-based ingestion.
pub trait JournalHandler: Send + Sync {
    /// Whether journaling is configured. False means the SMTP server must not
    /// consult any other method here — in particular it must not start
    /// rejecting peers that it accepts today.
    fn enabled(&self) -> bool;

    /// Whether an **unauthenticated** peer may deliver.
    ///
    /// `tls` is the session's encryption state at the moment of the check;
    /// `ip` is the peer address, or `None` when the transport could not report
    /// one (which must be treated as "not allowed", never as "trusted
    /// local"). Implementations are expected to require TLS and an allow-list
    /// match; this is the only gate in front of a write into the archive.
    fn allow_peer(&self, ip: Option<IpAddr>, tls: bool) -> bool;

    /// Whether `addr` is the configured journal target address.
    ///
    /// Matched case-insensitively by the caller's contract; the implementation
    /// decides what "the target" means (one address).
    fn is_target(&self, addr: &str) -> bool;

    /// Archive one delivery. `raw` is the message exactly as received, after
    /// SMTP dot-unstuffing — the bytes the tenant sent.
    fn ingest<'a>(
        &'a self,
        raw: &'a [u8],
    ) -> Pin<Box<dyn Future<Output = BichonResult<JournalIngest>> + Send + 'a>>;
}

/// Default — journaling is unavailable.
struct NoopHandler;

impl JournalHandler for NoopHandler {
    fn enabled(&self) -> bool {
        false
    }

    fn allow_peer(&self, _ip: Option<IpAddr>, _tls: bool) -> bool {
        false
    }

    fn is_target(&self, _addr: &str) -> bool {
        false
    }

    fn ingest<'a>(
        &'a self,
        _raw: &'a [u8],
    ) -> Pin<Box<dyn Future<Output = BichonResult<JournalIngest>> + Send + 'a>> {
        Box::pin(async {
            Err(raise_error!(
                "SMTP journaling is not available in this edition".to_string(),
                ErrorCode::InternalError
            ))
        })
    }
}

static HANDLER: LazyLock<RwLock<Arc<dyn JournalHandler>>> =
    LazyLock::new(|| RwLock::new(Arc::new(NoopHandler)));

/// Called by the commercial server at startup to replace the noop default.
///
/// An `Arc` rather than a boxed reference because [`ingest`] must keep the
/// handler alive across an `await` without holding the lock — a read guard is
/// not `Send`, and the SMTP session is spawned onto the runtime.
pub fn set_handler(handler: Arc<dyn JournalHandler>) {
    *HANDLER.write().unwrap() = handler;
}

/// A snapshot of the current handler. Cheap (one `Arc` clone) and safe to hold
/// across awaits.
fn handler() -> Arc<dyn JournalHandler> {
    HANDLER.read().unwrap().clone()
}

/// Whether journaling is configured at all.
pub fn enabled() -> bool {
    handler().enabled()
}

/// Whether an unauthenticated peer may deliver.
pub fn allow_peer(ip: Option<IpAddr>, tls: bool) -> bool {
    handler().allow_peer(ip, tls)
}

/// Whether `addr` is the configured journal target.
pub fn is_target(addr: &str) -> bool {
    handler().is_target(addr)
}

/// Archive one delivery. See [`JournalHandler::ingest`].
pub async fn ingest(raw: &[u8]) -> BichonResult<JournalIngest> {
    // The `Arc` is moved into the future, so the lock is not held across the
    // await and the returned future stays `Send`.
    let handler = handler();
    handler.ingest(raw).await
}
