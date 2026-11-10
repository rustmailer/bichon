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

//! Asynchronous "delete everything matching this search" jobs.
//!
//! Deleting a large result set cannot run inside one HTTP request: the
//! synchronous delete endpoint is bounded by the 60 s request timeout and the
//! index work is proportional to the number of messages. A job therefore
//! enumerates the matching ids up front, deletes them in batches, and persists
//! its progress so the client can poll, cancel, or come back later.
//!
//! **Persistence.** The job record lives in memdb (`batch_delete_jobs`), so a
//! restart keeps the history; a job that was running at shutdown is marked
//! `Interrupted` on startup and is never resumed automatically (the exact
//! partial state is not knowable, and re-running is an explicit user action).
//!
//! **Scope.** The job keeps the *resolved* scope (accounts plus a view's
//! per-source filters) in memory for its lifetime. It is not persisted: a
//! restart interrupts the job anyway, and the resolved view filter is derived
//! state that must not be replayed from a stale snapshot.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use serde::{Deserialize, Serialize};
use tracing::{info, warn};
use uuid::Uuid;

use crate::account::access::ViewScopeGroup;
use crate::account::migration::AccountModel;
use crate::database::{delete_impl, list_all_impl, manager::DB_MANAGER, upsert_impl, MemDbModel};
use crate::error::{code::ErrorCode, BichonResult};
use crate::message::delete::{emit_delete_audit_scoped, plan_delete_audit};
use crate::message::search::EmailSearchFilter;
use crate::raise_error;
use crate::store::tantivy::envelope::ENVELOPE_MANAGER;
use crate::utc_now;

/// How many finished jobs are kept. Running jobs are never evicted.
pub const MAX_JOBS: usize = 30;
/// Messages deleted per batch. One batch is one `delete_messages_impl` call,
/// one write-gate acquisition and one persisted progress update.
pub const BATCH_SIZE: usize = 500;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Enum))]
// `serde` controls how the value is stored in memdb; `oai` controls the JSON
// the REST layer emits. Without the second, the API answered "Running" while
// the UI (and every documented example) expects "running".
#[cfg_attr(feature = "web-api", oai(rename_all = "snake_case"))]
#[serde(rename_all = "snake_case")]
pub enum BatchDeleteStatus {
    #[default]
    Running,
    /// A cancellation was requested; the worker stops between batches.
    Cancelling,
    Cancelled,
    Success,
    /// At least one batch failed; `deleted` / `failed` describe the outcome.
    Failed,
    /// The server restarted while the job was running.
    Interrupted,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Object))]
pub struct BatchDeleteJob {
    pub job_id: String,
    pub user_id: u64,
    pub username: String,
    /// Set when a second person executed the deletion on behalf of the
    /// requester (Pro dual control). The job belongs to the requester — it
    /// appears in their history — while the audit trail names the approver
    /// who actually ran it.
    #[serde(default)]
    pub executed_by: Option<String>,
    /// The approval request this job was started from, when the deletion went
    /// through dual control. Lets an auditor go from the decision to the job.
    #[serde(default)]
    pub approval_request_id: Option<String>,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub status: BatchDeleteStatus,
    /// The search filter the job was created with. Kept for display and audit;
    /// the actual id set was enumerated once at the start.
    pub filter: EmailSearchFilter,
    #[serde(default)]
    pub label: Option<String>,
    /// Resolved account scope (source accounts, after view expansion).
    #[serde(default)]
    pub account_ids: Vec<u64>,
    pub total: u64,
    pub processed: u64,
    pub deleted: u64,
    #[serde(default)]
    pub failed: u64,
    /// Messages skipped because they were still inside the account's
    /// minimum-retention window (Enterprise deletion lock).
    #[serde(default)]
    pub skipped_locked: u64,
    #[serde(default)]
    pub error: Option<String>,
    /// Persisted mirror of the in-process cancel flag, so a restarted server
    /// (which marks the job interrupted anyway) still shows what was asked.
    #[serde(default)]
    pub cancel_requested: bool,
    /// Per-account outcome. The global counters answer "how far", this
    /// answers "which account", which a caller that froze an explicit id set
    /// (Pro dual control) must report back per account.
    #[serde(default)]
    pub account_progress: Vec<BatchDeleteAccountProgress>,
}

/// Deleted / failed counts for one account in a [`BatchDeleteJob`].
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Object))]
pub struct BatchDeleteAccountProgress {
    pub account_id: u64,
    #[serde(default)]
    pub deleted: u64,
    #[serde(default)]
    pub failed: u64,
}

