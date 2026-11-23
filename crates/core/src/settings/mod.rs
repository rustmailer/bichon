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

use crate::settings::cli::Settings;
//use poem_openapi::Object;
use serde::{Deserialize, Serialize};

pub mod cli;
pub mod dir;
pub mod io;
pub mod proxy;
pub mod runtime;
pub mod system;
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Object))]
pub struct SystemConfigurations {
    pub bichon_log_level: String,
    pub bichon_http_port: i32,
    pub bichon_bind_ip: Option<String>,
    pub bichon_public_url: String,

    pub bichon_cors_origins: Option<Vec<String>>,
    pub bichon_cors_max_age: i32,

    pub bichon_ansi_logs: bool,
    pub bichon_log_to_file: bool,
    pub bichon_json_logs: bool,
    pub bichon_max_server_log_files: usize,

    pub bichon_encrypt_password_set: bool,
    pub bichon_webui_token_expiration_hours: u32,

    pub bichon_root_dir: String,

    pub bichon_enable_rest_https: bool,
    pub bichon_http_compression_enabled: bool,
    pub bichon_sync_concurrency: Option<u16>,

    pub bichon_base_url: String,
    pub bichon_index_dir: Option<String>,
    pub bichon_data_dir: Option<String>,

    pub bichon_enable_smtp: bool,
    pub bichon_smtp_port: u16,
    pub bichon_smtp_encryption: String,
    pub bichon_smtp_auth_required: bool,
    pub bichon_smtp_tls_key_path: Option<String>,
    pub bichon_smtp_tls_cert_path: Option<String>,


    pub bichon_upload_body_limit_mb: u64,

    pub bichon_web_mbox_upload_limit_mb: u64,

    pub bichon_web_pst_upload_limit_mb: u64,
}

impl From<&Settings> for SystemConfigurations {
    fn from(s: &Settings) -> Self {
        Self {
            bichon_log_level: s.bichon_log_level.clone(),
            bichon_http_port: s.bichon_http_port,
            bichon_bind_ip: s.bichon_bind_ip.clone(),
            bichon_public_url: s.bichon_public_url.clone(),
            bichon_cors_origins: s
                .bichon_cors_origins
                .as_ref()
                .map(|set| set.iter().cloned().collect()),
            bichon_cors_max_age: s.bichon_cors_max_age,
            bichon_ansi_logs: s.bichon_ansi_logs,
            bichon_log_to_file: s.bichon_log_to_file,
            bichon_json_logs: s.bichon_json_logs,
            bichon_max_server_log_files: s.bichon_max_server_log_files,
            bichon_encrypt_password_set: s.bichon_encrypt_password.is_some()
                || s.bichon_encrypt_password_file.is_some(),
            bichon_webui_token_expiration_hours: s.bichon_webui_token_expiration_hours,
            bichon_root_dir: s.root_dir().to_string(),
            bichon_enable_rest_https: s.bichon_enable_rest_https,
            bichon_http_compression_enabled: s.bichon_http_compression_enabled,
            bichon_sync_concurrency: s.bichon_sync_concurrency,
            bichon_base_url: s.bichon_base_url.clone(),
            bichon_index_dir: s.bichon_index_dir.clone(),
            bichon_data_dir: s.bichon_data_dir.clone(),
            bichon_enable_smtp: s.bichon_enable_smtp,
            bichon_smtp_port: s.bichon_smtp_port,
            bichon_smtp_encryption: s.bichon_smtp_encryption.to_string(),
            bichon_smtp_auth_required: s.bichon_smtp_auth_required,
            bichon_smtp_tls_key_path: s.bichon_tls_key_path.clone(),
            bichon_smtp_tls_cert_path: s.bichon_tls_cert_path.clone(),
            bichon_upload_body_limit_mb: s.bichon_upload_body_limit_mb,
            bichon_web_mbox_upload_limit_mb: s.bichon_web_mbox_upload_limit_mb,
            bichon_web_pst_upload_limit_mb: s.bichon_web_pst_upload_limit_mb,
        }
    }
}
