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

pub mod common;
pub mod error;
pub mod export;
pub mod rest;

#[cfg(test)]
mod tests;

use std::sync::LazyLock;

use bichon_core::{
    archive::imap::task::SYNC_TASKS,
    bichon_version,
    common::{rustls::BichonTls, signal::SignalManager},
    context::{executors::BichonContext, Initialize},
    database::manager::DB_MANAGER,
    error::{code::ErrorCode, BichonResult},
    logger,
    migrate::check_data_status,
    raise_error,
    settings::{cli::SETTINGS, dir::DataDirManager},
    store::{
        blob::BLOB_MANAGER,
        tantivy::{attachment::ATTACHMENT_MANAGER, envelope::ENVELOPE_MANAGER},
    },
    tasks::PeriodicTasks,
    users::manager::UserManager,
};
use bichon_smtp::server::{start_smtp_server, SmtpServer};
use tracing::{error, info};

pub async fn run() -> BichonResult<()> {
    logger::initialize_logging();
    info!(
        r#"
     _      _        _
    | |    (_)      | |
    | |__   _   ___ | |__    ___   _ __
    | '_ \ | | / __|| '_ \  / _ \ | '_ \
    | |_) || || (__ | | | || (_) || | | |
    |_.__/ |_| \___||_| |_| \___/ |_| |_|

    "#
    );
    info!("Starting bichon-server");
    info!("Version:  {}", bichon_version!());
    info!("Git:      [{}]", env!("GIT_HASH"));
    info!("GitHub:   https://github.com/rustmailer/bichon");

    // Disaster recovery (`restore`) lives in the `bichon-admin` tool, not
    // here: a fresh box has no data root yet, and this binary requires one
    // at the clap level (the non-optional `bichon_root_dir` field) before
    // `run` is ever reached.

    rest::maintenance::serve_maintenance(
                "Community Edition",
                "Migration required",
                &[
                    "Your data was created by an older version of Bichon and must be migrated before use.".to_string(),
                    "Docker: run `docker exec -it <bichon-container> bichon-admin` and choose the migration option matching your old version (v0.3.7 → v2.x via v1.x, or v1.x → v2.x).".to_string(),
                    "Other installs: run `./bichon-admin` from the install directory.".to_string(),
                    "Both migrations are non-destructive: legacy files are never modified.".to_string(),
                    "After the migration completes, restart the service (e.g. `docker restart <bichon-container>`).".to_string(),
                    "Documentation: https://github.com/rustmailer/bichon/wiki".to_string(),
                ],
            )
            .await;
    return Ok(());

    match check_data_status() {
        Ok(false) => {
            error!("Incompatible data format detected.");
            error!("Your data was created by an older version of Bichon and must be migrated before use.");
            error!("Please run: bichon-admin");
            error!("Available migration options:");
            error!("  - Legacy v0.3.7 → v2.x (via v1.x)");
            error!("  - v1.x (Fjall) → v2.x (bichon-blob)");
            error!("Documentation: https://github.com/rustmailer/bichon/wiki");
            // Do NOT exit here: under Docker an exited container cannot be
            // `docker exec`-ed into, which is the only practical way to run
            // the interactive bichon-admin migration. Stay in maintenance
            // mode instead — the process keeps running and every HTTP
            // request is answered with a 503 page explaining what to do.
            rest::maintenance::serve_maintenance(
                "Community Edition",
                "Migration required",
                &[
                    "Your data was created by an older version of Bichon and must be migrated before use.".to_string(),
                    "Docker: run `docker exec -it <bichon-container> bichon-admin` and choose the migration option matching your old version (v0.3.7 → v2.x via v1.x, or v1.x → v2.x).".to_string(),
                    "Other installs: run `./bichon-admin` from the install directory.".to_string(),
                    "Both migrations are non-destructive: legacy files are never modified.".to_string(),
                    "After the migration completes, restart the service (e.g. `docker restart <bichon-container>`).".to_string(),
                    "Documentation: https://github.com/rustmailer/bichon/wiki".to_string(),
                ],
            )
            .await;
            return Ok(());
        }
        Err(e) => {
            error!("Failed to check data layout: {:#?}", e);
            rest::maintenance::serve_maintenance(
                "Community Edition",
                "Startup check failed",
                &[
                    format!("Checking the data layout failed: {e:#?}"),
                    "Check the service logs (e.g. `docker logs <bichon-container>`) for details."
                        .to_string(),
                    "Fix the underlying problem, then restart the service.".to_string(),
                ],
            )
            .await;
            return Ok(());
        }
        Ok(true) => {}
    }

    if let Err(error) = initialize().await {
        eprintln!("{:?}", error);
        return Err(error);
    }

    export::load_persisted_exports();
    export::spawn_export_cleanup();

    let periodic_tasks = PeriodicTasks::setup();
    let mut smtp_service: Option<SmtpServer> = None;
    if SETTINGS.bichon_enable_smtp {
        info!("SMTP service is enabled, starting...");
        match start_smtp_server().await {
            Ok(server) => {
                info!("SMTP server listening on: {}", server.smtp_addr);
                smtp_service = Some(server);
            }
            Err(e) => {
                error!("Failed to start SMTP server: {}", e);
                return Err(raise_error!(format!("{:#?}", e), ErrorCode::InternalError));
            }
        }
    } else {
        info!("SMTP service is disabled by configuration.");
    }

    rest::start_http_server().await?;
    periodic_tasks.shutdown().await;

    if let Some(server) = smtp_service {
        info!("Shutting down SMTP server...");
        server.stop().await;
        info!("SMTP server stopped.");
    }

    SYNC_TASKS.shutdown().await;
    ENVELOPE_MANAGER.shutdown().await;
    ATTACHMENT_MANAGER.shutdown().await;
    BLOB_MANAGER.shutdown().await;
    DB_MANAGER.flush();
    info!("Bichon server stopped.");
    Ok(())
}

async fn initialize() -> BichonResult<()> {
    SignalManager::initialize().await?;
    DataDirManager::initialize().await?;
    UserManager::initialize().await?;
    BichonTls::initialize().await?;
    BichonContext::initialize().await?;
    LazyLock::force(&BLOB_MANAGER);
    LazyLock::force(&ENVELOPE_MANAGER);
    LazyLock::force(&ATTACHMENT_MANAGER);
    Ok(())
}
