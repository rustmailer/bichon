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

//! Library half of `bichon-admin`, split from the binary so the Pro build
//! can ship its own `bichon-admin` that reuses the exact same CLI, menus and
//! restore flow (the Pro binary only registers the audit replayer and
//! enables the verify-only drill on top).

use clap::{Parser, Subcommand};
use console::style;
use dialoguer::{theme::ColorfulTheme, Select};

use bichon_core::{bichon_version, settings::cli::RestoreArgs};

use crate::{migrate_v037::handle_migration_v037, migrate_v1::handle_migrate_v1, reset::handle_reset_password};

pub mod legacy;
pub mod meta;
pub mod migrate_store_v2;
pub mod migrate_v037;
pub mod migrate_v1;
pub mod reindex;
pub mod reset;
pub mod restore;

#[derive(Parser, Debug)]
#[command(
    name = "bichon-admin",
    author = "rustmailer",
    version = bichon_version!(),
    about = "Bichon administrative tool: migrations, password reset, disaster recovery"
)]
pub struct AdminCli {
    /// Omit for the interactive menu; `bichon-admin restore` runs the
    /// disaster-recovery restore non-interactively.
    #[command(subcommand)]
    pub command: Option<AdminCommand>,
}

#[derive(Subcommand, Debug)]
pub enum AdminCommand {
    /// Restore a backup point from S3 into an empty directory, then exit
    /// (disaster recovery). The S3 backend settings come from a JSON config
    /// file (`--config`), never from the local install's metadata store.
    Restore(RestoreArgs),
    /// Rebuild the search indexes from the existing indexes + blobs, then exit.
    /// Required after a tokenizer/format change (the old index terms no longer
    /// match new queries). Reads the old indexes as an immutable source.
    Reindex(reindex::ReindexArgs),
}

/// Interactive main menu. `verify_only` makes the restore entries run the
/// Pro verify-only drill instead of writing (no-op for the community build,
/// which always passes `false`).
pub fn run_interactive(verify_only: bool) {
    let theme = ColorfulTheme::default();
    println!(
        "\n{}\n",
        style("BICHON ADMINISTRATIVE TOOL").bold().bright().cyan()
    );
    println!(
        "{}",
        style("Non-interactive disaster recovery:\n  bichon-admin restore --config <restore-config.json> --into <empty-dir>\n").dim()
    );

    let main_options = vec![
        "Reset Admin Password",
        "Migrate Legacy v0.3.7 Storage to v2.x (bichon-blob)",
        "Migrate v1.x Storage to v2.x (Fjall → bichon-blob)",
        "Restore a Backup Point (Disaster Recovery)",
        "Rebuild Search Indexes",
        "Exit",
    ];

    let selection = Select::with_theme(&theme)
        .with_prompt("Select an operation")
        .default(0)
        .items(&main_options)
        .interact()
        .unwrap();

    match selection {
        0 => handle_reset_password(&theme),
        1 => handle_migration_v037(&theme),
        2 => handle_migrate_v1(&theme),
        3 => {
            let code = tokio::runtime::Runtime::new()
                .expect("tokio runtime")
                .block_on(restore::run_interactive(&theme, verify_only));
            std::process::exit(code);
        }
        4 => {
            let code = reindex::run_cli(reindex::interactive_args(&theme));
            std::process::exit(code);
        }
        _ => {
            println!("{}", style("Exiting...").dim());
        }
    }
}