impl MemDbModel for BatchDeleteJob {
    fn collection() -> &'static str {
        "batch_delete_jobs"
    }
    fn key(&self) -> String {
        self.job_id.clone()
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Object))]
pub struct BatchDeletePreviewView {
    /// Messages the caller may delete, i.e. what the job would remove.
    pub total: u64,
    /// Every message matching the filter in the caller's readable scope. The
    /// difference to `total` is `skipped_no_permission`.
    #[serde(default)]
    pub matched: u64,
    /// Messages matching the filter on accounts where the caller lacks
    /// `data:delete`. They are skipped, not deleted, and are shown before the
    /// operator confirms.
    #[serde(default)]
    pub skipped_no_permission: u64,
    /// Source accounts the job will touch (delete permission held).
    pub account_ids: Vec<u64>,
    /// Source accounts whose matching messages are skipped.
    #[serde(default)]
    pub skipped_account_ids: Vec<u64>,
    /// Accounts in scope currently under a legal hold. Any of them makes the
    /// whole request fail; the UI shows them before the operator confirms.
    #[serde(default)]
    pub held_account_ids: Vec<u64>,
    /// Accounts whose minimum-retention lock is active. Matching messages
    /// younger than the lock window will be skipped by the job; the UI warns
    /// before the operator confirms.
    #[serde(default)]
    pub retention_locked_account_ids: Vec<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Object))]
pub struct BatchDeleteRequest {
    pub filter: EmailSearchFilter,
    #[serde(default)]
    pub label: Option<String>,
}

impl Default for BatchDeleteRequest {
    fn default() -> Self {
        Self {
            filter: EmailSearchFilter::default(),
            label: None,
        }
    }
}

/// The id of the single running job, if any. Serializing destructive batch
/// deletions keeps two large jobs from thrashing the write gate and the index
/// writer; a second request fails fast with a clear message instead of queuing
/// silently for an unbounded time.
static ACTIVE: LazyLock<Mutex<Option<String>>> = LazyLock::new(|| Mutex::new(None));
/// In-process cancellation flags, keyed by job id.
static CANCEL: LazyLock<Mutex<HashMap<String, Arc<AtomicBool>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn lock_active() -> std::sync::MutexGuard<'static, Option<String>> {
    ACTIVE.lock().unwrap_or_else(|e| e.into_inner())
}

fn lock_cancel() -> std::sync::MutexGuard<'static, HashMap<String, Arc<AtomicBool>>> {
    CANCEL.lock().unwrap_or_else(|e| e.into_inner())
}

/// Releases the single-running-job slot when the worker ends, including on
/// panic. Without it a panic would make the feature refuse every request until
/// the server restarts.
struct ActiveJobGuard {
    job_id: String,
}

impl Drop for ActiveJobGuard {
    fn drop(&mut self) {
        *lock_active() = None;
        lock_cancel().remove(&self.job_id);
    }
}

/// Every persisted job, newest first.
pub fn list_jobs() -> BichonResult<Vec<BatchDeleteJob>> {
    let mut jobs = list_all_impl::<BatchDeleteJob>(DB_MANAGER.db())?;
    jobs.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    Ok(jobs)
}

pub fn get_job(job_id: &str) -> BichonResult<Option<BatchDeleteJob>> {
    Ok(crate::database::find_impl::<BatchDeleteJob>(
        DB_MANAGER.db(),
        job_id,
    )?)
}

fn upsert_job(job: &BatchDeleteJob) -> BichonResult<()> {
    upsert_impl::<BatchDeleteJob>(DB_MANAGER.db(), job.clone())?;
    Ok(())
}

/// Persist a *state transition* immediately. Progress updates take the normal
/// batched-WAL path (a crash may lose the last few counters), but a job's
/// existence and its final status must survive a hard kill: losing the record
/// of a destructive action is exactly what the history is for.
fn flush_job_state() {
    DB_MANAGER.flush();
}

fn evict_old_jobs() -> BichonResult<()> {
    let mut jobs = list_jobs()?;
    if jobs.len() < MAX_JOBS {
        return Ok(());
    }
    // Oldest first; only finished jobs are evictable.
    jobs.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    let mut removed = 0usize;
    let excess = jobs.len() + 1 - MAX_JOBS;
    for job in &jobs {
        if removed >= excess {
            break;
        }
        if job.status == BatchDeleteStatus::Running
            || job.status == BatchDeleteStatus::Cancelling
        {
            continue;
        }
        delete_impl::<BatchDeleteJob>(DB_MANAGER.db(), &job.job_id)?;
        removed += 1;
    }
    if removed < excess {
        return Err(raise_error!(
            "too many batch deletion jobs are still running; wait for one to finish".to_string(),
            ErrorCode::TooManyRequest
        ));
    }
    Ok(())
}

/// Mark jobs that were running when the process stopped as interrupted.
/// Called once from `BichonContext::initialize`.
pub fn interrupt_running_jobs() -> BichonResult<()> {
    let mut changed = 0usize;
    for mut job in list_all_impl::<BatchDeleteJob>(DB_MANAGER.db())? {
        if job.status == BatchDeleteStatus::Running
            || job.status == BatchDeleteStatus::Cancelling
        {
            job.status = BatchDeleteStatus::Interrupted;
            job.finished_at = Some(utc_now!());
            job.error = Some("server restarted while the job was running".to_string());
            upsert_job(&job)?;
            changed += 1;
        }
    }
    if changed > 0 {
        flush_job_state();
        info!(changed, "marked interrupted batch deletion job(s)");
    }
    Ok(())
}

