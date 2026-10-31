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

use crate::raise_error;
use crate::{
    common::signal::SIGNAL_MANAGER,
    envelope::extractor::reattach_eml_content_self_healing,
    error::{code::ErrorCode, BichonResult},
    settings::dir::DATA_DIR_MANAGER,
};
use bichon_blob::{Codec, Config, Engine};
use bytes::Bytes;

use std::io::Cursor;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::sync::LazyLock;
use std::time::Duration;
use tokio::{
    sync::{mpsc, Mutex},
    task::{self, JoinHandle},
};

pub static BLOB_MANAGER: LazyLock<BlobManager> = LazyLock::new(BlobManager::new);

/// Free-space floor on the blob volume. Writes that would breach it are
/// refused, and the watchdog requests a graceful shutdown when the volume
/// falls below it.
pub const MIN_FREE_BYTES: u64 = 1024 * 1024 * 1024;

/// How often the blob engine checks the volume against [`MIN_FREE_BYTES`].
pub const DISK_CHECK_INTERVAL_SECS: u64 = 5;

pub struct DetachedEmail {
    pub email: (String, Bytes),
    pub attachments: Option<Vec<(String, Bytes)>>,
}

pub struct BlobManager {
    sender: mpsc::Sender<DetachedEmail>,
    engine: Arc<Engine>,
    handle: Mutex<Option<JoinHandle<()>>>,
    /// Emails queued but not yet written by the background loop. Tracks the
    /// boundary the backup manager drains before sealing the blob store.
    pending: Arc<AtomicUsize>,
}

fn hex_to_key(hex: &str) -> BichonResult<[u8; 32]> {
    let mut key = [0u8; 32];
    hex::decode_to_slice(hex, &mut key).map_err(|e| {
        raise_error!(
            format!("invalid content hash '{hex}': {e:#?}"),
            ErrorCode::InternalError
        )
    })?;
    Ok(key)
}

impl BlobManager {
    pub async fn shutdown(&self) {
        let mut guard = self.handle.lock().await;
        if let Some(handle) = guard.take() {
            let _ = handle.await;
        }
        if let Err(e) = self.engine.shutdown() {
            tracing::error!("blob engine shutdown error: {}", e);
        }
    }

    fn process_detached_email(eml: DetachedEmail, engine: &Engine) {
        let (email_hash, email_data) = eml.email;
        let email_key = match hex_to_key(&email_hash) {
            Ok(k) => k,
            Err(e) => {
                tracing::error!("{:#?}", e);
                return;
            }
        };
        match engine.exists(&email_key) {
            Ok(false) => {
                if let Err(e) = engine.put(email_key, &email_data, Codec::Lz4) {
                    tracing::error!("CRITICAL: Failed to insert email blob: {:?}", e);
                }
            }
            Err(e) => tracing::error!("blob engine error: {:?}", e),
            Ok(true) => {
                tracing::debug!("Email blob already exists (dedup): {}", &email_hash);
            }
        }

        if let Some(attachments) = eml.attachments {
            for (a_hash, a_data) in attachments {
                let a_key = match hex_to_key(&a_hash) {
                    Ok(k) => k,
                    Err(e) => {
                        tracing::error!("{:#?}", e);
                        continue;
                    }
                };
                match engine.exists(&a_key) {
                    Ok(false) => {
                        if let Err(e) = engine.put(a_key, &a_data, Codec::Lz4) {
                            tracing::error!("CRITICAL: Failed to insert attachment blob: {:?}", e);
                        }
                    }
                    Err(e) => tracing::error!("blob engine error: {:?}", e),
                    Ok(true) => {
                        tracing::debug!("Attachment blob already exists (dedup): {}", &a_hash);
                    }
                }
            }
        }
    }

    pub fn new() -> Self {
        Self::open(DATA_DIR_MANAGER.storage_dir.join("blobs"))
    }

