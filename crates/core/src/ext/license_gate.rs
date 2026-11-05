//
// Copyright (c) 2025-2026 rustmailer.com (https://rustmailer.com)
//
// This file is part of the Bichon Email Archiving Project
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as
// published by the Free Software Foundation, either version 3 of the
// License, or (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <http://www.gnu.org/licenses/>.
//
// Commercial-edition gates that the *core* must apply inside the operations
// they protect, not only at the REST edge:
//
// * the account limit (every account-creation path — REST, journal, future
//   CLI/MCP — must be checked under the same lock as the insert);
// * whether syncing is allowed at all (e.g. a restore brought in more
//   accounts than the license allows).
//
// Community edition registers nothing: both gates are permissive. Pro /
// Enterprise register live callbacks in `bichon_pro::initialize()`, because
// the licence status is populated after that function runs and can change
// while the process keeps running.

use crate::error::code::ErrorCode;
use crate::error::BichonResult;
use crate::raise_error;
use std::sync::{LazyLock, Mutex, RwLock};

type AccountLimitCheck = fn() -> Result<(), String>;
type SyncAllowedCheck = fn() -> bool;

static ACCOUNT_LIMIT_CHECK: LazyLock<RwLock<Option<AccountLimitCheck>>> =
    LazyLock::new(|| RwLock::new(None));
static SYNC_ALLOWED_CHECK: LazyLock<RwLock<Option<SyncAllowedCheck>>> =
    LazyLock::new(|| RwLock::new(None));

/// Serializes "check the account limit" with "insert the account", so two
/// concurrent creations cannot both pass a check that only one of them
/// should have passed. Process-wide by design: every creation path must take
/// this lock.
static ACCOUNT_CREATE_LOCK: Mutex<()> = Mutex::new(());

/// Called by Pro/Enterprise at startup with a live account-limit check.
pub fn set_account_limit_check(check: AccountLimitCheck) {
    *ACCOUNT_LIMIT_CHECK.write().unwrap() = Some(check);
}

/// Called by Pro/Enterprise at startup with a live sync-allowed check.
pub fn set_sync_allowed_check(check: SyncAllowedCheck) {
    *SYNC_ALLOWED_CHECK.write().unwrap() = Some(check);
}

/// Run `insert` while holding the account-creation lock, after the registered
/// limit check has passed. This is the **only** correct way to create an
/// account in a build that can be license-limited: callers that insert
/// directly bypass both the lock and the check.
pub fn insert_account_checked<T>(insert: impl FnOnce() -> BichonResult<T>) -> BichonResult<T> {
    let _guard = ACCOUNT_CREATE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    if let Some(check) = *ACCOUNT_LIMIT_CHECK.read().unwrap() {
        check().map_err(|message| raise_error!(message, ErrorCode::Forbidden))?;
    }

    insert()
}

/// Whether background syncing may run right now. Community builds and any
/// build that has not registered a check are always allowed.
pub fn sync_allowed() -> bool {
    SYNC_ALLOWED_CHECK
        .read()
        .unwrap()
        .map(|check| check())
        .unwrap_or(true)
}

/// Error returned when a sync is refused by the gate.
pub fn sync_refused_error() -> crate::error::BichonError {
    raise_error!(
        "Syncing is paused: this deployment holds more accounts than its license allows. \
         Remove accounts or install a license with a higher limit."
            .to_string(),
        ErrorCode::Forbidden
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_runs_inside_the_lock_and_refuses_when_the_check_fails() {
        set_account_limit_check(|| Err("limit reached".into()));
        let err = insert_account_checked(|| Ok(1u8)).expect_err("must refuse");
        assert!(format!("{err:?}").contains("limit reached"));

        set_account_limit_check(|| Ok(()));
        assert_eq!(insert_account_checked(|| Ok(1u8)).unwrap(), 1);
        set_account_limit_check(|| Ok(()));
    }

    #[test]
    fn sync_defaults_to_allowed() {
        assert!(sync_allowed());
    }
}
