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

//! IMAP IDLE supervisor.
//!
//! When the server announces the `IDLE` capability (RFC 2177), bichon
//! can open a long-lived connection per "active" mailbox, send the IDLE
//! command, and stay parked until the server pushes an EXISTS / EXPUNGE
//! / FETCH notification. On notification, an incremental account-level
//! sync is triggered immediately — eliminating the 10-min polling lag
//! between an IMAP write (Berger move, manual flag, new mail) and the
//! corresponding bichon view update.
//!
//! Design notes:
//!
//! - One **dedicated** IMAP connection per `(account, mailbox)`. IDLE
//!   holds the connection open, so we cannot share with the bb8 pool.
//! - **Periodic restart** every 29 minutes (`IDLE_RECYCLE_INTERVAL`)
//!   per RFC 2177 §3 — many intermediaries silently drop idle sockets
//!   after 30 min.
//! - **Capability gate**: nothing happens unless `account.capabilities`
//!   contains `IDLE` and `account.idle_mailboxes` is a non-empty list.
//!   Behaviour is therefore strictly opt-in and backwards-compatible.
//! - **Single-flight sync**: notifications coalesce — multiple events
//!   on the same mailbox within a debounce window only trigger a single
//!   `process_imap_download` call.
//! - The classic 10-second `AccountDownTask` polling loop is left
//!   untouched as a safety net: if every idle watcher crashes the
//!   account still catches up eventually.

use crate::account::migration::AccountModel;
use crate::account::state::TriggerType;
use crate::cache::imap::download::process_imap_download;
use crate::error::BichonResult;
use crate::imap::executor::ImapExecutor;
use dashmap::DashMap;
use std::sync::LazyLock;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

/// Max time we stay parked in a single IDLE command. Servers and middle
/// boxes (corporate proxies, TLS terminators) frequently drop idle TCP
/// connections after 30 minutes; the RFC recommends a ≤29 min cycle.
const IDLE_RECYCLE_INTERVAL: Duration = Duration::from_secs(29 * 60);

/// After a notification we wait this long before triggering a sync so a
/// burst of EXISTS/EXPUNGE responses for the same mailbox collapses into
/// a single account-level download cycle.
const NOTIFY_DEBOUNCE: Duration = Duration::from_millis(750);

/// IMAP capability string we look for on the account.
const IDLE_CAPABILITY: &str = "IDLE";

/// Singleton supervisor — mirrors the design of `SYNC_TASKS` so callers
/// can `IDLE_SUPERVISOR.start_account(...)` / `.stop(...)` from the same
/// places as the existing polling task.
pub static IDLE_SUPERVISOR: LazyLock<IdleSupervisor> = LazyLock::new(IdleSupervisor::new);

#[derive(Default)]
pub struct IdleSupervisor {
    /// Keyed by `(account_id, mailbox_name)`. Each entry owns its
    /// connection + cancellation token.
    watchers: DashMap<(u64, String), WatcherEntry>,
    /// Guards add/remove so a concurrent `restart_account` can't race
    /// with itself.
    mutate_lock: Mutex<()>,
}

struct WatcherEntry {
    handle: JoinHandle<()>,
    cancel: CancellationToken,
}

impl IdleSupervisor {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start (or restart) all idle watchers for the given account. The
    /// list of mailboxes to watch is `account.idle_mailboxes`. Mailboxes
    /// already being watched are left running; new ones get a watcher;
    /// stale ones are stopped.
    pub async fn start_account(&self, account: AccountModel) {
        let _g = self.mutate_lock.lock().await;

        let mailboxes = match account.idle_mailboxes.clone() {
            Some(list) if !list.is_empty() => list,
            _ => {
                debug!(
                    account_id = account.id,
                    "idle: no mailboxes configured; nothing to do"
                );
                return;
            }
        };

        let supports = account
            .capabilities
            .as_ref()
            .map(|caps| {
                caps.iter()
                    .any(|c| c.eq_ignore_ascii_case(IDLE_CAPABILITY))
            })
            .unwrap_or(false);
        if !supports {
            warn!(
                account_id = account.id,
                "idle: server does not advertise the IDLE capability — skipping"
            );
            return;
        }

        // Stop any watchers whose mailbox is no longer in the wanted list.
        let wanted: std::collections::HashSet<String> = mailboxes.iter().cloned().collect();
        let to_drop: Vec<(u64, String)> = self
            .watchers
            .iter()
            .filter(|kv| kv.key().0 == account.id && !wanted.contains(&kv.key().1))
            .map(|kv| kv.key().clone())
            .collect();
        for key in to_drop {
            self.stop_one(&key).await;
        }

        // Start any missing watchers.
        for mailbox in mailboxes {
            let key = (account.id, mailbox.clone());
            if self.watchers.contains_key(&key) {
                continue;
            }
            let cancel = CancellationToken::new();
            let account_clone = account.clone();
            let token = cancel.clone();
            let mailbox_clone = mailbox.clone();
            let handle = tokio::spawn(async move {
                watch_mailbox(account_clone, mailbox_clone, token).await
            });
            self.watchers.insert(key, WatcherEntry { handle, cancel });
            info!(
                account_id = account.id,
                mailbox = mailbox.as_str(),
                "idle: watcher started"
            );
        }
    }

