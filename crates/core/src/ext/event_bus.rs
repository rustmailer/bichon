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

// Event bus extension point.
//
// Community edition: NoopEventBus — all events are discarded.
// Pro edition: AuditEventBus — events are persisted to audit database.
// Enterprise edition: adds SIEM webhook to the same trait impl.
//
// The open-source server emits events at key points (login, view, delete, search).
// It never reads from the event bus — events are fire-and-forget.

use std::net::IpAddr;
use std::sync::{LazyLock, Mutex, RwLock};
use std::time::{Duration, Instant};

use crate::account::access::ViewAccess;
use crate::oauth2::entity::OAuth2ClientType;

/// Free-form JSON value carried by events that need a content snapshot
/// (e.g. the subject / attachment names of a message being deleted, so the
/// audit trail stays self-describing after the content is gone).
pub type EventPayload = serde_json::Map<String, serde_json::Value>;

#[derive(Debug, Clone)]
pub enum Event {
    EmailViewed {
        email_id: String,
        user: String,
        ip: IpAddr,
        account_id: u64,
        mailbox_id: u64,
        /// Subject of the viewed message, so the audit trail is readable
        /// without resolving the envelope again.
        subject: Option<String>,
        /// Set when the message was readable through a *view* rather than a
        /// direct grant. Without it the trail says "user U read a message in
        /// account A" and cannot answer the compliance question behind it —
        /// *by whose delegation*, and therefore whether the read was inside the
        /// scope the viewer was meant to have.
        via_view: Option<ViewAccess>,
    },
    EmailDeleted {
        email_id: String,
        user: String,
        account_id: u64,
        mailbox_id: u64,
        /// Subject at deletion time (the message is gone afterwards).
        subject: Option<String>,
        /// Snapshot of the deleted message (attachment names, from/to, ...).
        snapshot: Option<EventPayload>,
        /// The job that performed the deletion, when it was a batch job.
        job_id: Option<String>,
        /// The search filter the job was created with. Recorded so the audit
        /// trail answers "under which selection was this deleted", not only
        /// "which messages disappeared".
        filter: Option<crate::message::search::EmailSearchFilter>,
    },
    /// A large delete batch whose per-message detail was externalized to a
    /// detail file instead of N `email.deleted` records. The row carries the
    /// file reference (relative path, size, SHA-256) plus the counts; the
    /// file itself is fetched on demand through the audit detail endpoint and
    /// is never shipped to SIEM.
    EmailBatchDeleted {
        user: String,
        /// Accounts spanned by the batch. One account lands in the indexed
        /// `account_id` column; many stay in the payload (see
        /// `single_account_id` in the Pro audit store).
        account_ids: Vec<u64>,
        /// Total messages deleted in the batch.
        count: u64,
        /// Correlation id; also the detail file name (`detail/<id>.jsonl`).
        batch_id: String,
        /// Relative path of the detail file, resolved against the audit dir.
        detail_file: String,
        detail_size: u64,
        /// SHA-256 of the detail file, bound into the audit hash chain.
        detail_sha256: String,
        /// The job that produced this batch.
        job_id: Option<String>,
        /// The search filter the job was created with. The detail file lists
        /// *what* was deleted; this records *why that set existed*.
        filter: Option<crate::message::search::EmailSearchFilter>,
    },
    /// The scheduled retention sweep permanently purged messages. Emitted
    /// once per account per sweep (never per message): a GoBD auditor needs
    /// the policy value and the cutoff, plus a way to resolve the exact ids.
    RetentionPurged {
        account_id: u64,
        /// Messages removed in this sweep.
        count: u64,
        /// Effective cutoff: messages older than this were eligible.
        cutoff_ms: i64,
        /// The maximum-retention window that produced the cutoff.
        window_days: u64,
        /// Optional JSONL detail file listing the purged envelope ids. Absent
        /// when audit detail is not configured (community) or the write failed.
        detail_file: Option<String>,
        detail_size: Option<u64>,
        detail_sha256: Option<String>,
    },
    /// An account's retention policy changed. Recorded separately from the
    /// generic `account.updated` row because the before/after values are the
    /// evidence: "who shortened the retention window, and from what".
    RetentionPolicyChanged {
        user: String,
        account_id: u64,
        retention_days_before: Option<u64>,
        retention_days_after: Option<u64>,
        min_retention_days_before: Option<u64>,
        min_retention_days_after: Option<u64>,
    },
    /// Raw EML file downloaded (export).
    EmailExported {
        email_id: String,
        user: String,
        account_id: u64,
        subject: Option<String>,
    },
    /// Email restored back to the source IMAP server.
    EmailRestored {
        email_id: String,
        user: String,
        account_id: u64,
        subject: Option<String>,
    },
    /// A source-mailbox cleanup preview was computed (read-only).
    RemoteCleanupPreviewed {
        user: String,
        account_id: u64,
        /// Kind of preview. Only `move` exists today.
        kind: String,
        candidates: u64,
        verified: u64,
        bytes: u64,
    },
    /// A scheduled or manual move run finished. `moved` counts messages moved
    /// out of the source folders; nothing in this event is a permanent delete.
    RemoteCleanupRunFinished {
        /// `None` for the scheduler; audit trails render it as `system`.
        user: Option<String>,
        account_id: u64,
        run_id: String,
        trigger: String,
        status: String,
        moved: u64,
        skipped: u64,
        failed: u64,
        bytes_moved: u64,
        message: Option<String>,
    },
    /// One message was moved from a source folder into the cleanup folder.
    /// Emitted per message so the trail names exactly what left the mailbox.
    RemoteCleanupMessageMoved {
        user: Option<String>,
        account_id: u64,
        source_mailbox: String,
        source_uid: u32,
        destination_folder: String,
        message_id: String,
        size: u64,
    },
    /// A guard rail (legal hold, stale backup, missing MOVE, missing
    /// destination) refused a cleanup run before anything was moved.
    RemoteCleanupBlocked {
        user: Option<String>,
        account_id: u64,
        reason: String,
    },
    /// A repair re-fetched the message from IMAP when its archived blobs were
    /// missing or unreadable, and/or rebuilt the index document from the
    /// archived EML when its presentation fields were wrong — both in place,
    /// keeping the envelope id. Emitted for every outcome the repair reports,
    /// `failed` included: an attempt to rewrite archived content is worth
    /// recording even when the fetched bytes did not match and nothing was
    /// written.
    EmailRepaired {
        email_id: String,
        user: String,
        /// Absent when the request carried no peer address.
        ip: Option<IpAddr>,
        account_id: u64,
        mailbox_id: u64,
        subject: String,
        /// `complete` (nothing was wrong), `repaired`, `partial` or `failed`.
        status: String,
        email_missing_before: bool,
        email_missing_after: bool,
        /// Whether the envelope document was re-derived from the archived EML.
        index_updated: bool,
        /// How many attachment blobs were suspected missing before / after.
        missing_before: usize,
        missing_after: usize,
        /// Set when the fetched content validated but the write was skipped
        /// because a backup window was open.
        skipped_due_to_backup: bool,
        error: Option<String>,
    },
    /// Facet tags added/removed on emails.
    EmailTagged {
        user: String,
        account_id: u64,
        /// Number of emails whose tags were changed.
        count: u64,
    },
    /// Facet tags added/removed on attachments.
    AttachmentTagged {
        user: String,
        account_id: u64,
        /// Number of attachments whose tags were changed.
        count: u64,
    },
    /// Attachment content streamed for in-browser preview (not a download).
    AttachmentPreviewed {
        email_id: String,
        content_hash: String,
        user: String,
        account_id: u64,
        mailbox_id: u64,
        filename: Option<String>,
        /// See [`Event::EmailViewed`]: the view that made the parent message
        /// readable, when it was not a direct grant.
        via_view: Option<ViewAccess>,
    },
    UserLoggedIn {
        user: String,
        ip: IpAddr,
    },
    /// TOTP two-factor authentication was enabled for a user.
    MfaEnabled {
        user: String,
        ip: Option<IpAddr>,
    },
    /// TOTP two-factor authentication was disabled for a user.
    MfaDisabled {
        user: String,
        ip: Option<IpAddr>,
    },
    /// An administrator forcibly reset another user's TOTP two-factor
    /// authentication (e.g. the user lost access to their authenticator).
    MfaResetByAdmin {
        admin: String,
        target_user: String,
        ip: Option<IpAddr>,
    },
    UserCreated {
        created_by: String,
        new_user: String,
    },
    UserUpdated {
        updated_by: String,
        target_user: String,
    },
    UserRemoved {
        removed_by: String,
        target_user: String,
    },
    RoleCreated {
        created_by: String,
        role_name: String,
    },
    RoleUpdated {
        updated_by: String,
        role_name: String,
    },
    RoleRemoved {
        removed_by: String,
        role_name: String,
    },
    SearchPerformed {
        query: String,
        user: String,
    },
    SettingsChanged {
        key: String,
        user: String,
    },
    AttachmentDownloaded {
        email_id: String,
        /// The attachment's own content hash.
        content_hash: String,
        user: String,
        account_id: u64,
        mailbox_id: u64,
        filename: Option<String>,
        size: Option<u64>,
        ext: Option<String>,
        /// Content hash of the parent email (EML), when known.
        parent_content_hash: Option<String>,
        /// See [`Event::EmailViewed`]: the view that made the parent message
        /// readable, when it was not a direct grant.
        via_view: Option<ViewAccess>,
    },
    AccountCreated {
        created_by: String,
        account_id: u64,
        email: String,
    },
    AccountUpdated {
        updated_by: String,
        account_id: u64,
        email: String,
        /// Set when the update changed a view's sources or filters. An
        /// `account.updated` line on its own cannot distinguish a rename from
        /// dropping a filter, and dropping a filter is a widening of someone
        /// else's visibility.
        view_change: Option<crate::account::migration::ViewConfigChange>,
    },
    AccountRemoved {
        removed_by: String,
        account_id: u64,
        email: String,
    },
    AccountDownloadStarted {
        user: String,
        account_id: u64,
        run_gap_fill: bool,
    },
    AccountDownloadStopped {
        user: String,
        account_id: u64,
    },
    /// Batch role / access assignment on one or more accounts.
    AccountRoleAssigned {
        user: String,
        target_user: String,
        account_count: usize,
        roles: Vec<String>,
    },
    AccessTokenCreated {
        user: String,
        target_user: String,
        name: Option<String>,
    },
    AccessTokenRemoved {
        user: String,
        token_user: String,
        name: Option<String>,
    },
    /// A role assignment was given an end date (Enterprise).
    ///
    /// `expires_at` is always `Some` here — clearing a deadline is
    /// `RoleDelegationRemoved`, a separate event, so the log distinguishes
    /// "delegated until 31 Dec" from "delegation cleared" by event type alone
    /// rather than by inspecting a nullable field.
    RoleDelegated {
        user: String,
        target_user: String,
        target_user_id: u64,
        role_id: u64,
        role_name: String,
        expires_at: Option<i64>,
    },
    /// A role assignment's end date was removed, leaving the role in place.
    ///
    /// Distinct from `RoleDelegated` with `expires_at: None` so that "the
    /// deadline was cleared" and "a deadline was set and then cleared" are
    /// distinguishable in the log by the pair of records they leave.
    RoleDelegationRemoved {
        user: String,
        target_user: String,
        target_user_id: u64,
        role_id: u64,
        role_name: String,
    },
    OAuth2ConfigCreated {
        user: String,
        oauth2_id: u64,
        name: String,
        /// Type the configuration was created with. A public client holds no
        /// secret, so nothing else in the record implies it.
        client_type: OAuth2ClientType,
    },
    OAuth2ConfigUpdated {
        user: String,
        oauth2_id: u64,
        name: String,
        /// Effective type after the update, not the requested one.
        client_type: OAuth2ClientType,
    },
    OAuth2ConfigRemoved {
        user: String,
        oauth2_id: u64,
        name: String,
    },
    /// External OAuth2 token stored / refreshed for an account.
    OAuth2TokenStored {
        user: String,
        account_id: u64,
    },
    ImportPerformed {
        user: String,
        account_id: u64,
        format: String,
        total: u64,
        success: u64,
        duplicates: u64,
        failed: u64,
    },
    MailboxRemoved {
        user: String,
        account_id: u64,
        mailbox_id: u64,
    },
    ProxyCreated {
        user: String,
        url: String,
    },
    ProxyUpdated {
        user: String,
        url: String,
    },
    ProxyRemoved {
        user: String,
        url: String,
    },
    /// Pro edition: SSO (OIDC) login, logout, or license upload.
    SsoLogin {
        user: String,
        ip: Option<IpAddr>,
    },
    SsoLogout {
        user: String,
    },
    LicenseUploaded {
        user: String,
        email: String,
        edition: String,
    },
    /// Pro edition: admin updated the brand identity (company name / tagline / logo).
    BrandingUpdated {
        user: String,
        company_name: Option<String>,
        tagline: Option<String>,
        logo_changed: bool,
    },
    /// Pro edition: admin started an integrity check (manual full verification).
    IntegrityCheckStarted {
        user: String,
        run_id: String,
        scope: String,
        mode: String,
    },
    /// Pro edition: integrity check finished with a summary.
    IntegrityCheckCompleted {
        user: String,
        run_id: String,
        total: u64,
        ok: u64,
        failed: u64,
    },
    /// Pro edition: an in-flight integrity check was cancelled.
    IntegrityCheckCancelled {
        user: String,
        run_id: String,
        processed: u64,
    },
    /// Pro edition: an integrity report was downloaded (CSV).
    IntegrityReportDownloaded {
        user: String,
        run_id: String,
        report_type: String,
    },
    /// Pro edition: a user saved a search condition for reuse.
    SavedSearchCreated {
        user: String,
        search_id: String,
        kind: String,
        name: String,
    },
    /// Pro edition: a saved search was renamed (conditions are immutable).
    SavedSearchRenamed {
        user: String,
        search_id: String,
        name: String,
    },
    /// Pro edition: a saved search was deleted.
    SavedSearchDeleted {
        user: String,
        search_id: String,
        name: String,
    },
    /// A batch export job was started from a saved search.
    ExportStarted {
        user: String,
        export_id: String,
        saved_search_id: String,
        /// The saved search's name, or the caller's label for an ad-hoc
        /// export (e.g. "All images"). Empty `saved_search_id` plus a name is
        /// how an ad-hoc export reads in the audit trail.
        saved_search_name: String,
        format: String,
        account_count: u64,
        email_count: u64,
    },
    /// A batch export job finished successfully.
    ExportCompleted {
        user: String,
        export_id: String,
        saved_search_id: String,
        saved_search_name: String,
        exported: u64,
        failed: u64,
        artifact_size: u64,
        /// SHA-256 of the finished artifact, bound into the audit trail.
        artifact_hash: Option<String>,
    },
    /// A finished export artifact was compliance-verified: the artifact's
    /// SHA-256 was recomputed and matched, and per-message content hashes
    /// were cross-checked against the live archive.
    ExportVerified {
        user: String,
        export_id: String,
        artifact_hash: Option<String>,
        checked: u64,
        matched: u64,
        mismatched: u64,
    },
    /// A batch export job failed.
    ExportFailed {
        user: String,
        export_id: String,
        saved_search_id: String,
        saved_search_name: String,
        error: String,
    },
    /// A batch export job was cancelled by its owner.
    ExportCancelled {
        user: String,
        export_id: String,
    },
    /// An export artifact was downloaded.
    ExportDownloaded {
        user: String,
        export_id: String,
        email_count: u64,
        artifact_size: u64,
    },
    /// A legal hold was placed on an account (Enterprise). The account is
    /// frozen: the retention sweep skips it and bulk deletion refuses.
    LegalHoldPlaced {
        user: String,
        account_id: u64,
        /// Operator-recorded reason for the hold (audit-friendly).
        reason: Option<String>,
    },
    /// A legal hold was released from an account (Enterprise).
    LegalHoldReleased {
        user: String,
        account_id: u64,
        /// Operator-recorded reason at release time.
        reason: Option<String>,
    },
    /// A Merkle-tree root over archived content hashes was anchored with an
    /// external Time-Stamp Authority (RFC 3161, Enterprise).
    TimestampAnchored {
        user: String,
        anchor_id: String,
        root_hash: String,
        /// TSA-certified time (epoch millis).
        gen_time: i64,
        leaf_count: u64,
        tsa_url: Option<String>,
    },
    /// A user signed in with an LDAP/AD bind (Enterprise).
    LdapLogin {
        user: String,
        ip: Option<IpAddr>,
    },
    /// An LDAP/AD bind-based sign-in attempt failed (Enterprise).
    LdapLoginFailed {
        username: String,
        ip: Option<IpAddr>,
        reason: String,
    },
    /// The SIEM webhook forwarder gave up on a batch after exhausting its
    /// retries (Enterprise). The cursor is NOT advanced, so the batch is
    /// retried on the next cycle (at-least-once delivery).
    SiemForwardFailed {
        url: String,
        seq_from: i64,
        seq_to: i64,
        error: String,
    },
    /// The SIEM webhook configuration was changed by an admin (Enterprise).
    SiemConfigUpdated { user: String },
    /// The backup configuration (target, schedule, retention, tags, excludes,
    /// credentials) was changed by an admin from the WebUI. Credential
    /// changes are audited as a single event; the values themselves never
    /// leave the server.
    BackupConfigUpdated { user: String },
    /// Pro/Enterprise: the compliance configuration (audit retention,
    /// integrity schedule and retention, TSP anchoring interval) was changed
    /// by an admin from the WebUI.
    ComplianceConfigUpdated { user: String },
    /// Pro/Enterprise: the audit trail itself was queried or its hash chain
    /// verified. Reading the compliance evidence is itself security-relevant —
    /// an operator who can inspect the trail without leaving a trace of their
    /// own would be a separation-of-duties hole.
    AuditLogQueried {
        user: String,
        /// Which surface asked: `list`, `by_email`, or `verify`.
        scope: String,
        /// The email the query was narrowed to, when the caller asked about
        /// one message's history.
        email_id: Option<String>,
        ip: Option<IpAddr>,
    },
    /// A GDPR Art. 17 erasure request was recorded (Enterprise).
    ///
    /// This is the record GoBD and the GDPR pull in opposite directions about:
    /// a data subject asked for their data to be erased, an operator assessed
    /// whether a statutory retention obligation stands in the way, and — when
    /// one does not — the erasure was submitted for a second signature.
    ///
    /// Deliberately its own event rather than a `reason` string on
    /// `ApprovalRequested`: the data subject, the search criteria and the
    /// assessment are what a supervisory authority asks to see, and a free-text
    /// field that happens to contain them is not evidence anyone can filter or
    /// machine-read. The assessment is also required at submission — a request
    /// with no stated basis is not something to ask a second person to sign.
    GdprErasureRequested {
        /// The operator who recorded the request.
        user: String,
        /// The data subject the request is about, as the operator recorded it
        /// (e-mail address, name, customer number).
        subject: String,
        /// How the affected messages were identified — the criteria searched.
        criteria: String,
        /// The operator's written assessment of why no statutory retention
        /// obligation applies.
        assessment: String,
        /// How many messages the request names.
        messages: u64,
        account_ids: Vec<u64>,
        /// `true` when dual control queued the request for a second person,
        /// `false` when the policy is off and the erasure ran directly on this
        /// one signature. The distinction is the whole point of the record: a
        /// reader has to be able to tell an authorized erasure from an
        /// unaccompanied one.
        queued: bool,
        /// The approval request id, when the erasure was queued.
        request_id: Option<String>,
    },
    /// A dual-control approval request was created and awaits a second
    /// person's decision (Enterprise). Nothing has happened to the target yet
    /// — this is the "first person" half of the two-person rule.
    ApprovalRequested {
        /// The requester (first person).
        user: String,
        request_id: String,
        /// The gated operation, e.g. `legal_hold.release`.
        op: String,
        account_ids: Vec<u64>,
        reason: Option<String>,
    },
    /// A dual-control request was approved by a second person (Enterprise).
    ///
    /// This records the *decision*, not the outcome. For an operation that runs
    /// in the background the outcome is not known yet when this is emitted —
    /// which is why the counts are optional and omitted rather than written as
    /// `0/0`: a fabricated "nothing succeeded, nothing failed" reads as a real
    /// result to anything consuming the trail. `approval.executed` carries the
    /// outcome.
    ApprovalApproved {
        /// The approver (second person).
        user: String,
        request_id: String,
        op: String,
        /// The requester, carried so the pair is legible in one record.
        requested_by: String,
        account_ids: Vec<u64>,
        /// Per-item outcome, when the approval executed synchronously.
        /// `None` means the outcome is not yet known.
        succeeded: Option<usize>,
        failed: Option<usize>,
        /// Whether this decision was taken through the break-glass override —
        /// one administrator acting alone, after the request had waited out
        /// the configured minimum.
        ///
        /// Carried on the event rather than left to be inferred from
        /// `user == requested_by`: the two are different facts. A normal
        /// approval by a third party and an override by a third party look
        /// identical on the user fields, and only one of them means the second
        /// signature was waived.
        break_glass: bool,
        /// How long the request had been pending when it was overridden.
        /// `None` for the normal path.
        pending_for_hours: Option<i64>,
    },
    /// A dual-control request finished executing (Enterprise). Emitted for both
    /// inline and background operations, so a consumer has one place to read
    /// the outcome from regardless of how the operation ran.
    ApprovalExecuted {
        request_id: String,
        op: String,
        /// The approver — the person who took the action and owns it.
        user: String,
        succeeded: usize,
        failed: usize,
    },
    /// A dual-control request failed to execute (Enterprise). Distinct from
    /// `ApprovalExecuted` with a nonzero failure count: this is the whole
    /// operation failing to reach its target at all (the executor refused up
    /// front, or panicked).
    ApprovalExecutionFailed {
        request_id: String,
        op: String,
        user: String,
        error: String,
    },
    /// A dual-control request was rejected by a second person (Enterprise).
    /// The target is untouched.
    ApprovalRejected {
        /// The rejecter (second person).
        user: String,
        request_id: String,
        op: String,
        requested_by: String,
        account_ids: Vec<u64>,
        note: Option<String>,
    },
    /// A pending dual-control request expired without a decision (Enterprise).
    /// Recorded so a request that quietly lapses leaves a trace.
    ApprovalExpired {
        request_id: String,
        op: String,
        requested_by: String,
    },
}