/// Resolve the accounts a job will touch. `scope` is the result of the search
/// scope expansion; for a view it lists source accounts (the view's own id
/// never appears in the index).
/// Split the readable scope into the accounts the caller may delete from and
/// the accounts they may not.
///
/// Deletion is authorized on the **source** account. A view is a read grant,
/// so holding `data:delete` on the view alone does not authorize deleting its
/// sources; those messages are reported as skipped instead of failing the
/// whole request. `None` means "every account", and is materialized here
/// because the split is per account — a caller with global read but
/// per-account delete must still be told which accounts are skipped.
pub fn split_delete_scope(
    scope: Option<Vec<ViewScopeGroup>>,
    can_delete: impl Fn(u64) -> bool,
) -> BichonResult<(Vec<ViewScopeGroup>, Vec<u64>)> {
    let groups = match scope {
        Some(groups) => groups,
        None => AccountModel::list_all()?
            .into_iter()
            .map(|account| ViewScopeGroup::unbounded(account.id))
            .collect(),
    };
    let mut allowed = Vec::new();
    let mut denied: Vec<u64> = Vec::new();
    for group in groups {
        if can_delete(group.account_id) {
            allowed.push(group);
        } else if !denied.contains(&group.account_id) {
            denied.push(group.account_id);
        }
    }
    Ok((allowed, denied))
}

pub fn resolve_account_ids(
    scope: &Option<Vec<ViewScopeGroup>>,
    filter: &EmailSearchFilter,
) -> BichonResult<Vec<u64>> {
    if let Some(groups) = scope {
        let mut ids: Vec<u64> = groups.iter().map(|g| g.account_id).collect();
        ids.sort_unstable();
        ids.dedup();
        return Ok(ids);
    }
    if let Some(ids) = &filter.account_ids {
        let mut ids: Vec<u64> = ids.iter().copied().collect();
        ids.sort_unstable();
        return Ok(ids);
    }
    Ok(AccountModel::list_all()?.into_iter().map(|a| a.id).collect())
}

/// Reject the request outright if any account in scope is under a legal hold.
/// The delete implementation checks again per batch (a hold can be placed
/// while a job runs); this early check exists so the operator sees the reason
/// before anything is queued.
pub fn check_legal_hold(account_ids: &[u64]) -> BichonResult<()> {
    let mut held = Vec::new();
    for id in account_ids {
        if let Ok(account) = AccountModel::get(*id) {
            if account.is_on_hold() {
                held.push(format!("{} ({})", account.id, account.email));
            }
        }
    }
    if held.is_empty() {
        Ok(())
    } else {
        Err(raise_error!(
            format!(
                "Batch deletion refused: account(s) under legal hold: {}",
                held.join(", ")
            ),
            ErrorCode::Forbidden
        ))
    }
}

/// Where a job's message ids come from.
enum JobSource {
    /// Enumerate the filter once, when the worker starts.
    Search {
        scope: Option<Vec<ViewScopeGroup>>,
        filter: EmailSearchFilter,
    },
    /// The ids were enumerated by the caller and must be used as-is. Pro dual
    /// control freezes the set it asked a second person to approve; searching
    /// again would delete mail that arrived after the approval was requested.
    Frozen(BTreeMap<u64, Vec<String>>),
}

/// Who a job runs for, and who started it.
///
/// The two differ under Pro dual control: the requester owns the job (only
/// they should see it in their history), but the approver is the person whose
/// action is recorded in the audit trail. A job started directly by the
/// community path has no second party and leaves both approval fields unset.
#[derive(Clone, Debug, Default)]
pub struct JobOrigin {
    pub user_id: u64,
    pub username: String,
    /// Set when someone else executed an approved request.
    pub executed_by: Option<String>,
    /// The approval request that was executed, when there was one.
    pub approval_request_id: Option<String>,
}

/// Create a job and start its worker.
pub fn start_job(
    scope: Option<Vec<ViewScopeGroup>>,
    request: BatchDeleteRequest,
    account_ids: Vec<u64>,
    user_id: u64,
    username: String,
) -> BichonResult<BatchDeleteJob> {
    if scope.is_none() && request.filter.is_empty() {
        return Err(raise_error!(
            "A batch deletion needs at least one search condition.".to_string(),
            ErrorCode::InvalidParameter
        ));
    }
    check_legal_hold(&account_ids)?;
    let filter = request.filter.clone();
    start_job_inner(
        filter.clone(),
        request.label,
        account_ids,
        JobSource::Search { scope, filter },
        JobOrigin {
            user_id,
            username,
            executed_by: None,
            approval_request_id: None,
        },
    )
}