    /// Opens a manager rooted at `blob_dir`. Split out from [`Self::new`] so a
    /// test can run against a private directory: the writer task below is bound
    /// to whichever runtime created the manager, so a test that used the
    /// process-global `BLOB_MANAGER` would silently depend on which other test
    /// initialized it first and on that test's runtime still being alive.
    fn open(blob_dir: std::path::PathBuf) -> Self {
        let mut config = Config::default();
        config.default_codec = Codec::Zstd;
        config.compress_threshold = 1024;
        config.flush_interval_secs = 60;
        config.gc_interval_secs = 300;
        // Leave 1 GiB of headroom: refuse writes that would breach it, and
        // request a graceful shutdown if the volume falls below it. The
        // shutdown path is still allowed to flush queued blobs into the
        // reserve; a genuinely full volume will surface as write errors.
        config.min_free_bytes = MIN_FREE_BYTES;
        config.disk_check_interval_secs = DISK_CHECK_INTERVAL_SECS;

        let engine = Engine::open(&blob_dir, config)
            .expect("Failed to initialize blob engine: Check disk space and permissions.");

        let engine = Arc::new(engine);

        // Low disk does not kill the process: it requests the normal graceful
        // shutdown, and the drain path below is allowed to keep writing even
        // though the volume is below the reserve.
        engine.set_low_disk_callback({
            let engine = Arc::downgrade(&engine);
            move || {
                tracing::error!(
                    "Blob storage is below the free-space floor; requesting a graceful shutdown"
                );
                if let Some(engine) = engine.upgrade() {
                    engine.allow_writes_below_free_floor(true);
                }
                SIGNAL_MANAGER.trigger_shutdown();
            }
        });

        let (sender, mut receiver) = mpsc::channel::<DetachedEmail>(100);

        let pending = Arc::new(AtomicUsize::new(0));
        let pending_bg = Arc::clone(&pending);
        let engine_bg = Arc::clone(&engine);
        let handler = task::spawn(async move {
            let mut shutdown = SIGNAL_MANAGER.subscribe();
            loop {
                tokio::select! {
                    res = receiver.recv() => {
                        match res {
                            Some(eml) => {
                                let mut batch = vec![eml];
                                while let Ok(next_eml) = receiver.try_recv() {
                                    batch.push(next_eml);
                                }
                                let batch_len = batch.len();
                                let engine_bg = Arc::clone(&engine_bg);
                                if let Err(e) = tokio::task::spawn_blocking(move || {
                                    for eml in batch {
                                        Self::process_detached_email(eml, &engine_bg);
                                    }
                                }).await {
                                    tracing::error!("BlobManager: spawn_blocking join error: {:#?}", e);
                                }
                                pending_bg.fetch_sub(batch_len, Ordering::AcqRel);
                            }
                            None => {
                                tracing::info!("BlobManager: All senders dropped, closing blob storage.");
                                break;
                            }
                        }
                    }
                    _ = shutdown.recv() => {
                        // Graceful shutdown: queued blobs must still be
                        // flushed even if the volume is below the reserve.
                        engine_bg.allow_writes_below_free_floor(true);
                        receiver.close();
                        let mut remaining = Vec::new();
                        while let Some(eml) = receiver.recv().await {
                            remaining.push(eml);
                        }
                        tracing::info!(
                            "BlobManager: Shutdown signal received. Processing {} remaining tasks...",
                            remaining.len()
                        );
                        if !remaining.is_empty() {
                            let batch_len = remaining.len();
                            let engine_bg = Arc::clone(&engine_bg);
                            if let Err(e) = tokio::task::spawn_blocking(move || {
                                for eml in remaining {
                                    Self::process_detached_email(eml, &engine_bg);
                                }
                            }).await {
                                tracing::error!("BlobManager: shutdown spawn_blocking join error: {:#?}", e);
                            }
                            pending_bg.fetch_sub(batch_len, Ordering::AcqRel);
                        }
                        tracing::info!("BlobManager: All remaining tasks processed. Closing blob engine.");
                        break;
                    }
                }
            }
        });

        Self {
            sender,
            engine,
            pending,
            handle: Mutex::new(Some(handler)),
        }
    }

    pub async fn queue(&self, email: DetachedEmail) {
        self.pending.fetch_add(1, Ordering::AcqRel);
        if let Err(e) = self.sender.send(email).await {
            self.pending.fetch_sub(1, Ordering::AcqRel);
            tracing::error!("BlobManager channel closed, email lost: {:#?}", e);
        }
    }

    /// Number of emails queued but not yet persisted to the blob store.
    pub fn pending(&self) -> usize {
        self.pending.load(Ordering::Acquire)
    }

    /// Wait until every queued email has been written to disk. Called inside a
    /// backup write window (after the write gate has drained), so no new items
    /// are queued while we wait.
    pub async fn drain(&self) {
        while self.pending.load(Ordering::Acquire) != 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// Bring the blob store into a copy-safe, fully durable state: pause GC,
    /// seal the active segment, fsync every segment and flush the redb index.
    /// Callers must have paused the write gate and drained both in-flight
    /// extractors and this queue first.
    pub fn prepare_for_backup(&self) -> BichonResult<()> {
        self.engine
            .prepare_for_backup()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))
    }

    /// Re-enable the blob GC after a backup window closes.
    pub fn resume_gc(&self) {
        self.engine.set_gc_paused(false);
    }

    /// Every sealed segment as a backup artifact (call after
    /// [`BlobManager::prepare_for_backup`], while GC is paused). Missing
    /// hashes are computed and persisted here, so a capture window only ever
    /// pays for first-time hashes.
    pub fn backup_segments(&self) -> BichonResult<Vec<bichon_blob::engine::SegmentArtifact>> {
        self.engine
            .backup_segments()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))
    }

    pub fn get_email(&self, content_hash: &str) -> BichonResult<Option<Bytes>> {
        self.get(content_hash)
    }

    pub fn get_attachment(&self, content_hash: &str) -> BichonResult<Option<Bytes>> {
        self.get(content_hash)
    }

    fn get(&self, content_hash: &str) -> BichonResult<Option<Bytes>> {
        let key = hex_to_key(content_hash)?;
        self.engine
            .get(&key)
            .map(|v| v.map(Bytes::from))
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))
    }

    pub fn delete<I1, I2>(
        &self,
        email_content_hashes: I1,
        attachment_content_hashes: I2,
    ) -> BichonResult<()>
    where
        I1: IntoIterator,
        I1::Item: AsRef<str>,
        I2: IntoIterator,
        I2::Item: AsRef<str>,
    {
        let mut keys: Vec<[u8; 32]> = email_content_hashes
            .into_iter()
            .map(|h| hex_to_key(h.as_ref()))
            .collect::<BichonResult<_>>()?;

        for h in attachment_content_hashes {
            keys.push(hex_to_key(h.as_ref())?);
        }

        if !keys.is_empty() {
            self.engine
                .delete_batch(&keys)
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        }

        Ok(())
    }
}

