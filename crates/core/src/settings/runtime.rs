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

//! Runtime overrides for settings that used to be startup-only.
//!
//! The whole group is stored as **one** encrypted JSON document in the memdb
//! `system_config` collection. Each memdb value costs a PBKDF2 derivation to
//! encrypt or decrypt, so one document per settings page keeps Save and load
//! fast instead of paying that cost once per field. The parsed document is
//! cached in memory; writes replace it atomically.
//!
//! Values win over the frozen `SETTINGS` (env/CLI) value. Settings that must
//! be known before the database is opened (data directories, encryption
//! password, HTTP port, TLS) deliberately stay out of this module.
//!
//! Listener settings (SMTP, and the IMAP server in Pro) are read when the
//! listener starts, so a change needs a restart to take effect. Everything
//! else here is read at use time and applies without one.

use std::sync::{LazyLock, RwLock};

use serde::{Deserialize, Serialize};

use crate::error::code::ErrorCode;
use crate::settings::cli::{EncryptionMode, SETTINGS};
use crate::system_config;
use crate::raise_error;

/// Single `system_config` key holding the encrypted settings document.
pub const KEY: &str = "runtime.settings";

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Overrides {
    pub smtp_enabled: Option<bool>,
    pub smtp_port: Option<u16>,
    pub smtp_encryption: Option<String>,
    pub smtp_auth_required: Option<bool>,
    pub webui_token_expiration_hours: Option<u32>,
    /// `Some(0)` means automatic; `None` means "no override".
    pub sync_concurrency: Option<u16>,
    pub imap_timeout_seconds: Option<u64>,
    pub web_mbox_upload_limit_mb: Option<u64>,
    pub web_pst_upload_limit_mb: Option<u64>,
}

static CACHE: LazyLock<RwLock<Option<Overrides>>> = LazyLock::new(|| RwLock::new(None));

fn load() -> Overrides {
    if let Some(cached) = CACHE.read().unwrap().clone() {
        return cached;
    }
    let loaded = system_config::get(KEY)
        .and_then(|raw| match serde_json::from_str::<Overrides>(&raw) {
            Ok(v) => Some(v),
            Err(e) => {
                tracing::warn!("ignoring invalid runtime settings document: {e}");
                None
            }
        })
        .unwrap_or_default();
    *CACHE.write().unwrap() = Some(loaded.clone());
    loaded
}

/// Current overrides. Read-only; accessors below fall back to `SETTINGS`.
pub fn overrides() -> Overrides {
    load()
}

/// Replace every override in one encrypted write.
pub fn store(value: &Overrides) -> crate::error::BichonResult<()> {
    let raw = serde_json::to_string(value)
        .map_err(|e| raise_error!(format!("cannot encode runtime settings: {e}"), ErrorCode::InternalError))?;
    system_config::set(KEY, &raw)?;
    *CACHE.write().unwrap() = Some(value.clone());
    Ok(())
}

/// Drop every override and return to the startup values.
pub fn clear() -> crate::error::BichonResult<()> {
    system_config::delete(KEY)?;
    *CACHE.write().unwrap() = Some(Overrides::default());
    Ok(())
}

/// Whether the SMTP listener starts. Read at startup.
pub fn smtp_enabled() -> bool {
    load()
        .smtp_enabled
        .unwrap_or(SETTINGS.bichon_enable_smtp)
}

/// SMTP listener port. Read at startup.
pub fn smtp_port() -> u16 {
    load().smtp_port.unwrap_or(SETTINGS.bichon_smtp_port)
}

/// SMTP encryption mode. Read at startup.
pub fn smtp_encryption() -> EncryptionMode {
    match load().smtp_encryption.as_deref() {
        Some("none") => EncryptionMode::None,
        Some("starttls") => EncryptionMode::Starttls,
        Some("tls") => EncryptionMode::Tls,
        Some(other) => {
            tracing::warn!(
                "ignoring invalid runtime SMTP encryption {other:?} (using the startup value)"
            );
            SETTINGS.bichon_smtp_encryption
        }
        None => SETTINGS.bichon_smtp_encryption,
    }
}

/// Whether SMTP clients must authenticate. Read at startup.
pub fn smtp_auth_required() -> bool {
    load()
        .smtp_auth_required
        .unwrap_or(SETTINGS.bichon_smtp_auth_required)
}

/// Idle lifetime of a WebUI token. Read per request.
pub fn webui_token_expiration_hours() -> u32 {
    load()
        .webui_token_expiration_hours
        .unwrap_or(SETTINGS.bichon_webui_token_expiration_hours)
}

