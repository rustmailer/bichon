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

//! Prometheus / OpenMetrics exposition for the community edition.
//!
//! The process owns a dedicated [`REGISTRY`] (never the crate's global
//! default) and one static per metric family. Instrumentation helpers are
//! infallible on purpose: metrics must never take the ingestion path down.
//!
//! Edition-specific (Pro/Enterprise) families are added from the edition's
//! own codebase through [`register_metrics_collector`]. The community binary
//! registers no collector, so edition series are structurally absent from
//! its output.

pub mod classify;

use prometheus::{
    core::Collector, Gauge, GaugeVec, Histogram, HistogramOpts, HistogramVec,
    IntCounterVec, IntGauge, Opts, Registry, TextEncoder,
};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{LazyLock, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

use crate::account::migration::{AccountModel, AccountType};
use crate::account::state::{DownloadState, DownloadStatus, GapFillState, TriggerType};
use crate::backup::config as backup_config;
use crate::backup::model::BackupRecord;
use crate::bichon_version;
use crate::error::{code::ErrorCode, BichonResult};
use crate::raise_error;
use crate::settings::dir::DATA_DIR_MANAGER;
use crate::store::tantivy::{attachment::ATTACHMENT_MANAGER, envelope::ENVELOPE_MANAGER};
use crate::users::UserModel;
use crate::utils::get_total_size;
use crate::utc_now;

/// Dedicated registry; never register into the process-wide default.
pub static REGISTRY: LazyLock<Registry> = LazyLock::new(Registry::new);

/// Process start, used by `bichon_uptime_seconds`.
static PROCESS_START: LazyLock<Instant> = LazyLock::new(Instant::now);

/// Register a metric family and hand the caller its own handle.
fn registered<T>(metric: T) -> T
where
    T: Collector + Clone + 'static,
{
    REGISTRY
        .register(Box::new(metric.clone()))
        .expect("valid metric registration");
    metric
}

macro_rules! gauge_family {
    ($name:ident, $metric:literal, $help:literal) => {
        pub static $name: LazyLock<Gauge> = LazyLock::new(|| {
            registered(Gauge::new($metric, $help).expect("valid metric definition"))
        });
    };
}

macro_rules! int_gauge_family {
    ($name:ident, $metric:literal, $help:literal) => {
        pub static $name: LazyLock<IntGauge> = LazyLock::new(|| {
            registered(IntGauge::new($metric, $help).expect("valid metric definition"))
        });
    };
}

macro_rules! gauge_vec_family {
    ($name:ident, $metric:literal, $help:literal, [$($label:literal),*]) => {
        pub static $name: LazyLock<GaugeVec> = LazyLock::new(|| {
            registered(
                GaugeVec::new(Opts::new($metric, $help), &[$($label),*])
                    .expect("valid metric definition"),
            )
        });
    };
}

macro_rules! counter_vec_family {
    ($name:ident, $metric:literal, $help:literal, [$($label:literal),*]) => {
        pub static $name: LazyLock<IntCounterVec> = LazyLock::new(|| {
            registered(
                IntCounterVec::new(Opts::new($metric, $help), &[$($label),*])
                    .expect("valid metric definition"),
            )
        });
    };
}

macro_rules! histogram_vec_family {
    ($name:ident, $metric:literal, $help:literal, [$($label:literal),*], $opts:expr) => {
        pub static $name: LazyLock<HistogramVec> = LazyLock::new(|| {
            registered(HistogramVec::new($opts, &[$($label),*]).expect("valid metric definition"))
        });
    };
}

// ---------------------------------------------------------------------------
// 1. Global build & uptime
// ---------------------------------------------------------------------------

gauge_vec_family!(
    BUILD_INFO,
    "bichon_build_info",
    "Bichon build information. The value is always 1; read the version from the 'version' label.",
    ["version"]
);

gauge_family!(
    UPTIME_SECONDS,
    "bichon_uptime_seconds",
    "Seconds since the Bichon server process started."
);

// ---------------------------------------------------------------------------
// 2. Global counts of configured things
// ---------------------------------------------------------------------------

gauge_vec_family!(
    ACCOUNTS_TOTAL,
    "bichon_accounts_total",
    "Number of configured email accounts, by account type (imap, nosync, view).",
    ["type"]
);

gauge_family!(
    USERS_TOTAL,
    "bichon_users_total",
    "Number of user accounts configured on this server."
);

gauge_family!(
    ACCOUNT_MAILBOX_FOLDERS_TOTAL,
    "bichon_account_mailbox_folders_total",
    "Number of tracked mailbox folders across all accounts."
);

// ---------------------------------------------------------------------------
// 3. Global archive totals (TTL-cached)
// ---------------------------------------------------------------------------

gauge_family!(
    EMAILS_TOTAL,
    "bichon_emails_total",
    "Total number of emails archived across all accounts."
);

gauge_family!(
    ATTACHMENTS_TOTAL,
    "bichon_attachments_total",
    "Total number of attachments archived across all accounts."
);

gauge_family!(
    STORAGE_USAGE_BYTES,
    "bichon_storage_usage_bytes",
    "Disk usage of the blob storage directory, in bytes."
);

gauge_family!(
    INDEX_USAGE_BYTES,
    "bichon_index_usage_bytes",
    "Disk usage of the full-text search index directory, in bytes."
);

// ---------------------------------------------------------------------------
// 4. Per-account metadata and sync state
// ---------------------------------------------------------------------------

gauge_vec_family!(
    ACCOUNT_INFO,
    "bichon_account_info",
    "Metadata for a configured email account. The value is always 1; use the labels (email, name, type, enabled) to annotate other per-account series via the account_id label.",
    ["account_id", "email", "name", "type", "enabled"]
);

gauge_vec_family!(
    ACCOUNT_SYNC_RUNNING,
    "bichon_account_sync_running",
    "Whether a download session is currently running for the account (1 = running, 0 = idle).",
    ["account_id"]
);

gauge_vec_family!(
    ACCOUNT_LAST_SYNC_START_TIMESTAMP_SECONDS,
    "bichon_account_last_sync_start_timestamp_seconds",
    "Unix timestamp (seconds) of the most recent download session start, whether running or finished.",
    ["account_id"]
);

gauge_vec_family!(
    ACCOUNT_LAST_SYNC_SUCCESS_TIMESTAMP_SECONDS,
    "bichon_account_last_sync_success_timestamp_seconds",
    "Unix timestamp (seconds) of the end of the most recent successful download session. 0 if the account has never completed a successful sync.",
    ["account_id"]
);

gauge_vec_family!(
    ACCOUNT_LAST_SYNC_SUCCEEDED,
    "bichon_account_last_sync_succeeded",
    "Whether the most recently finished download session ended with status success (1) or not (0).",
    ["account_id"]
);

gauge_vec_family!(
    ACCOUNT_LAST_SYNC_DURATION_SECONDS,
    "bichon_account_last_sync_duration_seconds",
    "Duration in seconds of the most recently finished download session.",
    ["account_id"]
);

gauge_vec_family!(
    ACCOUNT_LAST_SYNC_ERRORS,
    "bichon_account_last_sync_errors",
    "Number of errors recorded in the most recently finished download session.",
    ["account_id"]
);

gauge_vec_family!(
    ACCOUNT_EMAILS_TOTAL,
    "bichon_account_emails_total",
    "Number of emails archived for this account. Absent for accounts that store no mail.",
    ["account_id"]
);

gauge_vec_family!(
    ACCOUNT_DOWNLOAD_INTERVAL_MINUTES,
    "bichon_account_download_interval_minutes",
    "Configured download interval for the account, in minutes. 0 if the account does not sync.",
    ["account_id"]
);

gauge_vec_family!(
    ACCOUNT_GAPFILL_RUNNING,
    "bichon_account_gapfill_running",
    "Whether a gap-fill run is currently in progress for the account (1 = running, 0 = idle).",
    ["account_id"]
);

gauge_vec_family!(
    ACCOUNT_LAST_GAPFILL_DOWNLOADED,
    "bichon_account_last_gapfill_downloaded",
    "Emails newly downloaded by the most recent finished gap-fill run.",
    ["account_id"]
);

gauge_vec_family!(
    ACCOUNT_LAST_GAPFILL_FAILED,
    "bichon_account_last_gapfill_failed",
    "Emails that failed to download in the most recent finished gap-fill run.",
    ["account_id"]
);

// ---------------------------------------------------------------------------
// 5. In-memory counters
// ---------------------------------------------------------------------------

counter_vec_family!(
    EMAILS_INGESTED,
    "bichon_emails_ingested_total",
    "Emails processed by the ingestion pipeline, by account, source and outcome. 'imported' means a new envelope was stored; 'filtered' means the account's archive rules rejected it; 'duplicate' means the content hash was already archived; 'failed' means the message could not be fetched, parsed or stored. Counters reset to zero on process restart; use rate() in queries and alerting.",
    ["account_id", "source", "outcome"]
);

counter_vec_family!(
    SYNC_ERRORS,
    "bichon_sync_errors_total",
    "Download and sync errors by account and classified kind (auth, throttled, network, timeout, protocol, internal). Counters reset to zero on process restart; use rate() in queries and alerting.",
    ["account_id", "kind"]
);

counter_vec_family!(
    SYNC_SESSIONS,
    "bichon_sync_sessions_total",
    "Completed download sessions by account, trigger and final status, including sessions cancelled by a restart or by a newer session. Counters reset to zero on process restart; use rate() in queries and alerting.",
    ["account_id", "trigger", "status"]
);

/// Sync duration buckets: syncs run from seconds to hours.
const SYNC_DURATION_BUCKETS: &[f64] = &[
    1.0, 5.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0, 1800.0, 3600.0, 10800.0,
];

histogram_vec_family!(
    SYNC_SESSION_DURATION_SECONDS,
    "bichon_sync_session_duration_seconds",
    "Duration in seconds of completed download sessions.",
    ["account_id"],
    HistogramOpts::new(
        "bichon_sync_session_duration_seconds",
        "Duration in seconds of completed download sessions.",
    )
    .buckets(SYNC_DURATION_BUCKETS.to_vec())
);

// ---------------------------------------------------------------------------
// 6. Service-level metrics
// ---------------------------------------------------------------------------

counter_vec_family!(
    HTTP_REQUESTS,
    "bichon_http_requests_total",
    "HTTP requests served by the REST API, by method and status class (2xx, 3xx, 4xx, 5xx). Counters reset to zero on process restart; use rate() in queries and alerting.",
    ["method", "status"]
);

histogram_vec_family!(
    HTTP_REQUEST_DURATION_SECONDS,
    "bichon_http_request_duration_seconds",
    "HTTP request latency of the REST API in seconds.",
    ["method"],
    HistogramOpts::new(
        "bichon_http_request_duration_seconds",
        "HTTP request latency of the REST API in seconds.",
    )
);

counter_vec_family!(
    SMTP_MESSAGES,
    "bichon_smtp_messages_total",
    "SMTP messages processed by the built-in receiver, by outcome (accepted, rejected). Counters reset to zero on process restart; use rate() in queries and alerting.",
    ["outcome"]
);

counter_vec_family!(
    SMTP_REJECTIONS,
    "bichon_smtp_rejections_total",
    "SMTP messages rejected before archiving, by reason. Counters reset to zero on process restart; use rate() in queries and alerting.",
    ["reason"]
);

int_gauge_family!(
    SMTP_ACTIVE_CONNECTIONS,
    "bichon_smtp_active_connections",
    "Currently open SMTP client connections."
);

counter_vec_family!(
    BACKUP_RUNS,
    "bichon_backup_runs_total",
    "Backup runs by outcome (success, failed). Counters reset to zero on process restart; use rate() in queries and alerting.",
    ["outcome"]
);

/// Backup durations run minutes to hours.
const BACKUP_DURATION_BUCKETS: &[f64] = &[60.0, 300.0, 600.0, 1800.0, 3600.0, 7200.0, 14400.0, 28800.0];

pub static BACKUP_DURATION_SECONDS: LazyLock<Histogram> = LazyLock::new(|| {
    registered(
        Histogram::with_opts(
            HistogramOpts::new(
                "bichon_backup_duration_seconds",
                "Duration of completed backup runs in seconds.",
            )
            .buckets(BACKUP_DURATION_BUCKETS.to_vec()),
        )
        .expect("valid metric definition"),
    )
});

gauge_family!(
    BACKUP_LAST_SUCCESS_TIMESTAMP_SECONDS,
    "bichon_backup_last_success_timestamp_seconds",
    "Unix timestamp (seconds) of the most recent successful backup. 0 if no backup has ever succeeded."
);

counter_vec_family!(
    OAUTH2_TOKEN_REFRESH,
    "bichon_oauth2_token_refresh_total",
    "OAuth2 access-token refresh attempts by outcome (success, failure). Counters reset to zero on process restart; use rate() in queries and alerting.",
    ["outcome"]
);

// ---------------------------------------------------------------------------
// Edition extension hook
// ---------------------------------------------------------------------------

/// Scrape-time refresh hook for edition-specific (Pro/Enterprise) metrics.
///
/// Community edition registers nothing. A Pro/Enterprise server constructs
/// its collector at startup: the constructor creates the edition's metric
/// families and registers them into [`REGISTRY`], and `refresh` is then
/// called at the start of every scrape to update scrape-time gauges from
/// edition-managed state before the registry is encoded.
pub trait MetricsCollector: Send + Sync {
    /// Human-readable name used in warning logs when refresh fails.
    fn name(&self) -> &'static str;
    /// Refresh scrape-time gauges. Called at the start of every scrape.
    /// Must not panic and must not propagate errors — log and continue,
    /// a broken collector must never fail the whole scrape.
    fn refresh(&self);
}

static COLLECTORS: LazyLock<RwLock<Vec<Box<dyn MetricsCollector>>>> =
    LazyLock::new(|| RwLock::new(Vec::new()));

/// Register an edition-specific collector. Call once at server startup,
/// before the first scrape. Idempotency is the caller's problem: a collector
/// registered twice refreshes twice.
pub fn register_metrics_collector(collector: Box<dyn MetricsCollector>) {
    COLLECTORS
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .push(collector);
}

fn refresh_collectors() {
    let collectors = COLLECTORS.read().unwrap_or_else(|e| e.into_inner());
    for collector in collectors.iter() {
        // The trait contract forbids panics, but a panicking edition collector
        // must still never take the scrape (or the server) down with it.
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| collector.refresh())).is_err() {
            tracing::warn!(
                collector = collector.name(),
                "metrics collector panicked during refresh"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Per-account stale-series tracking
// ---------------------------------------------------------------------------

/// Remembers the label values set for one per-account family so that series
/// of deleted accounts can be removed (`remove_label_values` needs the full
/// tuple, which the registry cannot enumerate by account).
///
/// A family can hold several tuples for one account (e.g. counts for every
/// source/outcome combination), so the tracker keeps all of them.
#[derive(Default)]
struct AccountSeries {
    known: Mutex<HashMap<u64, Vec<Vec<String>>>>,
}

fn label_tuple(labels: &[&str]) -> Vec<String> {
    labels.iter().map(|s| s.to_string()).collect()
}

fn label_refs(labels: &[String]) -> Vec<&str> {
    labels.iter().map(String::as_str).collect()
}

impl AccountSeries {
    /// Record a tuple for the account, keeping any other tuples already
    /// recorded for it.
    fn set(&self, account_id: u64, labels: &[&str]) {
        let mut known = self.known.lock().unwrap_or_else(|e| e.into_inner());
        let tuples = known.entry(account_id).or_default();
        // Compare borrowed values first: ingestion calls this once per
        // message, and the tuple is almost always already known.
        let known_tuple = tuples.iter().any(|tuple| {
            tuple.len() == labels.len()
                && tuple
                    .iter()
                    .zip(labels.iter())
                    .all(|(recorded, current)| recorded.as_str() == *current)
        });
        if !known_tuple {
            tuples.push(label_tuple(labels));
        }
    }

    /// Record the account's single, current tuple, removing any previously
    /// recorded one (used by families whose labels describe the account
    /// itself and change when the account is edited).
    fn replace(&self, account_id: u64, labels: &[&str], remove: &dyn Fn(&[&str])) {
        let tuple = label_tuple(labels);
        let mut known = self.known.lock().unwrap_or_else(|e| e.into_inner());
        let tuples = known.entry(account_id).or_default();
        if tuples.len() == 1 && tuples[0] == tuple {
            return;
        }
        for old in tuples.drain(..) {
            remove(&label_refs(&old));
        }
        tuples.push(tuple);
    }

    /// Remove every recorded tuple for accounts that no longer exist.
    fn remove_stale(&self, live: &HashSet<u64>, remove: &dyn Fn(&[&str])) {
        self.known
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|account_id, tuples| {
                if live.contains(account_id) {
                    return true;
                }
                for tuple in tuples.iter() {
                    remove(&label_refs(tuple));
                }
                false
            });
    }
}

static ACCOUNT_INFO_SERIES: LazyLock<AccountSeries> = LazyLock::new(AccountSeries::default);
static ACCOUNT_SYNC_RUNNING_SERIES: LazyLock<AccountSeries> = LazyLock::new(AccountSeries::default);
static ACCOUNT_SYNC_START_SERIES: LazyLock<AccountSeries> = LazyLock::new(AccountSeries::default);
static ACCOUNT_SYNC_SUCCESS_SERIES: LazyLock<AccountSeries> = LazyLock::new(AccountSeries::default);
static ACCOUNT_SYNC_SUCCEEDED_SERIES: LazyLock<AccountSeries> = LazyLock::new(AccountSeries::default);
static ACCOUNT_SYNC_DURATION_SERIES: LazyLock<AccountSeries> = LazyLock::new(AccountSeries::default);
static ACCOUNT_SYNC_ERRORS_SERIES: LazyLock<AccountSeries> = LazyLock::new(AccountSeries::default);
static ACCOUNT_EMAILS_SERIES: LazyLock<AccountSeries> = LazyLock::new(AccountSeries::default);
static ACCOUNT_INTERVAL_SERIES: LazyLock<AccountSeries> = LazyLock::new(AccountSeries::default);
static ACCOUNT_GAPFILL_RUNNING_SERIES: LazyLock<AccountSeries> =
    LazyLock::new(AccountSeries::default);
static ACCOUNT_GAPFILL_DOWNLOADED_SERIES: LazyLock<AccountSeries> =
    LazyLock::new(AccountSeries::default);
static ACCOUNT_GAPFILL_FAILED_SERIES: LazyLock<AccountSeries> = LazyLock::new(AccountSeries::default);
static EMAILS_INGESTED_SERIES: LazyLock<AccountSeries> = LazyLock::new(AccountSeries::default);
static SYNC_ERRORS_SERIES: LazyLock<AccountSeries> = LazyLock::new(AccountSeries::default);
static SYNC_SESSIONS_SERIES: LazyLock<AccountSeries> = LazyLock::new(AccountSeries::default);
static SYNC_SESSION_DURATION_SERIES: LazyLock<AccountSeries> =
    LazyLock::new(AccountSeries::default);

/// Remove series belonging to accounts that no longer exist.
fn cleanup_stale_account_series(live: &HashSet<u64>) {
    macro_rules! cleanup {
        ($tracker:expr, $metric:expr) => {
            $tracker.remove_stale(live, &|labels| {
                let _ = $metric.remove_label_values(labels);
            });
        };
    }

    cleanup!(ACCOUNT_INFO_SERIES, ACCOUNT_INFO);
    cleanup!(ACCOUNT_SYNC_RUNNING_SERIES, ACCOUNT_SYNC_RUNNING);
    cleanup!(ACCOUNT_SYNC_START_SERIES, ACCOUNT_LAST_SYNC_START_TIMESTAMP_SECONDS);
    cleanup!(
        ACCOUNT_SYNC_SUCCESS_SERIES,
        ACCOUNT_LAST_SYNC_SUCCESS_TIMESTAMP_SECONDS
    );
    cleanup!(
        ACCOUNT_SYNC_SUCCEEDED_SERIES,
        ACCOUNT_LAST_SYNC_SUCCEEDED
    );
    cleanup!(
        ACCOUNT_SYNC_DURATION_SERIES,
        ACCOUNT_LAST_SYNC_DURATION_SECONDS
    );
    cleanup!(ACCOUNT_SYNC_ERRORS_SERIES, ACCOUNT_LAST_SYNC_ERRORS);
    cleanup!(ACCOUNT_EMAILS_SERIES, ACCOUNT_EMAILS_TOTAL);
    cleanup!(ACCOUNT_INTERVAL_SERIES, ACCOUNT_DOWNLOAD_INTERVAL_MINUTES);
    cleanup!(ACCOUNT_GAPFILL_RUNNING_SERIES, ACCOUNT_GAPFILL_RUNNING);
    cleanup!(ACCOUNT_GAPFILL_DOWNLOADED_SERIES, ACCOUNT_LAST_GAPFILL_DOWNLOADED);
    cleanup!(ACCOUNT_GAPFILL_FAILED_SERIES, ACCOUNT_LAST_GAPFILL_FAILED);
    cleanup!(EMAILS_INGESTED_SERIES, EMAILS_INGESTED);
    cleanup!(SYNC_ERRORS_SERIES, SYNC_ERRORS);
    cleanup!(SYNC_SESSIONS_SERIES, SYNC_SESSIONS);
    cleanup!(SYNC_SESSION_DURATION_SERIES, SYNC_SESSION_DURATION_SECONDS);
}

// ---------------------------------------------------------------------------
// TTL cache for archive aggregates
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct DashboardCache {
    refreshed_at: Instant,
    emails_total: u64,
    attachments_total: u64,
    storage_bytes: u64,
    index_bytes: u64,
    emails_by_account: BTreeMap<u64, u64>,
}

static DASHBOARD_CACHE: LazyLock<tokio::sync::Mutex<Option<DashboardCache>>> =
    LazyLock::new(|| tokio::sync::Mutex::new(None));
const DASHBOARD_CACHE_TTL: Duration = Duration::from_secs(60);

fn compute_dashboard_cache() -> BichonResult<DashboardCache> {
    let storage_bytes = get_total_size(&DATA_DIR_MANAGER.blob_dir())
        .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
    let index_bytes = get_total_size(&DATA_DIR_MANAGER.index_dir)
        .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
    Ok(DashboardCache {
        refreshed_at: Instant::now(),
        emails_total: ENVELOPE_MANAGER.total_emails(&None)?,
        attachments_total: ATTACHMENT_MANAGER.total_attachments(&None)?,
        storage_bytes,
        index_bytes,
        emails_by_account: ENVELOPE_MANAGER.total_emails_by_account()?,
    })
}

/// Refresh the aggregates when older than the TTL. A failed refresh serves
/// the stale cache instead of erroring the whole scrape.
async fn refresh_dashboard_cache() -> Option<DashboardCache> {
    let mut cache = DASHBOARD_CACHE.lock().await;
    if let Some(current) = cache.as_ref() {
        if current.refreshed_at.elapsed() < DASHBOARD_CACHE_TTL {
            return Some(current.clone());
        }
    }

    match tokio::task::spawn_blocking(compute_dashboard_cache).await {
        Ok(Ok(fresh)) => {
            *cache = Some(fresh.clone());
            Some(fresh)
        }
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "metrics: failed to refresh archive aggregates; serving stale values");
            cache.clone()
        }
        Err(e) => {
            tracing::warn!(error = %e, "metrics: aggregate refresh task failed; serving stale values");
            cache.clone()
        }
    }
}

