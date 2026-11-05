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

use std::str::FromStr;

use chrono::{DateTime, Local, TimeZone, Utc};
use cron::Schedule;

use crate::{
    utc_now,
    {
        account::{
            migration::AccountModel,
            state::{DownloadState, TriggerType},
        },
        error::BichonResult,
    },
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DownloadTask {
    FullFetch,
    TraceFetch,
    Idle,
}

pub async fn decide_next_download_task(
    account: &AccountModel,
    trigger_type: TriggerType,
) -> BichonResult<DownloadTask> {
    let state = match DownloadState::get(account.id)? {
        None => {
            DownloadState::init(account.id).await?;
            return Ok(DownloadTask::FullFetch);
        }
        Some(s) => s,
    };

    let should_start = match trigger_type {
        TriggerType::Manual => true,
        TriggerType::SyncFull => true,
        TriggerType::Scheduled => {
            let now = utc_now!();
            if !scheduled_ready(now, state.last_finished_at, state.retry_after) {
                false
            } else if let Some(ref schedule) = account.download_schedule {
                should_trigger_scheduled(schedule, state.last_trigger_at)
            } else {
                should_trigger_next_download(
                    state.last_trigger_at,
                    account.download_interval_min.unwrap_or(60),
                )
            }
        }
    };

    if should_start {
        DownloadState::start_new_session(account.id, trigger_type)?;
        Ok(DownloadTask::TraceFetch)
    } else {
        // Nothing to download right now. A Running active_session at this point
        // is a leftover from an interrupted run (a real download would have
        // been blocked by the busy guard before reaching here), so mark it
        // Cancelled instead of leaving the UI showing a phantom "syncing".
        DownloadState::finalize_stale_session(account.id)?;
        Ok(DownloadTask::Idle)
    }
}
/// Whether a scheduled round may start: not within a minute of the previous
/// one, and past any backoff a throttled server forced on us (#396).
///
/// The backoff lives here rather than only in the interval comparison so a
/// short interval *or* a cron schedule cannot re-hammer a server that is
/// already refusing this account.
fn scheduled_ready(now: i64, last_finished_at: Option<i64>, retry_after: Option<i64>) -> bool {
    let cooldown_ok = now - last_finished_at.unwrap_or(0) > 60 * 1000;
    let backoff_over = retry_after.is_none_or(|t| now >= t);
    cooldown_ok && backoff_over
}

fn should_trigger_next_download(last_trigger_at: i64, sync_interval_min: i64) -> bool {
    let now = utc_now!();
    now - last_trigger_at > (sync_interval_min * 60 * 1000)
}

fn should_trigger_scheduled(schedule_str: &str, last_trigger_at: i64) -> bool {
    let schedule = match Schedule::from_str(schedule_str) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(
                "Invalid cron expression '{}', falling back to no trigger: {}",
                schedule_str,
                e
            );
            return false;
        }
    };
    // last_trigger_at is a UTC millis timestamp; convert to server local time
    let last_utc = match Utc.timestamp_millis_opt(last_trigger_at) {
        chrono::LocalResult::Single(dt) => dt,
        _ => {
            tracing::warn!("Invalid last_trigger_at timestamp: {}", last_trigger_at);
            return false;
        }
    };
    let last_dt: DateTime<Local> = last_utc.with_timezone(&Local);
    let now = Local::now();
    schedule
        .after(&last_dt)
        .next()
        .map_or(false, |next| next <= now)
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn cron_every_minute_triggers_after_60s() {
        // "0 * * * * *" = every minute at second 0. last_trigger 90s ago → should trigger
        let now = Local::now();
        let last_trigger = now.timestamp_millis() - 90_000;
        assert!(should_trigger_scheduled("0 * * * * *", last_trigger));
    }

    #[test]
    fn cron_daily_midnight_triggers_when_missed() {
        // "0 0 0 * * *" = daily at midnight
        // last_trigger was 25 hours ago → should trigger (we missed midnight)
        let now = Local::now();
        let last_trigger = now.timestamp_millis() - 25 * 60 * 60 * 1000;
        assert!(should_trigger_scheduled("0 0 0 * * *", last_trigger));
    }

    #[test]
    fn cron_daily_midnight_no_trigger_if_already_fired() {
        // "0 0 0 * * *" = daily at midnight
        // last_trigger was 1 minute ago → should NOT trigger
        let now = Local::now();
        let last_trigger = now.timestamp_millis() - 60_000;
        assert!(!should_trigger_scheduled("0 0 0 * * *", last_trigger));
    }

    #[test]
    fn invalid_cron_returns_false() {
        assert!(!should_trigger_scheduled("invalid cron expression", 0));
    }

    #[test]
    fn cron_every_hour_triggers() {
        // "0 0 * * * *" = every hour at minute 0, second 0
        // last_trigger was 61 minutes ago → should trigger
        let now = Local::now();
        let last_trigger = now.timestamp_millis() - 61 * 60 * 1000;
        assert!(should_trigger_scheduled("0 0 * * * *", last_trigger));
    }

    // ── Throttle backoff gate (#396) ───────────────────────────────

    #[test]
    fn scheduled_round_starts_when_nothing_blocks_it() {
        assert!(super::scheduled_ready(1_000_000, Some(1_000), None));
        // Exactly at/after the backoff deadline is allowed.
        assert!(super::scheduled_ready(1_000_000, Some(1_000), Some(1_000_000)));
        assert!(super::scheduled_ready(1_000_000, Some(1_000), Some(999_999)));
    }

    #[test]
    fn scheduled_round_waits_out_the_throttle_backoff() {
        // retry_after is in the future: a short interval or cron schedule
        // must not start the round anyway.
        assert!(!super::scheduled_ready(1_000_000, Some(1_000), Some(1_000_001)));
        assert!(!super::scheduled_ready(1_000_000, Some(1_000), Some(3_600_000)));
    }

    #[test]
    fn scheduled_round_keeps_the_one_minute_cooldown() {
        assert!(!super::scheduled_ready(1_000_000, Some(1_000_000), None));
        assert!(!super::scheduled_ready(1_000_000, Some(999_500), None));
        assert!(super::scheduled_ready(1_000_000, Some(939_000), None));
    }
}
