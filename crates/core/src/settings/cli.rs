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

use crate::settings::io::check_dir_read_write;
use clap::{builder::ValueParser, Args, Parser, ValueEnum};
use std::{
    collections::HashSet,
    env, fmt,
    path::PathBuf,
    sync::{LazyLock, OnceLock},
};

pub static SETTINGS: LazyLock<Settings> = LazyLock::new(Settings::init);

#[derive(Clone, Debug, Parser)]
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

    /// The IP address that the node binds to, in IPv4 or IPv6 format (e.g., 192.168.1.1 or ::1).
    #[clap(
        long,
        env,
        default_value = "0.0.0.0",
        help = "The IP address that the node binds to, in IPv4 or IPv6 format (e.g., 192.168.1.1 or ::1).",
        value_parser = ValueParser::new(|s: &str| {
            // Ensure the input is a valid IPv4 or IPv6 address
            if s.parse::<std::net::Ipv4Addr>().is_err() && s.parse::<std::net::Ipv6Addr>().is_err() {
                return Err("The bind IP address must be a valid IPv4 or IPv6 address.".to_string());
            }

            // If the address is valid, return it
            Ok(s.to_string())
        })
    )]
    pub bichon_bind_ip: Option<String>,

    /// bichon public URL (default: "http://localhost:15630")
    #[clap(
        long,
        default_value = "http://localhost:15630",
        env,
        help = "Set the public URL for bichon"
    )]
    pub bichon_public_url: String,

    /// bichon base URL path (default: "/")
    #[clap(
        long,
        default_value = "/",
        env,
        help = "Set the base UI path for bichon (e.g., '/bichon' or '/bichon/'). Must start with /",
        value_parser = validate_base_url
    )]
    pub bichon_base_url: String,

    /// CORS allowed origins (default: "*")
    #[clap(
        long,
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
    pub bichon_cors_origins: Option<HashSet<String>>,

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
        env,
        default_value = "change-this-default-password-now",
        help = "Set the encryption password for bichon. Alternatively, you can use --bichon-encrypt-password-file. If both are set, this parameter takes precedence over the file."
    )]
    pub bichon_encrypt_password: Option<String>,

    #[clap(
        long,
        env,
        help = "The file containing the encryption password. An alternative to --bichon-encrypt-password."
    )]
    pub bichon_encrypt_password_file: Option<String>,

    /// WebUI token expiration time in seconds (default: 7 days)
    #[clap(
        long,
        default_value = "168",
        env,
        help = "Set the WebUI token expiration time in hours"
    )]
    pub bichon_webui_token_expiration_hours: u32,

    #[clap(
        long,
        env,
        help = "Set the file path for bichon database",
        value_parser = ValueParser::new(|s: &str| {
            let path = PathBuf::from(s);

            if !path.is_absolute() {
                return Err("'bichon_root_dir' must be an absolute directory path".to_string());
            }

            check_dir_read_write(&path)?;
            Ok(s.to_string())
        })
    )]
    pub bichon_root_dir: String,
    #[clap(
        long,
        env,
        help = "Set the file path for email index directory",
        value_parser = ValueParser::new(|s: &str| {
            let path = PathBuf::from(s);

            if !path.is_absolute() {
                return Err("'bichon_index_dir' must be an absolute directory path".to_string());
            }

            check_dir_read_write(&path)?;
            Ok(s.to_string())
        })
    )]
    pub bichon_index_dir: Option<String>,
    #[clap(
        long,
        env,
        help = "Set the file path for email data directory",
        value_parser = ValueParser::new(|s: &str| {
            let path = PathBuf::from(s);

            if !path.is_absolute() {
                return Err("'bichon_data_dir' must be an absolute directory path".to_string());
            }

            check_dir_read_write(&path)?;
            Ok(s.to_string())
        })
    )]
    pub bichon_data_dir: Option<String>,
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

    #[clap(
        long,
        env,
        default_value = "90",
        help = "IMAP socket read timeout in seconds (0 disables the timeout). Servers that throttle or burst slowly (e.g. Zoho) can pause for 30-60s between responses; keep this above the longest expected server silence so throttling surfaces as progress delay, not a failed sync."
    )]
    pub bichon_imap_timeout_seconds: u64,

    #[clap(
        long,
        env,
        default_value = "false",
        help = "Enable the embedded SMTP server for real-time email receiving"
    )]
    pub bichon_enable_smtp: bool,

    #[clap(
        long,
        env,
        help = "Path to the SMTP TLS private key file (e.g., key.pem)",
        value_parser = ValueParser::new(|s: &str| {
            let path = PathBuf::from(s);
            if !path.is_absolute() {
                return Err("'bichon_smtp_tls_key_path' must be an absolute path".to_string());
            }
            if !path.exists() {
                return Err(format!("SMTP TLS key file not found: {}", s));
            }
            Ok(s.to_string())
        })
    )]
    pub bichon_tls_key_path: Option<String>,

    #[clap(
        long,
        env,
        help = "Path to the SMTP TLS certificate chain file (e.g., cert.pem)",
        value_parser = ValueParser::new(|s: &str| {
            let path = PathBuf::from(s);
            if !path.is_absolute() {
                return Err("'bichon_smtp_tls_cert_path' must be an absolute path".to_string());
            }
            if !path.exists() {
                return Err(format!("SMTP TLS certificate file not found: {}", s));
            }
            Ok(s.to_string())
        })
    )]
    pub bichon_tls_cert_path: Option<String>,

    #[clap(
        long,
        default_value = "2525",
        env,
        help = "Set the SMTP port for Bichon (e.g., 25 or 2525). Note: Port 25 may require root privileges.",
        value_parser = clap::value_parser!(u16).range(1..)
    )]
    pub bichon_smtp_port: u16,

    #[clap(
        long,
        env,
        default_value = "starttls",
        help = "Set the encryption mode for SMTP: 'none', 'starttls', or 'tls'"
    )]
    pub bichon_smtp_encryption: EncryptionMode,

    #[clap(
        long,
        env,
        default_value = "true",
        help = "Enable SMTP authentication requirement"
    )]
    pub bichon_smtp_auth_required: bool,

    /// Enable OIDC-based Single Sign-On (available in community edition in this fork).
    #[clap(long, default_value = "false", env, help = "Enable OpenID Connect SSO")]
    pub bichon_oidc_enabled: bool,

    /// OIDC issuer URL (e.g. https://keycloak.example.com/realms/myorg).
    #[clap(long, env, help = "OpenID Connect issuer URL")]
    pub bichon_oidc_issuer_url: Option<String>,

    /// OIDC client ID registered with the IdP.
    #[clap(long, env, help = "OpenID Connect client ID")]
    pub bichon_oidc_client_id: Option<String>,

    /// OIDC client secret registered with the IdP.
    #[clap(long, env, help = "OpenID Connect client secret")]
    pub bichon_oidc_client_secret: Option<String>,

    /// OIDC redirect URI (must match what's registered with the IdP).
    #[clap(long, env, help = "OpenID Connect redirect URI")]
    pub bichon_oidc_redirect_uri: Option<String>,

    /// Role ID assigned to auto-provisioned OIDC users. Defaults to the built-in
    /// Member role. Set to another built-in or custom role ID to change behaviour.
    #[clap(
        long,
        env,
        default_value_t = 100_200_000_000_000_u64,
        help = "Global role ID assigned to auto-provisioned OIDC users (default: Member role)"
    )]
    pub bichon_oidc_default_role_id: u64,

    /// When enabled and OIDC is configured, the sign-in page automatically
    /// redirects the browser to the OIDC provider instead of showing the
    /// username/password form. Users can still reach the local login by
    /// visiting `/sign-in?local=1`.
    #[clap(
        long,
        default_value = "false",
        env,
        help = "Automatically redirect the sign-in page to the OIDC provider"
    )]
    pub bichon_oidc_auto_redirect: bool,

    /// Maximum HTTP request body size in MB for file uploads (default: 1100 MB).
    /// Requests exceeding this limit are rejected at the framework level before
    /// the application reads the body, preventing memory exhaustion attacks.
    #[clap(
        long,
        default_value = "1100",
        env,
        help = "Maximum HTTP request body size in MB for file uploads"
    )]
    pub bichon_upload_body_limit_mb: u64,

    /// Maximum per-file size in MB for MBOX uploads via the web UI (default: 1024 MB = 1 GB).
    /// Individual EML files are always capped at 100 MB regardless of this setting.
    #[clap(
        long,
        default_value = "1024",
        env,
        help = "Maximum per-file size in MB for MBOX uploads via the web UI"
    )]
    pub bichon_web_mbox_upload_limit_mb: u64,

    /// Maximum per-file size in MB for PST uploads via the web UI (default: 2048 MB = 2 GB).
    #[clap(
        long,
        default_value = "2048",
        env,
        help = "Maximum per-file size in MB for PST uploads via the web UI"
    )]
    pub bichon_web_pst_upload_limit_mb: u64,
}