// ---------------------------------------------------------------------------
// Scrape
// ---------------------------------------------------------------------------

/// Register every community family and start the uptime clock. Called once
/// at server startup so `bichon_uptime_seconds` measures the real uptime.
pub fn init() {
    let _ = &*PROCESS_START;
    ensure_registered();
}

/// Force-initialize every community static so all families are registered
/// before the first scrape. Edition families are initialized by their own
/// collectors.
fn ensure_registered() {
    let _ = &*BUILD_INFO;
    let _ = &*UPTIME_SECONDS;
    let _ = &*ACCOUNTS_TOTAL;
    let _ = &*USERS_TOTAL;
    let _ = &*ACCOUNT_MAILBOX_FOLDERS_TOTAL;
    let _ = &*EMAILS_TOTAL;
    let _ = &*ATTACHMENTS_TOTAL;
    let _ = &*STORAGE_USAGE_BYTES;
    let _ = &*INDEX_USAGE_BYTES;
    let _ = &*ACCOUNT_INFO;
    let _ = &*ACCOUNT_SYNC_RUNNING;
    let _ = &*ACCOUNT_LAST_SYNC_START_TIMESTAMP_SECONDS;
    let _ = &*ACCOUNT_LAST_SYNC_SUCCESS_TIMESTAMP_SECONDS;
    let _ = &*ACCOUNT_LAST_SYNC_SUCCEEDED;
    let _ = &*ACCOUNT_LAST_SYNC_DURATION_SECONDS;
    let _ = &*ACCOUNT_LAST_SYNC_ERRORS;
    let _ = &*ACCOUNT_EMAILS_TOTAL;
    let _ = &*ACCOUNT_DOWNLOAD_INTERVAL_MINUTES;
    let _ = &*ACCOUNT_GAPFILL_RUNNING;
    let _ = &*ACCOUNT_LAST_GAPFILL_DOWNLOADED;
    let _ = &*ACCOUNT_LAST_GAPFILL_FAILED;
    let _ = &*EMAILS_INGESTED;
    let _ = &*SYNC_ERRORS;
    let _ = &*SYNC_SESSIONS;
    let _ = &*SYNC_SESSION_DURATION_SECONDS;
    let _ = &*HTTP_REQUESTS;
    let _ = &*HTTP_REQUEST_DURATION_SECONDS;
    let _ = &*SMTP_MESSAGES;
    let _ = &*SMTP_REJECTIONS;
    let _ = &*SMTP_ACTIVE_CONNECTIONS;
    let _ = &*BACKUP_RUNS;
    let _ = &*BACKUP_DURATION_SECONDS;
    let _ = &*BACKUP_LAST_SUCCESS_TIMESTAMP_SECONDS;
    let _ = &*OAUTH2_TOKEN_REFRESH;
}

