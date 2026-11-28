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

//! Provider-connector extension point.
//!
//! The community build knows the *shape* of Graph/Gmail accounts (the type
//! and `connection_id` are part of the stored model so rows stay readable and
//! backups portable) but ships no connector. Pro/Enterprise register an
//! implementation at startup:
//!
//! * account creation asks [`is_available`] before accepting a provider
//!   account, so a community build cannot create an account it can never sync;
//! * the executor startup asks [`try_start_account_sync`] for every enabled
//!   provider account, after the IMAP initialisation has run untouched;
//! * account deletion asks [`stop_account_sync`] and
//!   [`delete_account_state`] so a connector can release its cursors, mappings
//!   and credentials.
//!
//! The trait is deliberately synchronous and cheap: implementations spawn
//! their own background work. This keeps the IMAP startup path (which is
//! fully async) structurally unchanged.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, RwLock};

use crate::account::migration::{Account, ProviderKind};
use crate::error::code::ErrorCode;
use crate::error::BichonResult;
use crate::raise_error;

/// One provider integration (Microsoft Graph or Gmail API).
pub trait ProviderConnector: Send + Sync {
    /// Which provider this connector implements. Registration uses it as the
    /// key; a mismatch between `kind()` and the account is a programming error.
    fn kind(&self) -> ProviderKind;

    /// Start (or resume) background syncing for one account.
    ///
    /// Must return promptly: spawn the actual work and return. Called once per
    /// enabled provider account during startup, and by the REST layer when an
    /// account is enabled or created.
    fn start_account_sync(&self, account: &Account) -> BichonResult<()>;

    /// Stop scheduling and, best effort, cancel in-flight work for an account.
    /// Called when the account is disabled or deleted. Default: no-op.
    fn stop_account_sync(&self, _account_id: u64) -> BichonResult<()> {
        Ok(())
    }

    /// Delete provider-side state owned by an account: cursors, provider
    /// message mappings, error ledger entries. Called from the account
    /// cleanup path. Default: no-op.
    fn delete_account_state(&self, _account_id: u64) -> BichonResult<()> {
        Ok(())
    }
}

static CONNECTORS: LazyLock<RwLock<HashMap<ProviderKind, Arc<dyn ProviderConnector>>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// Register (or replace) the connector for its provider kind. Called once by
/// Pro/Enterprise during startup; community builds register nothing.
pub fn register_connector(connector: Arc<dyn ProviderConnector>) {
    let kind = connector.kind();
    CONNECTORS
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(kind, connector);
}

/// Whether this build can sync accounts of `kind`. Community: always `false`.
pub fn is_available(kind: ProviderKind) -> bool {
    CONNECTORS
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .contains_key(&kind)
}

fn connector_for(kind: ProviderKind) -> Option<Arc<dyn ProviderConnector>> {
    CONNECTORS
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(&kind)
        .cloned()
}

/// Start syncing one provider account if a connector is registered.
///
/// Returns `Ok(false)` when no connector is available: the caller (startup)
/// logs nothing alarming, because a community build reading a Pro data
/// directory is a supported state. Returns `Ok(true)` once the connector
/// accepted the account.
pub fn try_start_account_sync(account: &Account) -> BichonResult<bool> {
    let Some(kind) = account.provider_kind() else {
        return Ok(false);
    };
    match connector_for(kind) {
        Some(connector) => {
            connector.start_account_sync(account)?;
            Ok(true)
        }
        None => Ok(false),
    }
}

/// Stop syncing one provider account. No-op when the account is not a
/// provider account or no connector is registered.
pub fn stop_account_sync(account: &Account) -> BichonResult<()> {
    let Some(kind) = account.provider_kind() else {
        return Ok(());
    };
    if let Some(connector) = connector_for(kind) {
        connector.stop_account_sync(account.id)?;
    }
    Ok(())
}

/// Delete provider-side state for an account. No-op when unavailable.
pub fn delete_account_state(account: &Account) -> BichonResult<()> {
    let Some(kind) = account.provider_kind() else {
        return Ok(());
    };
    if let Some(connector) = connector_for(kind) {
        connector.delete_account_state(account.id)?;
    }
    Ok(())
}

/// Why a message was deleted locally. Informational only (ops/audit); the
/// suppression behaviour is identical for every reason.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeletionReason {
    Manual,
    Gdpr,
    Retention,
}