/// Returns a reader over the raw EML for an indexed message.
///
/// If the message's content blob is missing from the blob store, it is fetched
/// on demand from the IMAP server, persisted, and returned (self-healing). The
/// underlying "content not found" error is only surfaced if that on-demand
/// fetch itself fails.
pub async fn get_reader(account_id: u64, eid: String) -> BichonResult<Cursor<Bytes>> {
    let (_, data) = reattach_eml_content_self_healing(account_id, eid).await?;
    Ok(Cursor::new(data))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A private manager rooted at its own temp dir. The process-global
    /// `BLOB_MANAGER` is deliberately avoided: its writer is a spawned task
    /// bound to whichever runtime first dereferenced the `LazyLock`, so sharing
    /// it across test modules makes the outcome depend on execution order (and
    /// on that first runtime still being alive).
    fn temp_manager(suffix: &str) -> (BlobManager, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("bichon-blob-{suffix}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        (BlobManager::open(dir.clone()), dir)
    }

    /// Drop the manager and let the writer loop notice the closed channel, so
    /// the engine releases its file handles before the temp dir is removed.
    async fn close(manager: BlobManager, dir: std::path::PathBuf) -> std::path::PathBuf {
        drop(manager);
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
        dir
    }

    /// First write wins: re-queueing an existing key must not replace the
    /// archived bytes. This is what makes a repeated detach (re-ingest, repair)
    /// idempotent rather than a way to overwrite the archive.
    #[test]
    fn queued_blobs_are_deduplicated_and_never_overwritten() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let dir = runtime.block_on(async {
            let (manager, dir) = temp_manager("dedup");

            let email_hash = crate::utils::compute_content_hash(b"original email");
            let attachment_hash = crate::utils::compute_content_hash(b"original attachment");

            manager
                .queue(DetachedEmail {
                    email: (email_hash.clone(), Bytes::from_static(b"original email")),
                    attachments: Some(vec![(
                        attachment_hash.clone(),
                        Bytes::from_static(b"original attachment"),
                    )]),
                })
                .await;
            manager.drain().await;
            assert_eq!(manager.pending(), 0);

            assert_eq!(
                manager.get_email(&email_hash).unwrap(),
                Some(Bytes::from_static(b"original email"))
            );
            assert_eq!(
                manager.get_attachment(&attachment_hash).unwrap(),
                Some(Bytes::from_static(b"original attachment"))
            );

            // Same keys, different bytes.
            manager
                .queue(DetachedEmail {
                    email: (email_hash.clone(), Bytes::from_static(b"tampered email")),
                    attachments: Some(vec![(
                        attachment_hash.clone(),
                        Bytes::from_static(b"tampered attachment"),
                    )]),
                })
                .await;
            manager.drain().await;

            assert_eq!(
                manager.get_email(&email_hash).unwrap(),
                Some(Bytes::from_static(b"original email"))
            );
            assert_eq!(
                manager.get_attachment(&attachment_hash).unwrap(),
                Some(Bytes::from_static(b"original attachment"))
            );

            close(manager, dir).await
        });
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A malformed hash (a corrupt envelope field) must be dropped on the floor
    /// rather than panic — and must not take the rest of the batch with it.
    #[test]
    fn a_malformed_hash_is_skipped_without_dropping_the_batch() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let dir = runtime.block_on(async {
            let (manager, dir) = temp_manager("badkey");

            let good_hash = crate::utils::compute_content_hash(b"good email");
            manager
                .queue(DetachedEmail {
                    email: ("not-hex".to_string(), Bytes::from_static(b"junk")),
                    attachments: None,
                })
                .await;
            manager
                .queue(DetachedEmail {
                    email: (good_hash.clone(), Bytes::from_static(b"good email")),
                    attachments: None,
                })
                .await;
            manager.drain().await;
            assert_eq!(manager.pending(), 0);

            assert_eq!(
                manager.get_email(&good_hash).unwrap(),
                Some(Bytes::from_static(b"good email"))
            );

            close(manager, dir).await
        });
        let _ = std::fs::remove_dir_all(&dir);
    }
}