fn encode_registry() -> BichonResult<String> {
    TextEncoder::new()
        .encode_to_string(&REGISTRY.gather())
        .map_err(|e| raise_error!(format!("failed to encode metrics: {e}"), ErrorCode::InternalError))
}

/// Render the full exposition document. Refreshes state gauges from the
/// database, refreshes TTL-cached aggregates, runs edition collectors, and
/// encodes the registry.
pub async fn scrape() -> BichonResult<String> {
    ensure_registered();

    BUILD_INFO.with_label_values(&[bichon_version!()]).set(1.0);
    UPTIME_SECONDS.set(PROCESS_START.elapsed().as_secs_f64());

    let accounts = AccountModel::list_all().map_err(|e| {
        tracing::warn!(error = %e, "metrics: failed to list accounts");
        e
    });

    let cache = refresh_dashboard_cache().await;
    if let Some(cache) = cache.as_ref() {
        EMAILS_TOTAL.set(cache.emails_total as f64);
        ATTACHMENTS_TOTAL.set(cache.attachments_total as f64);
        STORAGE_USAGE_BYTES.set(cache.storage_bytes as f64);
        INDEX_USAGE_BYTES.set(cache.index_bytes as f64);
    }

    match &accounts {
        Ok(accounts) => refresh_account_metrics(accounts, cache.as_ref()),
        Err(_) => {
            // Keep the previous account series rather than clearing them on a
            // transient database failure.
        }
    }

    match UserModel::list_all() {
        Ok(users) => USERS_TOTAL.set(users.len() as f64),
        Err(e) => tracing::warn!(error = %e, "metrics: failed to list users"),
    }

    init_backup_last_success();
    refresh_collectors();

    if let Ok(accounts) = &accounts {
        let live: HashSet<u64> = accounts.iter().map(|account| account.id).collect();
        cleanup_stale_account_series(&live);
    }

    encode_registry()
}

