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

use bichon_core::bichon_version;
use clap::{Parser, Subcommand};
use console::style;
use dialoguer::{theme::ColorfulTheme, Confirm, Input, Select};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::IsTerminal;
use std::path::Path;
use std::process::ExitCode;

use crate::{
    auth::verify_user_and_get_account, eml::handle_eml_directory_import,
    export::{handle_account_export, handle_export, handle_export_verify},
    mbox::handle_mbox_single_file_import, pst::handle_pst_import,
    thunderbird::handle_thunderbird_import,
};

pub mod api;
pub mod auth;
pub mod eml;
pub mod export;
pub mod mbox;
pub mod pst;
pub mod thunderbird;

#[derive(Parser, Debug)]
#[command(
    name = "bichon-cli",
    author = "rustmailer",
    version = bichon_version!(),
    about = "A CLI tool to import email data into Bichon service"
)]
pub struct BichonCli {
    /// Path to the configuration file
    #[arg(
        short,
        long,
        global = true,
        default_value = "config.toml",
        value_name = "FILE",
        help = "Sets a custom config file"
    )]
    pub config: std::path::PathBuf,

    /// Base URL of the Bichon service, overriding the config file
    /// (env: BICHON_BASE_URL)
    #[arg(long, global = true, value_name = "URL")]
    pub base_url: Option<String>,

    /// API token, overriding the config file (env: BICHON_API_TOKEN)
    #[arg(long, global = true, value_name = "TOKEN")]
    pub api_token: Option<String>,

    /// Without a subcommand the interactive wizard is started
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug, Clone)]
pub enum Command {
    /// Export to an MBOX or attachments ZIP without any prompts (cron-friendly)
    Export(export::noninteractive::ExportArgs),
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct BichonCliConfig {
    pub base_url: String,
    pub api_token: String,
}

/// Values that override the config file, from flags first and the environment
/// second. Kept free of I/O so the precedence is testable.
fn env_overrides(cli: &BichonCli) -> (Option<String>, Option<String>) {
    (
        cli.base_url
            .clone()
            .or_else(|| std::env::var("BICHON_BASE_URL").ok()),
        cli.api_token
            .clone()
            .or_else(|| std::env::var("BICHON_API_TOKEN").ok()),
    )
}

/// Build a config from explicit values and/or the config file, without ever
/// prompting. Used by the subcommands: a script has nobody to answer a prompt.
fn merge_config(
    path: &Path,
    base_url: Option<String>,
    api_token: Option<String>,
) -> Result<BichonCliConfig, String> {
    let file = if path.exists() {
        let content = fs::read_to_string(path)
            .map_err(|e| format!("could not read '{}': {e}", path.display()))?;
        Some(toml::from_str::<BichonCliConfig>(&content).map_err(|e| {
            format!(
                "could not parse '{}': {e}; it must contain `base_url` and `api_token`",
                path.display()
            )
        })?)
    } else {
        None
    };

    match (base_url, api_token, file) {
        (Some(base_url), Some(api_token), _) => Ok(BichonCliConfig {
            base_url,
            api_token,
        }),
        (base_url, api_token, Some(config)) => Ok(BichonCliConfig {
            base_url: base_url.unwrap_or(config.base_url),
            api_token: api_token.unwrap_or(config.api_token),
        }),
        (Some(_), None, None) => Err(format!(
            "cannot configure the client: '{}' does not exist and no API token was given. \
             Pass --api-token or set BICHON_API_TOKEN.",
            path.display()
        )),
        (None, Some(_), None) => Err(format!(
            "cannot configure the client: '{}' does not exist and no base URL was given. \
             Pass --base-url or set BICHON_BASE_URL.",
            path.display()
        )),
        (None, None, None) => Err(format!(
            "cannot configure the client: '{}' does not exist. Create it with `base_url` \
             and `api_token`, or pass --base-url/--api-token (BICHON_BASE_URL/BICHON_API_TOKEN).",
            path.display()
        )),
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = BichonCli::parse();

    match cli.command.clone() {
        Some(Command::Export(args)) => {
            let (base_url, api_token) = env_overrides(&cli);
            match merge_config(&cli.config, base_url, api_token) {
                Ok(config) => ExitCode::from(
                    export::noninteractive::run_export_command(&config, &args).await,
                ),
                Err(message) => {
                    eprintln!("{} {}", style("✘").red().bold(), message);
                    ExitCode::from(export::noninteractive::EXIT_USAGE)
                }
            }
        }
        None => {
            // The wizard cannot work without a terminal, and a cron job that
            // forgets a subcommand should fail loudly instead of blocking on
            // an invisible prompt.
            if !std::io::stdin().is_terminal() {
                eprintln!(
                    "{} No subcommand given and stdin is not a terminal.\n\
                     Use `bichon-cli export --help` for the non-interactive export.",
                    style("✘").red().bold()
                );
                return ExitCode::from(export::noninteractive::EXIT_USAGE);
            }
            run_wizard(&cli).await;
            ExitCode::SUCCESS
        }
    }
}

/// The original interactive wizard. Behaviour is unchanged except that a
/// complete `--base-url`/`--api-token` (or environment) pair skips the config
/// prompts.
async fn run_wizard(cli: &BichonCli) {
    let theme = ColorfulTheme::default();
    let config_path = &cli.config;
    let (override_url, override_token) = env_overrides(cli);

    let final_config = if let (Some(base_url), Some(api_token)) = (override_url, override_token) {
        println!(
            "{}",
            style("✔ Using the base URL and API token from the command line.").green()
        );
        BichonCliConfig {
            base_url,
            api_token,
        }
    } else {
        let mut current_config: Option<BichonCliConfig> = None;

        if config_path.exists() {
            if let Ok(content) = fs::read_to_string(config_path) {
                if let Ok(config) = toml::from_str::<BichonCliConfig>(&content) {
                    println!("{}", style("✔ Existing configuration found:").green());
                    println!("  Base URL: {}", style(&config.base_url).yellow());
                    println!(" API Token: {}", style(&config.api_token).yellow());

                    // Confirm with user
                    if Confirm::with_theme(&theme)
                        .with_prompt("Do you want to use this configuration?")
                        .default(true)
                        .interact()
                        .unwrap()
                    {
                        current_config = Some(config);
                    }
                }
            }
        }

        match current_config {
            Some(conf) => conf,
            None => {
                println!("\n{}", style("Please enter Bichon service details:").bold());

                let url: String = Input::with_theme(&theme)
                    .with_prompt("Bichon Base URL")
                    .default("http://localhost:15630".into())
                    .interact_text()
                    .unwrap();

                let token: String = Input::with_theme(&theme)
                    .with_prompt("API Token")
                    .interact_text()
                    .unwrap();

                let conf = BichonCliConfig {
                    base_url: url,
                    api_token: token,
                };

                // 3. Offer to save the new configuration
                if Confirm::with_theme(&theme)
                    .with_prompt("Save this configuration for future use?")
                    .default(true)
                    .interact()
                    .unwrap()
                {
                    let toml_str = toml::to_string(&conf).unwrap();
                    fs::write(config_path, toml_str).expect("Failed to save config file");
                    println!("{}", style("Configuration saved successfully!").green());
                }
                conf
            }
        }
    };

    let operations = &[
        "1. Import: Upload email data to Bichon",
        "2. Export: Download account data as MBOX file",
    ];

    let op_idx = Select::with_theme(&theme)
        .with_prompt("Select operation")
        .items(operations)
        .default(0)
        .interact()
        .unwrap();

    match op_idx {
        0 => {
            let target_account = verify_user_and_get_account(&final_config, &theme, true).await;

            let import_modes = &[
                "1. EML: Scan directory recursively (Maintains folder structure)",
                "2. MBOX: Single archive file (Stream from one file)",
                "3. Thunderbird: Import from local profile directory",
                "4. PST: Outlook Personal Storage (Single .pst file)",
            ];

            let mode_idx = Select::with_theme(&theme)
                .with_prompt("Select import method")
                .items(import_modes)
                .default(0)
                .interact()
                .unwrap();

            match mode_idx {
                0 => handle_eml_directory_import(&final_config, target_account.id, &theme).await,
                1 => handle_mbox_single_file_import(&final_config, target_account.id, &theme).await,
                2 => handle_thunderbird_import(&final_config, target_account.id, &theme).await,
                3 => handle_pst_import(&final_config, target_account.id, &theme).await,
                _ => unreachable!(),
            }
        }
        1 => {
            let export_modes = &[
                "1. Export by saved search (recommended)",
                "2. Export entire account (legacy)",
                "3. Verify an export job (compliance)",
            ];
            let export_idx = Select::with_theme(&theme)
                .with_prompt("Select export method")
                .items(export_modes)
                .default(0)
                .interact()
                .unwrap();
            match export_idx {
                0 => handle_export(&final_config, &theme).await,
                1 => {
                    let target_account =
                        verify_user_and_get_account(&final_config, &theme, false).await;
                    handle_account_export(&final_config, target_account, &theme).await;
                }
                2 => handle_export_verify(&final_config, &theme).await,
                _ => unreachable!(),
            }
        }
        _ => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(label: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        std::env::temp_dir().join(format!(
            "bichon-cli-config-{}-{}-{}.toml",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst),
            label
        ))
    }

    #[test]
    fn parses_the_export_subcommand() {
        let cli = BichonCli::try_parse_from([
            "bichon-cli",
            "--config",
            "/etc/bichon.toml",
            "export",
            "--saved-search",
            "Nightly",
            "--output-dir",
            "/data/exports",
            "--keep",
            "7",
        ])
        .unwrap();

        assert_eq!(cli.config, std::path::PathBuf::from("/etc/bichon.toml"));
        match cli.command {
            Some(Command::Export(args)) => {
                assert_eq!(args.saved_search.as_deref(), Some("Nightly"));
                assert_eq!(
                    args.output_dir.as_deref(),
                    Some(std::path::Path::new("/data/exports"))
                );
                assert_eq!(args.keep, Some(7));
                assert!(!args.no_wait);
            }
            other => panic!("expected an export subcommand, got {other:?}"),
        }
    }

    #[test]
    fn keeps_the_wizard_when_no_subcommand_is_given() {
        let cli = BichonCli::try_parse_from(["bichon-cli", "--config", "config.toml"]).unwrap();
        assert!(cli.command.is_none());
    }

    #[test]
    fn rejects_conflicting_output_arguments() {
        assert!(BichonCli::try_parse_from([
            "bichon-cli",
            "export",
            "--saved-search",
            "Nightly",
            "--output",
            "a.mbox",
            "--output-dir",
            "out",
        ])
        .is_err());
    }

    #[test]
    fn rejects_conflicting_sources() {
        assert!(BichonCli::try_parse_from([
            "bichon-cli",
            "export",
            "--saved-search",
            "Nightly",
            "--account-id",
            "1",
            "--output-dir",
            "out",
        ])
        .is_err());
    }

    #[test]
    fn accepts_repeated_account_ids() {
        let cli = BichonCli::try_parse_from([
            "bichon-cli",
            "export",
            "--account-id",
            "1",
            "--account-id",
            "2",
            "--output-dir",
            "out",
        ])
        .unwrap();
        match cli.command {
            Some(Command::Export(args)) => assert_eq!(args.account_id, vec![1, 2]),
            other => panic!("expected an export subcommand, got {other:?}"),
        }
    }

    #[test]
    fn explicit_values_win_over_the_config_file() {
        let path = temp_path("overrides");
        fs::write(
            &path,
            "base_url = \"http://from-file\"\napi_token = \"file-token\"\n",
        )
        .unwrap();

        let config = merge_config(&path, Some("http://flag".into()), None).unwrap();
        assert_eq!(config.base_url, "http://flag");
        assert_eq!(config.api_token, "file-token");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn the_config_file_is_used_when_no_values_are_given() {
        let path = temp_path("file-only");
        fs::write(&path, "base_url = \"http://file\"\napi_token = \"t\"\n").unwrap();

        let config = merge_config(&path, None, None).unwrap();
        assert_eq!(config.base_url, "http://file");
        assert_eq!(config.api_token, "t");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn a_missing_config_without_flags_is_an_actionable_error() {
        let path = temp_path("missing");
        let error = merge_config(&path, None, None).unwrap_err();
        assert!(error.contains("does not exist"), "{error}");
        assert!(error.contains("BICHON_API_TOKEN"), "{error}");
        assert!(error.contains("--base-url"), "{error}");
    }

    #[test]
    fn flags_complete_a_missing_config() {
        let path = temp_path("missing-with-flags");
        let config = merge_config(&path, Some("http://x".into()), Some("t".into())).unwrap();
        assert_eq!(config.base_url, "http://x");
        assert_eq!(config.api_token, "t");
    }

    #[test]
    fn a_malformed_config_file_is_reported_not_ignored() {
        let path = temp_path("malformed");
        fs::write(&path, "this is not toml").unwrap();

        let error = merge_config(&path, None, None).unwrap_err();
        assert!(error.contains("could not parse"), "{error}");
        let _ = fs::remove_file(path);
    }
}
