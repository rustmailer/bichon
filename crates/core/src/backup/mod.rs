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

//! Built-in zero-downtime backups.
//!
//! Bichon owns every byte of its data on local disk (Tantivy indices, the
//! bichon-blob content store, and the memdb snapshot/WAL). Backups are
//! **two-phase** (design doc §5): a short capture window produces immutable
//! artifacts, then a content-addressed upload persists them without touching
//! the write gate. The destination is an S3-compatible object store or a
//! local directory ([`engine::BackupTarget`]) — everything below the target
//! is backend-agnostic:
//!
//! 1. **Preflight** — [`backend::BackupBackend::preflight`] verifies the
//!    target before anything is paused.
//! 2. **Capture** — [`gate::WRITE_GATE`] is paused, every in-flight write
//!    drains, and each registered contributor
//!    ([`prepare::BackupPreparer`]) delivers its exact products as
//!    [`artifact::Artifact`]s. The gate reopens the moment capture finishes,
//!    so the server keeps receiving mail for the whole upload.
//! 3. **Upload** — [`backend::BackupBackend::upload`] content-addresses the
//!    artifacts (object key = sha256), commits a restore-point manifest and
//!    applies retention — all with the gate open ([`engine`]).
//! 4. **Finalize** — the window is closed unconditionally, even on failure or
//!    panic ([`manager`]).
//!
//! The community edition registers four preparers (memdb, blob, envelope
//! index, attachment index) via [`prepare::register_base_preparers`]; Pro and
//! enterprise editions append their own via [`prepare::register_preparer`].
//!
//! [`init`] wires everything up (preparers + scheduler) and is called from
//! the shared `BichonContext::initialize()` hook.

pub mod artifact;
pub mod backend;
pub mod config;
pub mod engine;
pub mod errors;
pub mod gate;
pub mod manager;
pub mod manifest;
pub mod model;
pub(crate) mod path;
pub mod prepare;
pub mod restore;
pub mod retention;
pub mod schedule;

/// One-time initialization of the built-in backup subsystem: registers the
/// community preparers and starts the scheduler. Called from
/// `BichonContext::initialize()`, which both the community and Pro servers
/// run through. Pro registers its extra preparers *before* this runs.
pub fn init() {
    // Stale `Running` records from a previous (crashed) process must be
    // finalized before anything can observe them — the scheduler, the records
    // API, the status view.
    manager::recover_interrupted_runs();
    prepare::register_base_preparers();
    schedule::start_scheduler();
}

/// Cross-module lock for tests that mutate the *process-global* backup state:
/// the write gate and the backup config document/cache. The `manager` machine
/// tests pause [`gate::WRITE_GATE`] for a whole run and read
/// `config::retention_policy_struct()`, while the `config` tests write through
/// the gate and replace the config cache. Those two groups cannot overlap, but
/// their per-module `RUN_TESTS` mutexes do not know about each other — so both
/// modules' tests take this shared lock instead. (Tests elsewhere that touch
/// only their own temp roots — `restore`, `engine`, `gate`, … — are unaffected.)
#[cfg(test)]
pub(crate) static BACKUP_STATE_TESTS: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));