/// Arguments for the one-shot disaster-recovery restore (`bichon-admin
/// restore`). Restore lives in the admin tool, not the server binary: a
/// fresh box has no data root yet, and the server CLI requires one at the
/// clap level. The S3 backend connection details come from a JSON config
/// file (`--config`) — restore deliberately does not read the local
/// install's metadata store, so it runs unchanged on a fresh box.
#[derive(Clone, Debug, Args)]
pub struct RestoreArgs {
    /// JSON config file with the S3 backup backend settings (`s3_endpoint`,
    /// `s3_bucket`, `prefix`, `s3_region`, `s3_access_key`,
    /// `s3_secret_key`; only endpoint and bucket are mandatory).
    #[clap(
        long,
        short = 'c',
        required = true,
        value_parser = ValueParser::new(|s: &str| {
            let path = PathBuf::from(s);
            if !path.is_file() {
                return Err(format!("restore: config file not found: {s}"));
            }
            Ok(s.to_string())
        }),
        help = "JSON config file with the S3 backup backend settings (s3_endpoint, s3_bucket, prefix, s3_region, s3_access_key, s3_secret_key)"
    )]
    pub config: String,

    /// Empty target directory the restore writes into (required). Refused if
    /// it is not empty or touches the live data root (R7).
    #[clap(
        long,
        env = "BICHON_RESTORE_INTO",
        help = "Empty target directory to restore into (required; R7: must be empty, never the live data root)"
    )]
    pub into: String,

    /// Restore a specific point (`m-<id>`); default = LATEST.
    #[clap(
        long,
        env = "BICHON_RESTORE_POINT",
        help = "Restore point to use (default: LATEST)"
    )]
    pub point: Option<String>,

    /// Target index parent dir (mirrors `bichon-index-dir`). Default = the
    /// index lives under the target root (`<into>/bichon-indices`).
    #[clap(
        long,
        env = "BICHON_RESTORE_INDEX_DIR",
        help = "Target index parent directory (default: <into>/bichon-indices; R7: must be empty, never the live data root)"
    )]
    pub index_dir: Option<String>,

    /// Target blob data parent dir (mirrors `bichon-data-dir`). Default = the
    /// blob store lives under the target root (`<into>/bichon-storage`).
    #[clap(
        long,
        env = "BICHON_RESTORE_DATA_DIR",
        help = "Target blob data parent directory (default: <into>/bichon-storage; R7: must be empty, never the live data root)"
    )]
    pub data_dir: Option<String>,

    /// The live data root of an existing install (if the box being recovered
    /// still has one). Restore targets that touch it are refused (R7). Leave
    /// unset on a fresh disaster-recovery box.
    #[clap(
        long,
        env = "BICHON_ROOT_DIR",
        help = "Path of the existing bichon data root (R7: restore targets must be disjoint from it); omit on a fresh box"
    )]
    pub root_dir: Option<String>,
}