fn refresh_account_metrics(
    accounts: &[AccountModel],
    cache: Option<&DashboardCache>,
) {
    let mut imap = 0usize;
    let mut nosync = 0usize;
    let mut view = 0usize;
    let mut folders = 0usize;

    for account in accounts {
        match account.account_type {
            AccountType::IMAP => imap += 1,
            AccountType::NoSync => nosync += 1,
            AccountType::View => view += 1,
        }
        folders += account.known_folders.as_ref().map_or(0, |f| f.len());

        let id = account.id.to_string();
        let account_type = account_type_label(&account.account_type);
        let name = account.account_name.as_deref().unwrap_or(&account.email);
        let enabled = if account.enabled { "true" } else { "false" };

        ACCOUNT_INFO_SERIES.replace(
            account.id,
            &[&id, &account.email, name, account_type, enabled],
            &|labels| {
                let _ = ACCOUNT_INFO.remove_label_values(labels);
            },
        );
        ACCOUNT_INFO
            .with_label_values(&[&id, &account.email, name, account_type, enabled])
            .set(1.0);

        let interval = account.download_interval_min.unwrap_or(0) as f64;
        ACCOUNT_DOWNLOAD_INTERVAL_MINUTES
            .with_label_values(&[&id])
            .set(interval);
        ACCOUNT_INTERVAL_SERIES.set(account.id, &[&id]);

        if let Some(cache) = cache {
            // Views own no documents, so they have no stored-mail series.
            if !matches!(account.account_type, AccountType::View) {
                let count = cache.emails_by_account.get(&account.id).copied().unwrap_or(0);
                ACCOUNT_EMAILS_TOTAL
                    .with_label_values(&[&id])
                    .set(count as f64);
                ACCOUNT_EMAILS_SERIES.set(account.id, &[&id]);
            }
        }

        refresh_download_state(account.id, &id);
        refresh_gapfill_state(account.id, &id);
    }

    ACCOUNTS_TOTAL.with_label_values(&["imap"]).set(imap as f64);
    ACCOUNTS_TOTAL
        .with_label_values(&["nosync"])
        .set(nosync as f64);
    ACCOUNTS_TOTAL.with_label_values(&["view"]).set(view as f64);
    ACCOUNT_MAILBOX_FOLDERS_TOTAL.set(folders as f64);
}

