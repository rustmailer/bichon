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
    utils::compute_content_hash,
};
use bichon_blob::{Codec, Config, Engine};
use bytes::Bytes;

use std::{
    collections::{hash_map::Entry, HashMap, HashSet},
    io::Cursor,
    sync::Arc,
    sync::LazyLock,
};
use tokio::{
    sync::{mpsc, Mutex},
    task::{self, JoinHandle},
};

pub static BLOB_MANAGER: LazyLock<BlobManager> = LazyLock::new(BlobManager::new);

pub(crate) fn uidonly_exact_raw_blob_key(content_hash: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"bichon-uidonly-exact-raw-v1\0");
    hasher.update(content_hash.as_bytes());
    hex::encode(hasher.finalize().as_bytes())
}

pub struct DetachedEmail {
    pub email: (String, Bytes),
    pub attachments: Option<Vec<(String, Bytes)>>,
}

pub(crate) struct UidOnlyBlob {
    pub content_hash: String,
    pub raw: Vec<u8>,
    pub attachments: Vec<(String, Bytes)>,
}

pub struct BlobManager {
    sender: mpsc::Sender<DetachedEmail>,
    engine: Arc<Engine>,
    handle: Mutex<Option<JoinHandle<()>>>,
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

fn insert_uidonly_blob(
    values: &mut HashMap<[u8; 32], Vec<u8>>,
    order: &mut Vec<[u8; 32]>,
    key: [u8; 32],
    value: Vec<u8>,
    reject_mismatch: bool,
) -> BichonResult<()> {
    match values.entry(key) {
        Entry::Vacant(entry) => {
            order.push(key);
            entry.insert(value);
        }
        Entry::Occupied(entry) if reject_mismatch && entry.get() != &value => {
            return Err(raise_error!(
                "one UIDONLY blob key mapped to different bytes in a batch".into(),
                ErrorCode::InternalError
            ));
        }
        Entry::Occupied(_) => {}
    }
    Ok(())
}

fn store_uidonly_exact_batch_inner(engine: &Engine, blobs: Vec<UidOnlyBlob>) -> BichonResult<()> {
    let mut values = HashMap::new();
    let mut order = Vec::new();
    let mut exact_keys = HashSet::new();
    let mut exact_readbacks = Vec::new();

    for blob in blobs {
        if compute_content_hash(&blob.raw) != blob.content_hash {
            return Err(raise_error!(
                "UIDONLY raw bytes do not match their content hash".into(),
                ErrorCode::InternalError
            ));
        }
        let exact_key = hex_to_key(&uidonly_exact_raw_blob_key(&blob.content_hash))?;
        if exact_keys.insert(exact_key) {
            exact_readbacks.push((exact_key, blob.content_hash.clone(), blob.raw.len()));
        }
        insert_uidonly_blob(&mut values, &mut order, exact_key, blob.raw, true)?;
        for (hash, bytes) in blob.attachments {
            // Attachment keys predate UIDONLY and are based on decoded bytes,
            // while their stored MIME slices may differ in transfer encoding.
            // Preserve the first value, matching the legacy dedup behavior.
            insert_uidonly_blob(
                &mut values,
                &mut order,
                hex_to_key(&hash)?,
                bytes.to_vec(),
                false,
            )?;
        }
    }

    let existing = engine
        .exists_batch(&order)
        .map_err(|e| raise_error!(format!("{e:#?}"), ErrorCode::InternalError))?;
    let entries: Vec<_> = order
        .iter()
        .copied()
        .zip(existing)
        .filter(|(_, exists)| !exists)
        .map(|key| {
            let key = key.0;
            (
                key,
                values.remove(&key).expect("ordered UIDONLY blob"),
                Codec::Lz4,
            )
        })
        .collect();
    engine
        .put_batch(&entries)
        .map_err(|e| raise_error!(format!("{e:#?}"), ErrorCode::InternalError))?;
    drop(entries);
    drop(values);

    for (key, content_hash, size) in exact_readbacks {
        let stored = engine
            .get(&key)
            .map_err(|e| raise_error!(format!("{e:#?}"), ErrorCode::InternalError))?
            .ok_or_else(|| {
                raise_error!(
                    "UIDONLY exact raw blob missing after durable write".into(),
                    ErrorCode::InternalError
                )
            })?;
        if stored.len() != size || compute_content_hash(&stored) != content_hash {
            return Err(raise_error!(
                "UIDONLY exact raw blob failed readback verification".into(),
                ErrorCode::InternalError
            ));
        }
    }
    Ok(())
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
        let blob_dir = DATA_DIR_MANAGER.storage_dir.join("blobs");

        let mut config = Config::default();
        config.default_codec = Codec::Zstd;
        config.compress_threshold = 1024;
        config.flush_interval_secs = 60;
        config.gc_interval_secs = 300;

        let engine = Engine::open(&blob_dir, config)
            .expect("Failed to initialize blob engine: Check disk space and permissions.");

        let engine = Arc::new(engine);

        let (sender, mut receiver) = mpsc::channel::<DetachedEmail>(100);

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
                                let engine_bg = Arc::clone(&engine_bg);
                                if let Err(e) = tokio::task::spawn_blocking(move || {
                                    for eml in batch {
                                        Self::process_detached_email(eml, &engine_bg);
                                    }
                                }).await {
                                    tracing::error!("BlobManager: spawn_blocking join error: {:#?}", e);
                                }
                            }
                            None => {
                                tracing::info!("BlobManager: All senders dropped, closing blob storage.");
                                break;
                            }
                        }
                    }
                    _ = shutdown.recv() => {
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
                            let engine_bg = Arc::clone(&engine_bg);
                            if let Err(e) = tokio::task::spawn_blocking(move || {
                                for eml in remaining {
                                    Self::process_detached_email(eml, &engine_bg);
                                }
                            }).await {
                                tracing::error!("BlobManager: shutdown spawn_blocking join error: {:#?}", e);
                            }
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
            handle: Mutex::new(Some(handler)),
        }
    }

    pub async fn queue(&self, email: DetachedEmail) {
        if let Err(e) = self.sender.send(email).await {
            tracing::error!("BlobManager channel closed, email lost: {:#?}", e);
        }
    }

    /// Durably stores a batch of exact RFC822 messages, then reads every unique
    /// raw blob back before any search-index completion marker may be written.
    pub(crate) async fn store_uidonly_exact_batch(
        &self,
        blobs: Vec<UidOnlyBlob>,
    ) -> BichonResult<()> {
        let engine = Arc::clone(&self.engine);
        tokio::task::spawn_blocking(move || store_uidonly_exact_batch_inner(&engine, blobs))
            .await
            .map_err(|e| raise_error!(format!("{e:#?}"), ErrorCode::InternalError))?
    }

    pub fn get_email(&self, content_hash: &str) -> BichonResult<Option<Bytes>> {
        self.get(content_hash)
    }

    pub(crate) fn get_uidonly_exact(&self, content_hash: &str) -> BichonResult<Option<Bytes>> {
        let Some(raw) = self.get(&uidonly_exact_raw_blob_key(content_hash))? else {
            return Ok(None);
        };
        if compute_content_hash(&raw) != content_hash {
            return Err(raise_error!(
                "UIDONLY exact raw blob digest mismatch".into(),
                ErrorCode::InternalError
            ));
        }
        Ok(Some(raw))
    }

    /// Fast restart check. This is a receipt only when the caller has already
    /// found a valid envelope marker committed after the initial full readback.
    pub(crate) fn has_uidonly_exact_batch(
        &self,
        content_hashes: &[String],
    ) -> BichonResult<Vec<bool>> {
        let keys: Result<Vec<_>, _> = content_hashes
            .iter()
            .map(|hash| hex_to_key(&uidonly_exact_raw_blob_key(hash)))
            .collect();
        self.engine
            .exists_batch(&keys?)
            .map_err(|e| raise_error!(format!("{e:#?}"), ErrorCode::InternalError))
    }

    pub(crate) fn get_canonical_email(&self, content_hash: &str) -> BichonResult<Option<Bytes>> {
        match self.get_uidonly_exact(content_hash)? {
            Some(raw) => Ok(Some(raw)),
            None => self.get(content_hash),
        }
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
        let mut keys = Vec::new();
        for hash in email_content_hashes {
            keys.push(hex_to_key(hash.as_ref())?);
            keys.push(hex_to_key(&uidonly_exact_raw_blob_key(hash.as_ref()))?);
        }

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

#[cfg(test)]
mod tests {
    use super::{store_uidonly_exact_batch_inner, uidonly_exact_raw_blob_key, UidOnlyBlob};
    use crate::utils::compute_content_hash;
    use bichon_blob::{Codec, Config, Engine};
    use bytes::Bytes;

    #[test]
    fn exact_raw_write_is_durable_verified_and_idempotent() {
        let dir = std::env::temp_dir().join(format!("bichon-uidonly-{}", uuid::Uuid::new_v4()));
        let engine = Engine::open(&dir, Config::default()).unwrap();
        let raw = b"From: sender@example.invalid\r\n\r\nbody\r\n";
        let hash = compute_content_hash(raw);
        let exact_hash = uidonly_exact_raw_blob_key(&hash);
        assert_ne!(exact_hash, hash);
        assert_eq!(exact_hash, uidonly_exact_raw_blob_key(&hash));
        let attachment = Bytes::from_static(b"attachment");
        let attachment_hash = compute_content_hash(&attachment);
        let key = super::hex_to_key(&hash).unwrap();
        engine
            .put(key, b"legacy detached value", Codec::Lz4)
            .unwrap();
        let blob = || UidOnlyBlob {
            content_hash: hash.clone(),
            raw: raw.to_vec(),
            attachments: vec![(attachment_hash.clone(), attachment.clone())],
        };
        store_uidonly_exact_batch_inner(&engine, vec![blob(), blob()]).unwrap();
        store_uidonly_exact_batch_inner(&engine, vec![blob()]).unwrap();
        assert_eq!(engine.get(&key).unwrap().unwrap(), b"legacy detached value");
        let exact_key = super::hex_to_key(&uidonly_exact_raw_blob_key(&hash)).unwrap();
        assert_eq!(engine.get(&exact_key).unwrap().unwrap(), raw);
        let attachment_key = super::hex_to_key(&attachment_hash).unwrap();
        assert_eq!(engine.get(&attachment_key).unwrap().unwrap(), attachment);
        assert_eq!(
            engine
                .exists_batch(&[key, exact_key, attachment_key])
                .unwrap(),
            vec![true, true, true]
        );
        engine.shutdown().unwrap();
        std::fs::remove_dir_all(dir).unwrap();
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