/// Overrides the settings used by the `SETTINGS` global.
///
/// The Pro binary parses one merged clap command (community + Pro args) and
/// seeds this override before anything derefs `SETTINGS`, so community CLI
/// args keep working even when Pro-only flags are present on the same command
/// line.
pub fn override_settings(settings: Settings) {
    let _ = SETTINGS_OVERRIDE.set(settings);
}

static SETTINGS_OVERRIDE: OnceLock<Settings> = OnceLock::new();

impl Settings {
    /// The data root for this process. Always present: clap requires
    /// `--bichon-root-dir`/`BICHON_ROOT_DIR` (the field is non-optional, so
    /// the parser rejects a bare server run). The one-shot disaster-recovery
    /// restore lives in `bichon-admin`, which never parses `Settings`.
    pub fn root_dir(&self) -> &str {
        self.bichon_root_dir.as_str()
    }

    pub fn init() -> Self {
        // The Pro binary parses a single merged clap command (community +
        // Pro args) and seeds the override below before anything derefs
        // `SETTINGS`.  When an override is present it wins and argv is not
        // re-parsed, so community CLI args keep working even when Pro-only
        // flags are present on the same command line.
        let s = match SETTINGS_OVERRIDE.get() {
            Some(settings) => settings.clone(),
            None => {
                // `cargo test` passes test-filter names and flags (e.g.
                // --nocapture) as extra positional arguments.  Try the full
                // argv first; if clap rejects it, fall back to parsing with
                // only the binary name so that the settings come entirely
                // from environment variables.
                let args: Vec<String> = std::env::args().collect();
                match Self::try_parse_from(&args) {
                    Ok(s) => s,
                    Err(e) => {
                        // `--help` / `--version` short-circuit clap; print and
                        // exit cleanly instead of falling through to the
                        // env-only re-parse (which would then trip the
                        // data-root requirement below on a server run).
                        if matches!(
                            e.kind(),
                            clap::error::ErrorKind::DisplayHelp
                                | clap::error::ErrorKind::DisplayVersion
                        ) {
                            e.exit();
                        }
                        Self::parse_from(std::iter::once(args[0].clone()))
                    }
                }
            }
        };
        // The data-root requirement lives entirely in clap (the non-optional
        // `bichon_root_dir` field), so nothing to check here for it.
        if s.bichon_encrypt_password.is_none() && s.bichon_encrypt_password_file.is_none() {
            panic!(
                "One of --bichon_encrypt_password or --bichon_encrypt_password_file has to be set"
            );
        }
        s
    }
}