fn refresh_download_state(account_id: u64, id: &str) {
    let state = match DownloadState::get(account_id) {
        Ok(Some(state)) => state,
        Ok(None) => return,
        Err(e) => {
            tracing::warn!(
                account_id,
                error = %e,
                "metrics: failed to read download state; skipping account"
            );
            return;
        }
    };

    let running = state
        .active_session
        .as_ref()
        .is_some_and(|session| session.status == DownloadStatus::Running);
    ACCOUNT_SYNC_RUNNING
        .with_label_values(&[id])
        .set(if running { 1.0 } else { 0.0 });
    ACCOUNT_SYNC_RUNNING_SERIES.set(account_id, &[id]);

    let last_start = state
        .active_session
        .as_ref()
        .map(|session| session.start_time)
        .or_else(|| state.history.last().map(|session| session.start_time));
    if let Some(started_at) = last_start {
        ACCOUNT_LAST_SYNC_START_TIMESTAMP_SECONDS
            .with_label_values(&[id])
            .set(ms_to_seconds(started_at));
        ACCOUNT_SYNC_START_SERIES.set(account_id, &[id]);
    }

    let last_success = state
        .history
        .iter()
        .rev()
        .find(|session| session.status == DownloadStatus::Success)
        .and_then(|session| session.end_time);
    ACCOUNT_LAST_SYNC_SUCCESS_TIMESTAMP_SECONDS
        .with_label_values(&[id])
        .set(last_success.map(ms_to_seconds).unwrap_or(0.0));
    ACCOUNT_SYNC_SUCCESS_SERIES.set(account_id, &[id]);

    if let Some(last) = state.history.last() {
        ACCOUNT_LAST_SYNC_SUCCEEDED
            .with_label_values(&[id])
            .set(if last.status == DownloadStatus::Success {
                1.0
            } else {
                0.0
            });
        ACCOUNT_SYNC_SUCCEEDED_SERIES.set(account_id, &[id]);

        if let Some(end) = last.end_time {
            ACCOUNT_LAST_SYNC_DURATION_SECONDS
                .with_label_values(&[id])
                .set((end - last.start_time).max(0) as f64 / 1000.0);
            ACCOUNT_SYNC_DURATION_SERIES.set(account_id, &[id]);
        }

        ACCOUNT_LAST_SYNC_ERRORS
            .with_label_values(&[id])
            .set(last.errors.len() as f64);
        ACCOUNT_SYNC_ERRORS_SERIES.set(account_id, &[id]);
    }
}