impl DeletionReason {
    pub fn as_str(self) -> &'static str {
        match self {
            DeletionReason::Manual => "manual",
            DeletionReason::Gdpr => "gdpr",
            DeletionReason::Retention => "retention",
        }
    }
}

type DeletedIdentitySink =
    dyn Fn(u64, &[String], DeletionReason) -> BichonResult<()> + Send + Sync;

/// Sinks that persist the content hashes of messages deleted from accounts
/// whose source has no UID (provider accounts). Pro registers one; community
/// registers none, so every record call is a no-op.
static DELETED_IDENTITY_SINKS: LazyLock<RwLock<Vec<Box<DeletedIdentitySink>>>> =
    LazyLock::new(|| RwLock::new(Vec::new()));

/// Register a sink that remembers deleted message identities. Called once by
/// Pro/Enterprise at startup.
pub fn register_deleted_identity_sink(
    sink: impl Fn(u64, &[String], DeletionReason) -> BichonResult<()> + Send + Sync + 'static,
) {
    DELETED_IDENTITY_SINKS
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push(Box::new(sink));
}

/// Record the content hashes of messages that are about to be deleted from an
/// account whose remote source has no UID (Graph/Gmail).
///
/// Must be called *before* the messages are removed. A failure aborts the
/// delete: a committed deletion without a suppression record would let a
/// later re-scan resurrect the message. No-op when no sink is registered.
pub fn record_deleted_identities(
    account_id: u64,
    content_hashes: &[String],
    reason: DeletionReason,
) -> BichonResult<()> {
    if content_hashes.is_empty() {
        return Ok(());
    }
    let sinks = DELETED_IDENTITY_SINKS
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for sink in sinks.iter() {
        sink(account_id, content_hashes, reason)?;
    }
    Ok(())
}

/// Error used by account creation when a provider account is requested in a
/// build without that connector.
pub fn unavailable_error(kind: ProviderKind) -> crate::error::BichonError {
    raise_error!(
        format!("{kind:?} accounts require the Pro or Enterprise edition"),
        ErrorCode::Forbidden
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account::migration::AccountType;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct FakeConnector {
        kind: ProviderKind,
        started: AtomicBool,
    }

    impl ProviderConnector for FakeConnector {
        fn kind(&self) -> ProviderKind {
            self.kind
        }
        fn start_account_sync(&self, _account: &Account) -> BichonResult<()> {
            self.started.store(true, Ordering::SeqCst);
            Ok(())
        }
    }

    fn provider_account(kind: ProviderKind) -> Account {
        Account {
            account_type: match kind {
                ProviderKind::Graph => AccountType::Graph,
                ProviderKind::Gmail => AccountType::Gmail,
            },
            ..Default::default()
        }
    }

    #[test]
    fn registered_connector_is_started_for_provider_account() {
        let connector = Arc::new(FakeConnector {
            kind: ProviderKind::Gmail,
            started: AtomicBool::new(false),
        });
        register_connector(connector.clone());
        assert!(is_available(ProviderKind::Gmail));
        let account = provider_account(ProviderKind::Gmail);
        assert!(try_start_account_sync(&account).unwrap());
        assert!(connector.started.load(Ordering::SeqCst));
    }

    #[test]
    fn non_provider_account_is_ignored() {
        let account = Account::default();
        assert!(!try_start_account_sync(&account).unwrap());
    }

    #[test]
    fn deleted_identity_sink_receives_the_batch() {
        use std::sync::Mutex;
        static SEEN: LazyLock<Mutex<Vec<(u64, Vec<String>, &'static str)>>> =
            LazyLock::new(|| Mutex::new(Vec::new()));
        register_deleted_identity_sink(|account_id, hashes, reason| {
            SEEN.lock()
                .unwrap()
                .push((account_id, hashes.to_vec(), reason.as_str()));
            Ok(())
        });
        record_deleted_identities(
            7,
            &["hash-a".to_string(), "hash-b".to_string()],
            DeletionReason::Retention,
        )
        .unwrap();
        let seen = SEEN.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].0, 7);
        assert_eq!(seen[0].1, vec!["hash-a", "hash-b"]);
        assert_eq!(seen[0].2, "retention");
    }
}