fn validate_base_url(s: &str) -> Result<String, String> {
    if s == "/" {
        return Ok(s.to_string());
    }
    if !s.starts_with('/') {
        return Err(String::from(
            "Base URL must start with '/' (e.g., '/bichon')",
        ));
    }
    Ok(s.to_string())
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

#[derive(Clone, Copy, Debug, PartialEq, ValueEnum)]
pub enum EncryptionMode {
    #[clap(name = "none")]
    None,
    #[clap(name = "starttls")]
    Starttls,
    #[clap(name = "tls")]
    Tls,
}

impl fmt::Display for EncryptionMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EncryptionMode::None => write!(f, "none"),
            EncryptionMode::Starttls => write!(f, "starttls"),
            EncryptionMode::Tls => write!(f, "tls"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The flat server flags parse as before; `--bichon-root-dir` is required
    /// (non-optional field), enforced by clap itself.
    #[test]
    fn flat_server_flags_parse() {
        let root = std::env::temp_dir().join("bichon-cli-test-root");
        let s = Settings::try_parse_from([
            "bichon",
            "--bichon-root-dir",
            root.to_str().unwrap(),
            "--bichon-http-port",
            "9999",
        ])
        .expect("flat flags should parse");
        assert_eq!(s.bichon_http_port, 9999);
        assert_eq!(s.root_dir(), root.to_str().unwrap());
    }

    /// A server run without `--bichon-root-dir` must fail at the clap level —
    /// the requirement is enforced by the parser, not by a manual runtime
    /// check. The env binding is lifted for the parse (other tests in this
    /// binary legitimately set `BICHON_ROOT_DIR` to bootstrap `SETTINGS`) and
    /// restored afterwards.
    #[test]
    fn server_run_without_root_dir_is_clap_error() {
        let saved = std::env::var("BICHON_ROOT_DIR").ok();
        std::env::remove_var("BICHON_ROOT_DIR");
        let parsed = Settings::try_parse_from(["bichon", "--bichon-http-port", "9999"]);
        match saved {
            Some(v) => std::env::set_var("BICHON_ROOT_DIR", v),
            None => std::env::remove_var("BICHON_ROOT_DIR"),
        }
        let err = parsed.expect_err("missing --bichon-root-dir must be rejected");
        assert_eq!(err.kind(), clap::error::ErrorKind::MissingRequiredArgument);
    }

    /// `RestoreArgs` (the `bichon-admin restore` flags) parse standalone:
    /// `--config` and `--into` are required, the S3 details live in the
    /// config file.
    #[test]
    fn restore_args_parse_standalone() {
        use clap::{Command as ClapCommand, FromArgMatches};

        let cfg = std::env::temp_dir().join("bichon-restore-cfg-test.json");
        std::fs::write(
            &cfg,
            r#"{"s3_endpoint":"http://localhost:9000","s3_bucket":"bichon","prefix":"pfx","s3_region":"us-west-2","s3_access_key":"ak","s3_secret_key":"sk"}"#,
        )
        .unwrap();

        let matches = RestoreArgs::augment_args(ClapCommand::new("restore"))
            .try_get_matches_from([
                "restore",
                "--config",
                cfg.to_str().unwrap(),
                "--into",
                "/tmp/restore",
            ])
            .expect("restore args should parse");
        let args = RestoreArgs::from_arg_matches(&matches).unwrap();
        assert_eq!(args.into, "/tmp/restore");
        assert!(args.point.is_none());
        // `root_dir` binds `BICHON_ROOT_DIR`; other tests in this binary set
        // that env concurrently, so only assert absence on a clean env.
        if std::env::var("BICHON_ROOT_DIR").is_err() {
            assert!(args.root_dir.is_none());
        }

        // `--config` and `--into` are required; a missing file is rejected by
        // the value parser.
        let err = RestoreArgs::augment_args(ClapCommand::new("restore"))
            .try_get_matches_from(["restore", "--into", "/tmp/restore"])
            .expect_err("missing --config must be rejected");
        assert_eq!(err.kind(), clap::error::ErrorKind::MissingRequiredArgument);

        let err = RestoreArgs::augment_args(ClapCommand::new("restore"))
            .try_get_matches_from([
                "restore",
                "--config",
                "/definitely/not/a/real/file.json",
                "--into",
                "/tmp/restore",
            ])
            .expect_err("missing config file must be rejected");
        assert_eq!(err.kind(), clap::error::ErrorKind::ValueValidation);

        let _ = std::fs::remove_file(&cfg);
    }
}