fn refresh_gapfill_state(account_id: u64, id: &str) {
    let state = match GapFillState::get(account_id) {
        Ok(Some(state)) => state,
        Ok(None) => return,
        Err(e) => {
            tracing::warn!(
                account_id,
                error = %e,
                "metrics: failed to read gap-fill state; skipping account"
            );
            return;
        }
    };

    let running = state
        .active
        .as_ref()
        .is_some_and(|run| run.status == crate::account::state::GapFillStatus::Running);
    ACCOUNT_GAPFILL_RUNNING
        .with_label_values(&[id])
        .set(if running { 1.0 } else { 0.0 });
    ACCOUNT_GAPFILL_RUNNING_SERIES.set(account_id, &[id]);

    if let Some(last) = state.history.last() {
        ACCOUNT_LAST_GAPFILL_DOWNLOADED
            .with_label_values(&[id])
            .set(last.downloaded as f64);
        ACCOUNT_GAPFILL_DOWNLOADED_SERIES.set(account_id, &[id]);
        ACCOUNT_LAST_GAPFILL_FAILED
            .with_label_values(&[id])
            .set(last.failed as f64);
        ACCOUNT_GAPFILL_FAILED_SERIES.set(account_id, &[id]);
    }
}

fn account_type_label(account_type: &AccountType) -> &'static str {
    match account_type {
        AccountType::IMAP => "imap",
        AccountType::NoSync => "nosync",
        AccountType::View => "view",
    }
}

