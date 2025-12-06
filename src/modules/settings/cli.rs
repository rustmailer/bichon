//
// Copyright (c) 2025 rustmailer.com (https://rustmailer.com)
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


use clap::{builder::ValueParser, Parser, ValueEnum};
use std::{collections::HashSet, env, fmt, path::PathBuf, sync::LazyLock};

#[cfg(not(test))]
pub static SETTINGS: LazyLock<Settings> = LazyLock::new(Settings::parse);

#[cfg(test)]
pub static SETTINGS: LazyLock<Settings> = LazyLock::new(|| {
    // Use BICHON_DATA_DIR if set, otherwise use /tmp/bichon_test
    let root_dir = env::var("BICHON_DATA_DIR")
        .unwrap_or_else(|_| "/tmp/bichon_test".to_string());

    // Ensure the directory exists
    std::fs::create_dir_all(&root_dir).ok();

    Settings {
        bichon_log_level: "info".to_string(),
        bichon_http_port: 15630,
        bichon_bind_ip: Some("0.0.0.0".to_string()),
        bichon_public_url: "http://localhost:15630".to_string(),
        bichon_cors_origins: HashSet::new(),
        bichon_cors_max_age: 86400,
        bichon_ansi_logs: true,
        bichon_log_to_file: false,
        bichon_json_logs: false,
        bichon_max_server_log_files: 5,
        bichon_encrypt_password: "test-password".to_string(),
        bichon_root_dir: root_dir,
        bichon_metadata_cache_size: Some(134217728),
        bichon_envelope_cache_size: Some(1073741824),
        bichon_enable_access_token: false,
        bichon_enable_rest_https: false,
        bichon_http_compression_enabled: true,
        bichon_sync_concurrency: None,
    }
});