/// Sync worker concurrency. `0` means "auto" (CPU count × 2).
///
/// The semaphore is built once, so a change applies after a restart.
pub fn sync_concurrency() -> Option<u16> {
    match load().sync_concurrency {
        Some(0) => None,
        Some(n) => Some(n),
        None => SETTINGS.bichon_sync_concurrency,
    }
}

/// IMAP socket read timeout in seconds (`0` disables it). Read per connection.
pub fn imap_timeout_seconds() -> u64 {
    load()
        .imap_timeout_seconds
        .unwrap_or(SETTINGS.bichon_imap_timeout_seconds)
}

/// Maximum MBOX upload size accepted by the WebUI, in MiB. Read per request.
pub fn web_mbox_upload_limit_mb() -> u64 {
    load()
        .web_mbox_upload_limit_mb
        .unwrap_or(SETTINGS.bichon_web_mbox_upload_limit_mb)
}

/// Maximum PST upload size accepted by the WebUI, in MiB. Read per request.
pub fn web_pst_upload_limit_mb() -> u64 {
    load()
        .web_pst_upload_limit_mb
        .unwrap_or(SETTINGS.bichon_web_pst_upload_limit_mb)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::cli::SETTINGS;
    use std::sync::Once;

    static TEST_ENV: Once = Once::new();
    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn init_test_env() {
        TEST_ENV.call_once(|| {
            let root = crate::test_support::unique_test_root("bichon-runtime-settings-test");
            std::env::set_var("BICHON_ROOT_DIR", &root);
            std::env::set_var("BICHON_ENCRYPT_PASSWORD", "test-password");
            std::fs::create_dir_all(&root).unwrap();
            let _ = &*SETTINGS;
            let _ = &*crate::database::manager::DB_MANAGER;
        });
    }

    fn cleanup() {
        crate::backup::gate::WRITE_GATE.resume();
        let _ = clear();
    }

    #[tokio::test]
    async fn overrides_win_and_reset_falls_back() {
        let _guard = TEST_LOCK.lock().unwrap();
        init_test_env();
        cleanup();

        assert_eq!(smtp_enabled(), SETTINGS.bichon_enable_smtp);
        assert_eq!(smtp_port(), SETTINGS.bichon_smtp_port);
        assert_eq!(
            webui_token_expiration_hours(),
            SETTINGS.bichon_webui_token_expiration_hours
        );

        store(&Overrides {
            smtp_enabled: Some(true),
            smtp_port: Some(2526),
            smtp_encryption: Some("starttls".into()),
            smtp_auth_required: Some(false),
            webui_token_expiration_hours: Some(12),
            sync_concurrency: Some(6),
            imap_timeout_seconds: Some(0),
            web_mbox_upload_limit_mb: Some(2048),
            web_pst_upload_limit_mb: Some(4096),
        })
        .unwrap();

        assert!(smtp_enabled());
        assert_eq!(smtp_port(), 2526);
        assert_eq!(smtp_encryption(), EncryptionMode::Starttls);
        assert!(!smtp_auth_required());
        assert_eq!(webui_token_expiration_hours(), 12);
        assert_eq!(sync_concurrency(), Some(6));
        assert_eq!(imap_timeout_seconds(), 0);
        assert_eq!(web_mbox_upload_limit_mb(), 2048);
        assert_eq!(web_pst_upload_limit_mb(), 4096);

        cleanup();
        assert_eq!(smtp_port(), SETTINGS.bichon_smtp_port);
        assert_eq!(
            webui_token_expiration_hours(),
            SETTINGS.bichon_webui_token_expiration_hours
        );
    }

    #[tokio::test]
    async fn sync_concurrency_zero_means_auto() {
        let _guard = TEST_LOCK.lock().unwrap();
        init_test_env();
        cleanup();
        store(&Overrides {
            sync_concurrency: Some(0),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(sync_concurrency(), None);
        cleanup();
    }

    #[tokio::test]
    async fn document_roundtrips_as_one_value() {
        let _guard = TEST_LOCK.lock().unwrap();
        init_test_env();
        cleanup();
        store(&Overrides {
            smtp_port: Some(2527),
            ..Default::default()
        })
        .unwrap();
        // Exactly one encrypted document, not one entry per field.
        let raw = system_config::get(KEY).expect("document stored");
        assert!(raw.contains("smtp_port"));
        assert!(system_config::get("smtp.port").is_none());
        cleanup();
        assert!(system_config::get(KEY).is_none());
    }
}