    /// Stop every watcher for an account. Idempotent — useful on account
    /// disable, delete, or before applying a new config.
    pub async fn stop_account(&self, account_id: u64) {
        let _g = self.mutate_lock.lock().await;
        let keys: Vec<(u64, String)> = self
            .watchers
            .iter()
            .filter(|kv| kv.key().0 == account_id)
            .map(|kv| kv.key().clone())
            .collect();
        for key in keys {
            self.stop_one(&key).await;
        }
    }

    async fn stop_one(&self, key: &(u64, String)) {
        if let Some((_, entry)) = self.watchers.remove(key) {
            entry.cancel.cancel();
            // Best-effort: give the task a moment to finish cleanly.
            let _ = tokio::time::timeout(Duration::from_secs(5), entry.handle).await;
            info!(
                account_id = key.0,
                mailbox = key.1.as_str(),
                "idle: watcher stopped"
            );
        }
    }
}

/// Long-lived task that owns one IMAP connection and stays in IDLE on a
/// single mailbox, re-syncing the account whenever the server pushes
/// something.
async fn watch_mailbox(account: AccountModel, mailbox: String, cancel: CancellationToken) {
    let account_id = account.id;
    let mut backoff_secs: u64 = 1;
    loop {
        if cancel.is_cancelled() {
            return;
        }
        match run_idle_cycle(&account, &mailbox, cancel.clone()).await {
            Ok(()) => {
                // Clean exit (cancelled or graceful recycle) — reset backoff.
                backoff_secs = 1;
            }
            Err(e) => {
                warn!(
                    account_id,
                    mailbox = mailbox.as_str(),
                    error = ?e,
                    "idle: cycle failed; retry after backoff"
                );
                // Exponential backoff capped at 5 min; CancellationToken-aware sleep.
                let sleep = Duration::from_secs(backoff_secs);
                backoff_secs = (backoff_secs * 2).min(300);
                tokio::select! {
                    _ = tokio::time::sleep(sleep) => {}
                    _ = cancel.cancelled() => return,
                }
            }
        }
    }
}

/// One open → IDLE → DONE cycle. Returns Ok when cancelled or when the
/// recycle deadline fires (the outer loop will reconnect immediately).
async fn run_idle_cycle(
    account: &AccountModel,
    mailbox: &str,
    cancel: CancellationToken,
) -> BichonResult<()> {
    let account_id = account.id;

    // 1. Open a dedicated connection (NOT from the bb8 pool — IDLE holds
    //    the socket for tens of minutes, which would starve the pool).
    let mut session = ImapExecutor::create_connection(account_id).await?;

    // 2. Pin the target mailbox in EXAMINE mode (read-only — IDLE only
    //    needs to *observe* changes).
    if let Err(e) = session.examine(mailbox).await {
        // Disconnect cleanly before bubbling.
        let _ = session.logout().await;
        return Err(crate::raise_error!(
            format!("idle: examine({}) failed: {:#?}", mailbox, e),
            crate::error::code::ErrorCode::ImapCommandFailed
        ));
    }

    // 3. IDLE.
    let mut idle = session.idle();
    if let Err(e) = idle.init().await {
        return Err(crate::raise_error!(
            format!("idle: init failed: {:#?}", e),
            crate::error::code::ErrorCode::ImapCommandFailed
        ));
    }

    let outcome = tokio::select! {
        biased;
        _ = cancel.cancelled() => IdleOutcome::Cancelled,
        ev = wait_for_event(&mut idle) => ev,
    };

    // 4. Always end the IDLE cleanly so the server doesn't hang.
    let mut session_after = match idle.done().await {
        Ok(s) => s,
        Err(e) => {
            warn!(account_id, mailbox, error = ?e, "idle: DONE failed");
            return Ok(()); // outer loop will reconnect
        }
    };
    let _ = session_after.logout().await;

    // 5. If the server told us something happened, trigger a sync. We
    //    debounce in case multiple notifications arrived back-to-back.
    if matches!(outcome, IdleOutcome::Notified) {
        tokio::time::sleep(NOTIFY_DEBOUNCE).await;
        let token = cancel.clone();
        if let Err(e) =
            process_imap_download(account, token, TriggerType::Idle).await
        {
            warn!(
                account_id,
                mailbox,
                error = ?e,
                "idle: triggered sync failed (will retry on next event)"
            );
        }
    }

    Ok(())
}

enum IdleOutcome {
    Notified,
    Recycle,
    Cancelled,
}

/// Wait for any IMAP IDLE notification, recycling on the configured deadline.
///
/// The Handle's stream type is the same `Box<dyn SessionStream>` produced by
/// `ImapExecutor::create_connection`, so we constrain on it directly rather
/// than introducing a generic — that avoids fighting the fork's trait
/// boundaries (the rustmailer/async-imap fork plugs into `tokio::io::*`).
async fn wait_for_event(
    idle: &mut async_imap::extensions::idle::Handle<Box<dyn crate::imap::session::SessionStream>>,
) -> IdleOutcome {
    let (wait, _stop) = idle.wait_with_timeout(IDLE_RECYCLE_INTERVAL);
    match wait.await {
        Ok(async_imap::extensions::idle::IdleResponse::NewData(_)) => IdleOutcome::Notified,
        Ok(async_imap::extensions::idle::IdleResponse::ManualInterrupt) => IdleOutcome::Cancelled,
        Ok(_) | Err(_) => IdleOutcome::Recycle,
    }
}