/// Create a job from an already-frozen set of message ids.
///
/// Everything except enumeration — the single running-job slot, batching,
/// progress, cancellation, tombstones and audit — is shared with a
/// search-based job, so the approved execution cannot drift from the
/// community deletion path.
pub fn start_job_from_ids(
    by_account: BTreeMap<u64, Vec<String>>,
    filter: EmailSearchFilter,
    label: Option<String>,
    origin: JobOrigin,
) -> BichonResult<BatchDeleteJob> {
    let by_account: BTreeMap<u64, Vec<String>> = by_account
        .into_iter()
        .filter(|(_, ids)| !ids.is_empty())
        .collect();
    if by_account.is_empty() {
        return Err(raise_error!(
            "No messages specified".to_string(),
            ErrorCode::InvalidParameter
        ));
    }
    let account_ids: Vec<u64> = by_account.keys().copied().collect();
    check_legal_hold(&account_ids)?;
    start_job_inner(
        filter,
        label,
        account_ids,
        JobSource::Frozen(by_account),
        origin,
    )
}

fn start_job_inner(
    filter: EmailSearchFilter,
    label: Option<String>,
    account_ids: Vec<u64>,
    source: JobSource,
    origin: JobOrigin,
) -> BichonResult<BatchDeleteJob> {
    let job_id = Uuid::new_v4().to_string();
    {
        let mut active = lock_active();
        if let Some(running) = active.as_ref() {
            return Err(raise_error!(
                format!("A batch deletion is already running (job {running}). Wait for it to finish or cancel it first."),
                ErrorCode::TooManyRequest
            ));
        }
        *active = Some(job_id.clone());
    }
    // From here on, every error path must release the active slot.
    let created = (|| -> BichonResult<BatchDeleteJob> {
        evict_old_jobs()?;
        let now = utc_now!();
        let job = BatchDeleteJob {
            job_id: job_id.clone(),
            user_id: origin.user_id,
            username: origin.username.clone(),
            executed_by: origin.executed_by,
            approval_request_id: origin.approval_request_id,
            created_at: now,
            started_at: Some(now),
            finished_at: None,
            status: BatchDeleteStatus::Running,
            filter,
            label,
            account_ids,
            total: 0,
            processed: 0,
            deleted: 0,
            failed: 0,
            skipped_locked: 0,
            error: None,
            cancel_requested: false,
            account_progress: Vec::new(),
        };
        upsert_job(&job)?;
        flush_job_state();
        Ok(job)
    })();
    let job = match created {
        Ok(job) => job,
        Err(e) => {
            *lock_active() = None;
            return Err(e);
        }
    };

    let cancel = Arc::new(AtomicBool::new(false));
    lock_cancel().insert(job_id.clone(), cancel.clone());

    let worker_job = job.clone();
    tokio::spawn(async move {
        // Clear the in-process slot even if the worker panics: a leaked slot
        // would refuse every future batch deletion until a restart.
        let _slot = ActiveJobGuard {
            job_id: job_id.clone(),
        };
        let outcome = run_job(worker_job, source, cancel).await;
        if let Err(e) = outcome {
            warn!(job_id = %job_id, "batch deletion worker failed: {e}");
        }
    });

    Ok(job)
}

/// Ask a running job to stop. The worker checks between batches, so the
/// already-deleted messages are not rolled back.
pub fn request_cancel(job_id: &str) -> BichonResult<BatchDeleteJob> {
    let Some(mut job) = get_job(job_id)? else {
        return Err(raise_error!(
            format!("Batch deletion job '{job_id}' not found."),
            ErrorCode::ResourceNotFound
        ));
    };
    match job.status {
        BatchDeleteStatus::Running => {
            job.status = BatchDeleteStatus::Cancelling;
            job.cancel_requested = true;
            upsert_job(&job)?;
            flush_job_state();
            if let Some(flag) = lock_cancel().get(job_id) {
                flag.store(true, Ordering::SeqCst);
            }
        }
        BatchDeleteStatus::Cancelling => {}
        _ => {
            return Err(raise_error!(
                format!("Batch deletion job '{job_id}' is not running."),
                ErrorCode::InvalidParameter
            ));
        }
    }
    Ok(job)
}

async fn run_job(
    mut job: BatchDeleteJob,
    source: JobSource,
    cancel: Arc<AtomicBool>,
) -> BichonResult<()> {
    let job_id = job.job_id.clone();
    let result = run_job_inner(&mut job, source, &cancel).await;
    job.finished_at = Some(utc_now!());
    match &result {
        Ok(()) => {
            if cancel.load(Ordering::SeqCst) {
                job.status = BatchDeleteStatus::Cancelled;
            } else if job.failed > 0 {
                job.status = BatchDeleteStatus::Failed;
            } else {
                job.status = BatchDeleteStatus::Success;
            }
        }
        Err(e) => {
            job.status = BatchDeleteStatus::Failed;
            job.error = Some(format!("{e}"));
        }
    }
    let _ = upsert_job(&job);
    flush_job_state();
    info!(
        job_id = %job_id,
        status = ?job.status,
        total = job.total,
        deleted = job.deleted,
        failed = job.failed,
        "batch deletion finished"
    );
    result
}