fn ms_to_seconds(ms: i64) -> f64 {
    ms.max(0) as f64 / 1000.0
}

// ---------------------------------------------------------------------------
// Backup last-success gauge
// ---------------------------------------------------------------------------

static BACKUP_LAST_SUCCESS: LazyLock<Mutex<Option<i64>>> = LazyLock::new(|| Mutex::new(None));
static BACKUP_LAST_SUCCESS_INIT: OnceLock<()> = OnceLock::new();

/// Initialize the last-success gauge from the persisted backup history once
/// per process, so a restart does not reset the primary "are backups
/// happening?" alerting signal to 0.
fn init_backup_last_success() {
    if BACKUP_LAST_SUCCESS_INIT.set(()).is_err() {
        return;
    }
    let mut last = BACKUP_LAST_SUCCESS.lock().unwrap_or_else(|e| e.into_inner());
    if last.is_some() {
        return;
    }
    let Some(record) = BackupRecord::last_success_for(&backup_config::destination()) else {
        return;
    };
    let Some(finished_at) = record.finished_at else {
        return;
    };
    *last = Some(finished_at);
    BACKUP_LAST_SUCCESS_TIMESTAMP_SECONDS.set(ms_to_seconds(finished_at));
}

/// Record the outcome of a finished backup run.
pub fn record_backup_run(success: bool, duration_ms: i64) {
    BACKUP_RUNS
        .with_label_values(&[if success { "success" } else { "failed" }])
        .inc();
    BACKUP_DURATION_SECONDS.observe(duration_ms.max(0) as f64 / 1000.0);
    if success {
        let finished_at = utc_now!();
        *BACKUP_LAST_SUCCESS.lock().unwrap_or_else(|e| e.into_inner()) = Some(finished_at);
        BACKUP_LAST_SUCCESS_TIMESTAMP_SECONDS.set(ms_to_seconds(finished_at));
    }
}

// ---------------------------------------------------------------------------
// Instrumentation helpers — sync, cheap, non-failing
// ---------------------------------------------------------------------------

/// Count one message processed by the ingestion pipeline.
pub fn count_email_ingested(account_id: u64, source: &str, outcome: &str) {
    let id = account_id.to_string();
    EMAILS_INGESTED
        .with_label_values(&[&id, source, outcome])
        .inc();
    EMAILS_INGESTED_SERIES.set(account_id, &[&id, source, outcome]);
}

/// Count one classified sync/download error.
pub fn count_sync_error(account_id: u64, kind: classify::ErrorKind) {
    let id = account_id.to_string();
    SYNC_ERRORS.with_label_values(&[&id, kind.as_str()]).inc();
    SYNC_ERRORS_SERIES.set(account_id, &[&id, kind.as_str()]);
}

/// Record a finished download session (session counter and duration).
pub fn count_sync_session(account_id: u64, trigger: &str, status: &str, duration_ms: i64) {
    let id = account_id.to_string();
    SYNC_SESSIONS
        .with_label_values(&[&id, trigger, status])
        .inc();
    SYNC_SESSIONS_SERIES.set(account_id, &[&id, trigger, status]);
    SYNC_SESSION_DURATION_SECONDS
        .with_label_values(&[&id])
        .observe(duration_ms.max(0) as f64 / 1000.0);
    SYNC_SESSION_DURATION_SERIES.set(account_id, &[&id]);
}

/// Record one REST API request (method/status-class counter and latency).
pub fn count_http_request(method: &str, status_class: &str, seconds: f64) {
    HTTP_REQUESTS
        .with_label_values(&[method, status_class])
        .inc();
    HTTP_REQUEST_DURATION_SECONDS
        .with_label_values(&[method])
        .observe(seconds);
}

/// Count one SMTP message processed by the receiver.
pub fn count_smtp_message(outcome: &str) {
    SMTP_MESSAGES.with_label_values(&[outcome]).inc();
}

/// Count one SMTP message rejected before archiving (also counted as a
/// rejected message).
pub fn count_smtp_rejection(reason: &str) {
    count_smtp_message("rejected");
    SMTP_REJECTIONS.with_label_values(&[reason]).inc();
}

/// Keeps `bichon_smtp_active_connections` accurate for the lifetime of one
/// SMTP connection, including error and panic paths.
#[must_use = "the connection gauge is decremented when the guard is dropped"]
pub struct SmtpConnectionGuard;

impl SmtpConnectionGuard {
    pub fn new() -> Self {
        SMTP_ACTIVE_CONNECTIONS.inc();
        Self
    }
}