#[derive(Debug, Parser)]
#[clap(
    name = "bichon",
    about = "A self-hosted email synchronization and backup tool built in Rust",
    version = env!("CARGO_PKG_VERSION")
)]
pub struct Settings {
    /// bichon log level (default: "info")
    #[clap(
        long,
        default_value = "info",
        env,
        help = "Set the log level for bichon"
    )]
    pub bichon_log_level: String,

    /// bichon HTTP port (default: 15630)
    #[clap(
        long,
        default_value = "15630",
        env,
        help = "Set the HTTP port for bichon"
    )]
    pub bichon_http_port: i32,

    /// The IP address that the node binds to, in IPv4 format (e.g., 192.168.1.1).
    #[clap(
        long,
        env,
        default_value = "0.0.0.0",
        help = "The IP address that the node binds to, in IPv4 format (e.g., 192.168.1.1). Required in cluster mode.",
        value_parser = ValueParser::new(|s: &str| {
            // Ensure the input is a valid IPv4 address
            if s.parse::<std::net::Ipv4Addr>().is_err() {
                return Err("The bind IP address must be a valid IPv4 address.".to_string());
            }

            // If the address is valid, return it
            Ok(s.to_string())
        })
    )]
    pub bichon_bind_ip: Option<String>,

    /// RustMail public URL (default: "http://localhost:15630")
    #[clap(
        long,
        default_value = "http://localhost:15630",
        env,
        help = "Set the public URL for bichon"
    )]
    pub bichon_public_url: String,

    /// CORS allowed origins (default: "*")
    #[clap(
        long,
        default_value = "http://localhost:5173, http://localhost:15630, *",
        env,
        help = "Set the allowed CORS origins (comma-separated list, e.g., \"https://example.com, https://another.com\")",
        value_parser = ValueParser::new(|s: &str| -> Result<HashSet<String>, String> {
            let set: HashSet<String> = s.split(',')
                .map(|origin| origin.trim().to_string())
                .filter(|origin| !origin.is_empty())
                .collect();
            Ok(set)
        })
    )]
    pub bichon_cors_origins: HashSet<String>,

    /// CORS max age in seconds (default: 86400)
    #[clap(
        long,
        default_value = "86400",
        env,
        help = "Set the CORS max age in seconds"
    )]
    pub bichon_cors_max_age: i32,

    /// Enable ANSI logs (default: false)
    #[clap(long, default_value = "true", env, help = "Enable ANSI formatted logs")]
    pub bichon_ansi_logs: bool,

    /// Enable log file output (default: false)
    /// If false, logs will be printed to stdout
    #[clap(
        long,
        default_value = "false",
        env,
        help = "Enable log file output (otherwise logs go to stdout)"
    )]
    pub bichon_log_to_file: bool,

    /// Enable JSON logs (default: false)
    #[clap(
        long,
        default_value = "false",
        env,
        help = "Enable JSON formatted logs"
    )]
    pub bichon_json_logs: bool,

    /// Maximum number of log files (default: 5)
    #[clap(
        long,
        default_value = "5",
        env,
        help = "Set the maximum number of server log files"
    )]
    pub bichon_max_server_log_files: usize,

    /// bichon encryption password
    #[clap(
        long,
        default_value = "change-this-default-password-now",
        env,
        help = "Set the encryption password for bichon. ⚠️ Change this default in production!"
    )]
    pub bichon_encrypt_password: String,

    #[clap(
        long,
        env,
        help = "Set the file path for bichon database",
        value_parser = ValueParser::new(|s: &str| {
            let path = PathBuf::from(s);
            if !path.is_absolute() {
                return Err("Path must be an absolute directory path".to_string());
            }
            if !path.exists() {
                return Err(format!("Path {:?} does not exist", path));
            }
            if !path.is_dir() {
                return Err(format!("Path {:?} is not a directory", path));
            }
            Ok(s.to_string())
        })
    )]
    pub bichon_root_dir: String,

    #[clap(
        long,
        env,
        default_value = "134217728",
        help = "Set the cache size for bichon metadata database in bytes"
    )]
    pub bichon_metadata_cache_size: Option<usize>,

    #[clap(
        long,
        env,
        default_value = "1073741824",
        help = "Set the cache size for envelope database in bytes"
    )]
    pub bichon_envelope_cache_size: Option<usize>,

    /// Enables or disables the access token mechanism for HTTP endpoints.
    ///
    /// When set to `true`, HTTP requests will be subject to access token validation.
    /// If the `Authorization` header is missing or the token is invalid, the service will return a 401 Unauthorized response.
    /// When set to `false`, access token validation will be skipped.
    #[clap(
        long,
        default_value = "false",
        env,
        help = "Enables or disables the access token mechanism for HTTP endpoints."
    )]
    pub bichon_enable_access_token: bool,

    /// Enables or disables HTTPS for REST API endpoints.
    ///
    /// When set to `true`, the REST API will use HTTPS with a valid SSL/TLS certificate for secure communication.
    /// If no valid certificate is configured or HTTPS cannot be established, the service will fail to start.
    /// When set to `false`, the REST API will use plain HTTP without encryption.
    #[clap(
        long,
        default_value = "false",
        env,
        help = "Enables or disables HTTPS for REST API endpoints."
    )]
    pub bichon_enable_rest_https: bool,

    #[clap(
        long,
        default_value = "true",
        env,
        help = "Enable compression for the open api server"
    )]
    pub bichon_http_compression_enabled: bool,

    #[clap(
        long,
        env,
        help = "Maximum number of concurrent email sync tasks (default: number of CPU cores x 2)",
        value_parser = clap::value_parser!(u16).range(1..)
    )]
    pub bichon_sync_concurrency: Option<u16>,
}

#[derive(Clone, Copy, Debug, PartialEq, ValueEnum)]
pub enum CompressionAlgorithm {
    #[clap(name = "none")]
    None,
    #[clap(name = "gzip")]
    Gzip,
    #[clap(name = "brotli")]
    Brotli,
    #[clap(name = "zstd")]
    Zstd,
    #[clap(name = "deflate")]
    Deflate,
}

impl fmt::Display for CompressionAlgorithm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CompressionAlgorithm::None => write!(f, "none"),
            CompressionAlgorithm::Gzip => write!(f, "gzip"),
            CompressionAlgorithm::Brotli => write!(f, "brotli"),
            CompressionAlgorithm::Zstd => write!(f, "zstd"),
            CompressionAlgorithm::Deflate => write!(f, "deflate"),
        }
    }
}