async fn run_job_inner(
    job: &mut BatchDeleteJob,
    source: JobSource,
    cancel: &AtomicBool,
) -> BichonResult<()> {
    let by_account = match source {
        JobSource::Search { scope, filter } => {
            // Enumerate once: this is the point-in-time set the user
            // confirmed. New mail arriving after this line is not touched.
            let ids = ENVELOPE_MANAGER.search_ids(scope, filter)?;
            let mut by_account: BTreeMap<u64, Vec<String>> = BTreeMap::new();
            for (account_id, envelope_id) in ids {
                by_account.entry(account_id).or_default().push(envelope_id);
            }
            by_account
        }
        // The frozen set is the approval itself: no search, no widening.
        JobSource::Frozen(by_account) => by_account,
    };
    // Minimum retention: remove locked messages before they are counted or
    // chunked. The enforcing delete path is still the authority (a race with
    // a policy change fails the batch), but filtering here keeps the job's
    // counters and the user's expectation aligned.
    let mut by_account = by_account;
    for (account_id, envelope_ids) in by_account.iter_mut() {
        let Ok(account) = AccountModel::get(*account_id) else {
            continue;
        };
        let Some(cutoff) = crate::retention::min_retention_cutoff_ms(&account) else {
            continue;
        };
        let mut locked = Vec::new();
        for chunk in envelope_ids.chunks(BATCH_SIZE) {
            locked.extend(ENVELOPE_MANAGER.min_retention_locked_ids(
                *account_id,
                chunk,
                cutoff,
            )?);
        }
        if locked.is_empty() {
            continue;
        }
        let locked: std::collections::HashSet<&String> = locked.iter().collect();
        job.skipped_locked += locked.len() as u64;
        envelope_ids.retain(|id| !locked.contains(id));
    }
    job.total = by_account.values().map(|ids| ids.len() as u64).sum();
    upsert_job(job)?;

    for (account_id, envelope_ids) in by_account {
        for chunk in envelope_ids.chunks(BATCH_SIZE) {
            if cancel.load(Ordering::SeqCst) {
                job.status = BatchDeleteStatus::Cancelling;
                upsert_job(job)?;
                return Ok(());
            }
            let mut request: HashMap<u64, Vec<String>> = HashMap::new();
            request.insert(account_id, chunk.to_vec());
            let plan = plan_delete_audit(&request);
            match crate::message::delete::delete_messages_impl(request).await {
                Ok(()) => {
                    // The deletion is attributed to the person who executed
                    // it: the approver when a second person ran an approved
                    // request, otherwise the job's owner.
                    let audit_user = job.executed_by.as_deref().unwrap_or(&job.username);
                    emit_delete_audit_scoped(
                        plan,
                        audit_user,
                        Some(&job.job_id),
                        Some(&job.filter),
                    );
                    job.deleted += chunk.len() as u64;
                    bump_account_progress(job, account_id, chunk.len() as u64, 0);
                }
                Err(e) => {
                    // One failed batch does not stop the rest: the operator
                    // gets exact counts and can retry the remainder. The
                    // snapshots planned for this batch are dropped unsent, so
                    // the audit trail never claims a deletion that did not
                    // happen.
                    job.failed += chunk.len() as u64;
                    bump_account_progress(job, account_id, 0, chunk.len() as u64);
                    job.error = Some(format!("{e}"));
                    warn!(
                        job_id = %job.job_id,
                        account_id,
                        count = chunk.len(),
                        "batch deletion batch failed: {e}"
                    );
                }
            }
            job.processed += chunk.len() as u64;
            upsert_job(job)?;
        }
    }
    Ok(())
}