impl Default for SmtpConnectionGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for SmtpConnectionGuard {
    fn drop(&mut self) {
        SMTP_ACTIVE_CONNECTIONS.dec();
    }
}

/// Count one OAuth2 access-token refresh attempt.
pub fn count_oauth2_refresh(outcome: &str) {
    OAUTH2_TOKEN_REFRESH.with_label_values(&[outcome]).inc();
}

/// Map the persisted trigger of a download session to its label value.
pub fn trigger_label(trigger: &TriggerType) -> &'static str {
    match trigger {
        TriggerType::Manual => "manual",
        TriggerType::Scheduled => "scheduled",
        TriggerType::SyncFull => "sync_full",
    }
}

/// Map the persisted status of a download session to its label value.
///
/// Only terminal states reach the counter; `Running` is mapped to `cancelled`
/// defensively (it should never be recorded as a finished session).
pub fn download_status_label(status: &DownloadStatus) -> &'static str {
    match status {
        DownloadStatus::Success => "success",
        DownloadStatus::Partial => "partial",
        DownloadStatus::Failed => "failed",
        DownloadStatus::Running | DownloadStatus::Cancelled => "cancelled",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct GaugeCollector {
        name: &'static str,
        gauge: Gauge,
        refresh_count: AtomicUsize,
        should_panic: bool,
    }

    impl MetricsCollector for GaugeCollector {
        fn name(&self) -> &'static str {
            self.name
        }

        fn refresh(&self) {
            self.refresh_count.fetch_add(1, Ordering::SeqCst);
            self.gauge.set(42.0);
            if self.should_panic {
                panic!("collector failure");
            }
        }
    }

    #[test]
    fn encoding_contains_help_type_and_escaped_labels() {
        let counter = registered(
            IntCounterVec::new(
                Opts::new("test_encoder_total", "Help text for the encoder test."),
                &["label"],
            )
            .unwrap(),
        );
        counter
            .with_label_values(&["quote\"backslash\\newline\n"])
            .inc();

        let text = encode_registry().expect("encode registry");
        assert!(text.contains("# HELP test_encoder_total Help text for the encoder test."));
        assert!(text.contains("# TYPE test_encoder_total counter"));
        assert!(text.contains("quote\\\"backslash\\\\newline\\n"));
    }

    #[test]
    fn collector_refresh_updates_the_registry() {
        let gauge = registered(Gauge::new("test_collector_gauge", "Collector test gauge.").unwrap());
        register_metrics_collector(Box::new(GaugeCollector {
            name: "test-collector",
            gauge: gauge.clone(),
            refresh_count: AtomicUsize::new(0),
            should_panic: false,
        }));

        refresh_collectors();
        assert_eq!(gauge.get(), 42.0);
        let text = encode_registry().expect("encode registry");
        assert!(text.contains("test_collector_gauge 42"));
    }

    #[test]
    fn a_panicking_collector_does_not_break_the_scrape() {
        let gauge = registered(Gauge::new(
            "test_panicking_collector_gauge",
            "Panicking collector test gauge.",
        ).unwrap());
        register_metrics_collector(Box::new(GaugeCollector {
            name: "test-panicking-collector",
            gauge,
            refresh_count: AtomicUsize::new(0),
            should_panic: true,
        }));

        // Must return normally even though the collector panics.
        refresh_collectors();
        encode_registry().expect("a panicking collector must not fail the scrape");
    }

    #[test]
    fn sync_error_counter_uses_the_contract_labels() {
        // A unique account id keeps the assertion stable if other tests (or a
        // rerun in the same process) also increment the family.
        let account_id = 9_876_543_210u64;
        count_sync_error(account_id, classify::ErrorKind::Auth);

        let text = encode_registry().expect("encode registry");
        assert!(text.contains(&format!(
            "bichon_sync_errors_total{{account_id=\"{account_id}\",kind=\"auth\"}} 1"
        )));
    }

    #[test]
    fn per_account_series_are_removed_for_deleted_accounts() {
        let series = AccountSeries::default();
        series.set(1, &["1"]);
        series.set(2, &["2", "imap", "imported"]);
        series.set(2, &["2", "smtp", "failed"]);

        let removed = std::cell::RefCell::new(Vec::new());
        let live: HashSet<u64> = [1u64].into_iter().collect();
        series.remove_stale(&live, &|labels| {
            removed
                .borrow_mut()
                .push(labels.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        });

        let removed = removed.into_inner();
        assert_eq!(removed.len(), 2, "every tuple of a deleted account is removed");
        assert!(removed.contains(&vec![
            "2".to_string(),
            "imap".to_string(),
            "imported".to_string()
        ]));
        assert!(removed.contains(&vec![
            "2".to_string(),
            "smtp".to_string(),
            "failed".to_string()
        ]));
    }

    #[test]
    fn replacing_a_label_tuple_removes_the_previous_one() {
        let series = AccountSeries::default();
        series.replace(1, &["1", "old@example.com"], &|_| {});
        assert!(series.known.lock().unwrap()[&1]
            .iter()
            .any(|tuple| tuple == &vec!["1".to_string(), "old@example.com".to_string()]));

        let removed = std::cell::RefCell::new(Vec::new());
        series.replace(1, &["1", "new@example.com"], &|labels| {
            removed.borrow_mut().push(labels.join(","));
        });

        assert_eq!(*removed.borrow(), vec!["1,old@example.com"]);
        assert_eq!(
            series.known.lock().unwrap()[&1],
            vec![vec!["1".to_string(), "new@example.com".to_string()]]
        );
    }
}