pub trait EventBus: Send + Sync {
    fn emit(&self, event: Event);
}

/// Default — all events are discarded.
struct NoopEventBus;
impl EventBus for NoopEventBus {
    fn emit(&self, _event: Event) {}
}

static EVENT_BUS: LazyLock<RwLock<Box<dyn EventBus>>> =
    LazyLock::new(|| RwLock::new(Box::new(NoopEventBus)));

/// Short-window dedup of view/download events.
///
/// The web UI can fire duplicate `message-content` requests for the same
/// email (React StrictMode double-effects, remote-content toggle, thread
/// expansion). Deduping here keeps the audit trail to one record per
/// intentional view without hiding repeated deliberate accesses.
static VIEW_DEDUP: LazyLock<Mutex<Vec<(String, Instant)>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));

const VIEW_DEDUP_WINDOW: Duration = Duration::from_secs(10);

fn is_duplicate_view(event: &Event) -> bool {
    let key = match event {
        Event::EmailViewed {
            user, email_id, ..
        } => Some(format!("email.viewed|{user}|{email_id}")),
        Event::EmailDeleted {
            user, email_id, ..
        } => Some(format!("email.deleted|{user}|{email_id}")),
        Event::AttachmentDownloaded {
            user,
            email_id,
            content_hash,
            ..
        } => Some(format!("attachment.downloaded|{user}|{email_id}|{content_hash}")),
        Event::AttachmentPreviewed {
            user,
            email_id,
            content_hash,
            ..
        } => Some(format!("attachment.previewed|{user}|{email_id}|{content_hash}")),
        _ => None,
    };
    let Some(key) = key else {
        return false;
    };

    let mut entries = VIEW_DEDUP.lock().unwrap();
    let now = Instant::now();
    entries.retain(|(_, at)| now.duration_since(*at) < VIEW_DEDUP_WINDOW);
    if entries.iter().any(|(k, _)| *k == key) {
        return true;
    }
    entries.push((key, now));
    false
}

/// Called by Pro/Enterprise at startup to replace the noop default.
pub fn set_event_bus(bus: Box<dyn EventBus>) {
    *EVENT_BUS.write().unwrap() = bus;
}

/// Fire-and-forget. Called by the server at key points.
pub fn emit(event: Event) {
    if is_duplicate_view(&event) {
        return;
    }
    EVENT_BUS.read().unwrap().emit(event);
}
