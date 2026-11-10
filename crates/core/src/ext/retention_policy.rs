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
// Extension points for retention-policy changes.
//
// The retention fields live in the community data model, but two things above
// it are commercial:
//
// * the minimum-retention lock is an Enterprise compliance feature, so only an
//   Enterprise licence may set it;
// * dual control is Enterprise, and a policy change that **weakens**
//   protection (shortens the maximum age, or removes/shortens the lock) is the
//   remaining way to purge data outside the `message.delete` approval.
//
// Community registers nothing: changes execute directly, and the minimum lock
// is refused. Pro/Enterprise register live callbacks in `initialize()`.

use std::sync::{LazyLock, RwLock};

use crate::error::BichonResult;

/// The two retention values of one account. `None` means disabled.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RetentionValues {
    pub retention_days: Option<u64>,
    pub min_retention_days: Option<u64>,
}

/// A proposed policy change, with the values before and after.
#[derive(Clone, Copy, Debug)]
pub struct RetentionChange {
    pub account_id: u64,
    pub before: RetentionValues,
    pub after: RetentionValues,
}

impl RetentionChange {
    /// Whether the change reduces protection: data can now be purged sooner,
    /// or a lock that prevented deletion is removed or shortened.
    ///
    /// Strengthening changes (longer retention, a new/longer lock) are
    /// deliberately not gated: requiring a second signature to keep data
    /// longer trains people to click through approvals without reading them.
    pub fn is_weakening(self) -> bool {
        let max_weakens = match (self.before.retention_days, self.after.retention_days) {
            // Disabled means "never purge". Enabling it starts the clock.
            (None, Some(_)) => true,
            (Some(_), None) => false,
            (Some(a), Some(b)) => b < a,
            (None, None) => false,
        };
        let lock_weakens = match (
            self.before.min_retention_days,
            self.after.min_retention_days,
        ) {
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (Some(a), Some(b)) => b < a,
            (None, None) => false,
        };
        max_weakens || lock_weakens
    }
}

/// What the policy gate decided.
#[derive(Clone, Debug)]
pub enum RetentionGateDecision {
    /// Apply the change now.
    Execute,
    /// A request was queued; the change is NOT applied yet.
    Queued {
        request_id: String,
        expires_at: Option<i64>,
    },
}

type MinRetentionCheck = fn() -> bool;
type ChangeGate = fn(&RetentionChange, u64, &str, bool) -> BichonResult<RetentionGateDecision>;

static MIN_RETENTION_CHECK: LazyLock<RwLock<Option<MinRetentionCheck>>> =
    LazyLock::new(|| RwLock::new(None));
static CHANGE_GATE: LazyLock<RwLock<Option<ChangeGate>>> = LazyLock::new(|| RwLock::new(None));

/// Called by Pro/Enterprise at startup with a live Enterprise check.
pub fn set_min_retention_allowed(check: MinRetentionCheck) {
    *MIN_RETENTION_CHECK.write().unwrap() = Some(check);
}

/// Whether the minimum-retention lock may be set right now. Community builds
/// and any build that registered no check refuse it.
pub fn min_retention_allowed() -> bool {
    MIN_RETENTION_CHECK
        .read()
        .unwrap()
        .map(|check| check())
        .unwrap_or(false)
}

/// Called by Pro/Enterprise at startup with the dual-control gate.
pub fn set_retention_change_gate(gate: ChangeGate) {
    *CHANGE_GATE.write().unwrap() = Some(gate);
}

/// Ask the registered gate what to do with a **weakening** change. A build
/// with no gate executes it (community behaviour).
pub fn gate_weakening_change(
    change: &RetentionChange,
    actor_id: u64,
    actor_name: &str,
    is_admin: bool,
) -> BichonResult<RetentionGateDecision> {
    match *CHANGE_GATE.read().unwrap() {
        Some(gate) => gate(change, actor_id, actor_name, is_admin),
        None => Ok(RetentionGateDecision::Execute),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(
        before_max: Option<u64>,
        after_max: Option<u64>,
        before_min: Option<u64>,
        after_min: Option<u64>,
    ) -> RetentionChange {
        RetentionChange {
            account_id: 1,
            before: RetentionValues {
                retention_days: before_max,
                min_retention_days: before_min,
            },
            after: RetentionValues {
                retention_days: after_max,
                min_retention_days: after_min,
            },
        }
    }

    #[test]
    fn shortening_either_window_is_weakening() {
        assert!(change(Some(30), Some(10), None, None).is_weakening());
        assert!(change(None, Some(10), None, None).is_weakening());
        assert!(change(None, None, Some(365), Some(30)).is_weakening());
        assert!(change(None, None, Some(365), None).is_weakening());
    }

    #[test]
    fn lengthening_or_adding_protection_is_not_weakening() {
        assert!(!change(Some(10), Some(30), None, None).is_weakening());
        assert!(!change(Some(10), None, None, None).is_weakening());
        assert!(!change(None, None, None, Some(365)).is_weakening());
        assert!(!change(None, None, Some(30), Some(365)).is_weakening());
        assert!(!change(Some(30), Some(30), Some(365), Some(365)).is_weakening());
    }
}