fn bump_account_progress(job: &mut BatchDeleteJob, account_id: u64, deleted: u64, failed: u64) {
    match job
        .account_progress
        .iter_mut()
        .find(|progress| progress.account_id == account_id)
    {
        Some(progress) => {
            progress.deleted += deleted;
            progress.failed += failed;
        }
        None => job.account_progress.push(BatchDeleteAccountProgress {
            account_id,
            deleted,
            failed,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::cli::SETTINGS;
    use crate::settings::dir::DATA_DIR_MANAGER;
    use std::sync::Once;

    static TEST_ENV: Once = Once::new();
    static TEST_LOCK: LazyLock<tokio::sync::Mutex<()>> =
        LazyLock::new(|| tokio::sync::Mutex::new(()));

    fn init_test_env() {
        TEST_ENV.call_once(|| {
            let root = crate::test_support::unique_test_root("bichon-core-test");
            std::env::set_var("BICHON_ROOT_DIR", &root);
            std::env::set_var("BICHON_ENCRYPT_PASSWORD", "test-password");
            let _ = &*SETTINGS;
            let _ = &*DATA_DIR_MANAGER;
            let _ = &*DB_MANAGER;
            let _ = &*ENVELOPE_MANAGER;
            let _ = &*crate::store::tantivy::attachment::ATTACHMENT_MANAGER;
        });
    }

    fn test_job(id: &str, created_at: i64, status: BatchDeleteStatus) -> BatchDeleteJob {
        BatchDeleteJob {
            job_id: id.to_string(),
            user_id: 1,
            username: "tester".into(),
            created_at,
            started_at: Some(created_at),
            finished_at: if status == BatchDeleteStatus::Running {
                None
            } else {
                Some(created_at)
            },
            status,
            ..Default::default()
        }
    }

    fn clear_test_jobs(prefix: &str) {
        for job in list_jobs().unwrap() {
            if job.job_id.starts_with(prefix) {
                let _ = delete_impl::<BatchDeleteJob>(DB_MANAGER.db(), &job.job_id);
            }
        }
    }

    /// Permission is per source account. A denied account is reported once and
    /// contributes no groups; every group of an allowed account survives (a
    /// source can be reachable through more than one view).
    #[test]
    fn delete_scope_splits_by_permission() {
        let groups = vec![
            ViewScopeGroup::unbounded(1),
            ViewScopeGroup::unbounded(2),
            ViewScopeGroup {
                account_id: 1,
                any_participant: vec!["a@example.com".into()],
                any_recipient: Vec::new(),
            },
        ];
        let (allowed, denied) = split_delete_scope(Some(groups), |id| id != 2).unwrap();
        assert_eq!(allowed.len(), 2);
        assert!(allowed.iter().all(|group| group.account_id == 1));
        assert_eq!(denied, vec![2]);
    }

    #[test]
    fn status_serializes_snake_case() {
        let job = BatchDeleteJob {
            status: BatchDeleteStatus::Interrupted,
            ..Default::default()
        };
        let json = serde_json::to_string(&job).unwrap();
        assert!(json.contains("\"interrupted\""));
    }

    #[test]
    fn empty_filter_is_rejected_without_scope() {
        let request = BatchDeleteRequest::default();
        let err = start_job(None, request, vec![], 1, "u".into()).unwrap_err();
        assert!(format!("{err}").contains("search condition"));
    }

    #[test]
    fn job_key_is_job_id() {
        let job = BatchDeleteJob {
            job_id: "abc".into(),
            ..Default::default()
        };
        assert_eq!(BatchDeleteJob::collection(), "batch_delete_jobs");
        assert_eq!(job.key(), "abc");
    }

    #[test]
    fn resolve_account_ids_prefers_filter_when_unscoped() {
        let mut filter = EmailSearchFilter::default();
        filter.account_ids = Some([9u64, 3].into_iter().collect());
        assert_eq!(resolve_account_ids(&None, &filter).unwrap(), vec![3, 9]);
    }

    #[tokio::test]
    async fn restart_marks_only_open_jobs_interrupted() {
        init_test_env();
        let _guard = TEST_LOCK.lock().await;
        clear_test_jobs("test-restart-");
        upsert_job(&test_job(
            "test-restart-running",
            1,
            BatchDeleteStatus::Running,
        ))
        .unwrap();
        upsert_job(&test_job(
            "test-restart-cancelling",
            2,
            BatchDeleteStatus::Cancelling,
        ))
        .unwrap();
        upsert_job(&test_job(
            "test-restart-done",
            3,
            BatchDeleteStatus::Success,
        ))
        .unwrap();

        interrupt_running_jobs().unwrap();

        assert_eq!(
            get_job("test-restart-running").unwrap().unwrap().status,
            BatchDeleteStatus::Interrupted
        );
        assert_eq!(
            get_job("test-restart-cancelling").unwrap().unwrap().status,
            BatchDeleteStatus::Interrupted
        );
        assert_eq!(
            get_job("test-restart-done").unwrap().unwrap().status,
            BatchDeleteStatus::Success
        );
        clear_test_jobs("test-restart-");
    }

    #[tokio::test]
    async fn rolling_window_evicts_the_oldest_finished_job() {
        init_test_env();
        let _guard = TEST_LOCK.lock().await;
        clear_test_jobs("test-roll-");
        // 29 finished + 1 running = 30 (the cap). A 31st insertion must drop
        // exactly the oldest finished job and leave the running one alone.
        for i in 0..29 {
            upsert_job(&test_job(
                &format!("test-roll-old-{i:02}"),
                i,
                BatchDeleteStatus::Success,
            ))
            .unwrap();
        }
        upsert_job(&test_job(
            "test-roll-running-a",
            100,
            BatchDeleteStatus::Running,
        ))
        .unwrap();
        evict_old_jobs().unwrap();

        assert!(get_job("test-roll-old-00").unwrap().is_none(), "oldest removed");
        assert!(get_job("test-roll-old-01").unwrap().is_some());
        assert!(get_job("test-roll-running-a").unwrap().is_some());
        clear_test_jobs("test-roll-");
    }

    /// A job started from an approved request belongs to the requester, but
    /// records the approver who executed it and the decision it came from: the
    /// requester keeps the history entry, and an auditor can walk from the
    /// approval to the job that carries the per-batch audit events.
    #[tokio::test]
    async fn frozen_job_records_requester_and_executor() {
        init_test_env();
        let _guard = TEST_LOCK.lock().await;

        let by_account: BTreeMap<u64, Vec<String>> = [(93_901u64, vec!["frozen-1".to_string()])]
            .into_iter()
            .collect();
        let job = start_job_from_ids(
            by_account,
            EmailSearchFilter::default(),
            None,
            JobOrigin {
                user_id: 7,
                username: "requester".into(),
                executed_by: Some("approver".into()),
                approval_request_id: Some("request-1".into()),
            },
        )
        .expect("the job starts");

        assert_eq!(job.user_id, 7);
        assert_eq!(job.username, "requester");
        assert_eq!(job.executed_by.as_deref(), Some("approver"));
        assert_eq!(
            job.approval_request_id.as_deref(),
            Some("request-1"),
            "the decision must stay linked to the job it produced"
        );

        // No account 93_901 exists, so the worker's batches fail; wait for it
        // to reach a terminal state so the in-process slot cannot leak into a
        // later test.
        let _ = request_cancel(&job.job_id);
        for _ in 0..200 {
            match get_job(&job.job_id) {
                Ok(Some(current))
                    if matches!(
                        current.status,
                        BatchDeleteStatus::Running | BatchDeleteStatus::Cancelling
                    ) =>
                {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await
                }
                _ => break,
            }
        }
        let _ = delete_impl::<BatchDeleteJob>(DB_MANAGER.db(), &job.job_id);
    }

    #[tokio::test]
    async fn rolling_window_refuses_to_evict_a_running_job() {
        init_test_env();
        let _guard = TEST_LOCK.lock().await;
        clear_test_jobs("test-busy-");
        for i in 0..MAX_JOBS as i64 {
            upsert_job(&test_job(
                &format!("test-busy-{i:02}"),
                i,
                BatchDeleteStatus::Running,
            ))
            .unwrap();
        }
        let err = evict_old_jobs().unwrap_err();
        assert!(format!("{err}").contains("still running"));
        clear_test_jobs("test-busy-");
    }

    async fn write_doc(id: &str, account_id: u64, mailbox_id: u64, uid: u32) {
        use crate::store::tantivy::schema::SchemaTools;
        use tantivy::TantivyDocument;
        let f = SchemaTools::email_fields();
        let mut doc = TantivyDocument::new();
        doc.add_text(f.f_id, id);
        doc.add_text(f.f_message_id, format!("{id}@test"));
        doc.add_u64(f.f_account_id, account_id);
        doc.add_u64(f.f_mailbox_id, mailbox_id);
        doc.add_u64(f.f_uid, uid as u64);
        doc.add_u64(f.f_imap_uid, uid as u64 + 1000);
        doc.add_i64(f.f_date, 1);
        doc.add_i64(f.f_internal_date, 1);
        doc.add_u64(f.f_size, 10);
        let mut writer = ENVELOPE_MANAGER.index_writer().lock().await;
        writer.add_document(doc).unwrap();
        writer.commit().unwrap();
    }

    /// End to end: a job enumerates the filter, deletes in batches, records
    /// tombstones and finishes with exact counts.
    #[tokio::test]
    async fn job_deletes_matching_messages_and_records_tombstones() {
        use crate::account::migration::{AccountModel, AccountType};
        use crate::account::payload::AccountCreateRequest;
        use crate::archive::imap::mailbox::MailBox;
        use crate::database::insert_impl;
        use crate::store::tantivy::envelope::ENVELOPE_MANAGER;

        init_test_env();
        let _guard = TEST_LOCK.lock().await;
        let _index_guard = crate::test_support::GLOBAL_INDEX_LOCK.lock().await;

        let account_id = 93_001;
        let mailbox_id = 93_002;
        let request = AccountCreateRequest {
            email: "batch-delete@test.example".into(),
            login_name: None,
            account_name: None,
            imap: None,
            enabled: true,
            date_since: None,
            date_before: None,
            account_type: AccountType::NoSync,
            download_interval_min: None,
            download_batch_size: None,
            max_email_size_bytes: None,
            use_dangerous: false,
            pgp_key: None,
            imap_quota_bytes: None,
            imap_quota_window: None,
            auto_download_new_mailboxes: None,
            download_schedule: None,
            archive_rules: None,
            extraction_rules: None,
            override_from_headers: None,
            retention_days: None,
            min_retention_days: None,
            remote_cleanup: None,
            kind: None,
            view: None,
        };
        let mut account = AccountModel::new(0, request).unwrap();
        account.id = account_id;
        insert_impl(DB_MANAGER.db(), account).unwrap();
        insert_impl(
            DB_MANAGER.db(),
            MailBox {
                id: mailbox_id,
                account_id,
                name: "INBOX".into(),
                uid_validity: Some(11),
                ..Default::default()
            },
        )
        .unwrap();

        write_doc("batch-env-1", account_id, mailbox_id, 1).await;
        write_doc("batch-env-2", account_id, mailbox_id, 2).await;
        write_doc("batch-env-3", account_id, mailbox_id, 3).await;

        let mut filter = EmailSearchFilter::default();
        filter.mailbox_ids = Some([mailbox_id].into_iter().collect());
        let job = start_job(
            None,
            BatchDeleteRequest {
                filter,
                label: Some("three".into()),
            },
            vec![account_id],
            7,
            "tester".into(),
        )
        .unwrap();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let finished = loop {
            let current = get_job(&job.job_id).unwrap().unwrap();
            if !matches!(
                current.status,
                BatchDeleteStatus::Running | BatchDeleteStatus::Cancelling
            ) {
                break current;
            }
            assert!(std::time::Instant::now() < deadline, "job did not finish");
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        };

        assert_eq!(finished.status, BatchDeleteStatus::Success);
        assert_eq!(finished.total, 3);
        assert_eq!(finished.deleted, 3);
        assert_eq!(finished.failed, 0);

        let tombstones = crate::archive::imap::tombstones::load(account_id, mailbox_id)
            .unwrap()
            .expect("tombstones written");
        assert!(tombstones.bitmap.contains(1));
        assert!(tombstones.bitmap.contains(3));

        let remaining = ENVELOPE_MANAGER
            .search_ids(
                None,
                EmailSearchFilter {
                    mailbox_ids: Some([mailbox_id].into_iter().collect()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(remaining.is_empty(), "all matching messages deleted");

        // A frozen job uses the given ids verbatim: no search, no widening.
        write_doc("batch-env-4", account_id, mailbox_id, 4).await;
        write_doc("batch-env-5", account_id, mailbox_id, 5).await;
        write_doc("batch-env-6", account_id, mailbox_id, 6).await;
        let frozen_job = start_job_from_ids(
            BTreeMap::from([(
                account_id,
                vec!["batch-env-4".to_string(), "batch-env-5".to_string()],
            )]),
            EmailSearchFilter::default(),
            None,
            JobOrigin {
                user_id: 7,
                username: "tester".into(),
                executed_by: None,
                approval_request_id: None,
            },
        )
        .unwrap();
        let frozen_done = loop {
            let current = get_job(&frozen_job.job_id).unwrap().unwrap();
            if !matches!(
                current.status,
                BatchDeleteStatus::Running | BatchDeleteStatus::Cancelling
            ) {
                break current;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "frozen job did not finish"
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        };
        assert_eq!(frozen_done.status, BatchDeleteStatus::Success);
        assert_eq!(frozen_done.total, 2);
        assert_eq!(frozen_done.deleted, 2);
        assert_eq!(
            frozen_done.account_progress,
            vec![BatchDeleteAccountProgress {
                account_id,
                deleted: 2,
                failed: 0,
            }]
        );
        let remaining = ENVELOPE_MANAGER
            .search_ids(
                None,
                EmailSearchFilter {
                    mailbox_ids: Some([mailbox_id].into_iter().collect()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(remaining.len(), 1, "only the unfrozen id survives");
        assert_eq!(remaining[0].1, "batch-env-6");

        let _ = delete_impl::<BatchDeleteJob>(DB_MANAGER.db(), &job.job_id);
        let _ = delete_impl::<BatchDeleteJob>(DB_MANAGER.db(), &frozen_job.job_id);
        crate::archive::imap::tombstones::remove_account(account_id)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn cancel_moves_running_to_cancelling() {
        init_test_env();
        let _guard = TEST_LOCK.lock().await;
        upsert_job(&test_job("test-cancel", 1, BatchDeleteStatus::Running)).unwrap();

        let job = request_cancel("test-cancel").unwrap();
        assert_eq!(job.status, BatchDeleteStatus::Cancelling);
        assert!(job.cancel_requested);
        assert_eq!(
            get_job("test-cancel").unwrap().unwrap().status,
            BatchDeleteStatus::Cancelling
        );

        // A finished job cannot be cancelled.
        upsert_job(&test_job("test-cancel-done", 2, BatchDeleteStatus::Success)).unwrap();
        assert!(request_cancel("test-cancel-done").is_err());
        let _ = delete_impl::<BatchDeleteJob>(DB_MANAGER.db(), "test-cancel");
        let _ = delete_impl::<BatchDeleteJob>(DB_MANAGER.db(), "test-cancel-done");
    }
}
