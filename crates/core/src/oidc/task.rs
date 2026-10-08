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

use crate::{
    common::periodic::{PeriodicTask, TaskHandle},
    context::BichonTask,
    oidc::{handoff::OidcHandoffEntity, pending::OidcPendingEntity},
};
use std::time::Duration;

const TASK_INTERVAL: Duration = Duration::from_secs(15 * 60);

/// Periodic cleanup of expired OIDC pending-state and handoff entries.
///
/// Pending entries live for 10 minutes and handoff entries for 60 seconds,
/// so a 15-minute cleanup interval bounds the amount of stale state kept
/// in memdb even under an unfinished-login flood.
pub struct OidcCleanTask;

impl BichonTask for OidcCleanTask {
    fn start() -> TaskHandle {
        let periodic_task = PeriodicTask::new("oidc-cleanup");

        let task = move |_: Option<u64>| {
            Box::pin(async move {
                OidcPendingEntity::clean()?;
                OidcHandoffEntity::clean()?;
                Ok(())
            })
        };

        periodic_task.start(task, None, TASK_INTERVAL, false, false)
    }
}
