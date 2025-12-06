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


use mimalloc::MiMalloc;
use modules::{
    common::rustls::RustMailerTls,
    context::{executors::EmailClientExecutors, Initialize},
    error::BichonResult,
    logger,
    rest::start_http_server,
    tasks::PeriodicTasks,
    token::root::ensure_root_token,
};
use tracing::info;

use crate::modules::{common::signal::SignalManager, settings::dir::DataDirManager};

mod modules;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

static LOGO: &str = r#"
 _      _        _                   
| |    (_)      | |                  
| |__   _   ___ | |__    ___   _ __  
| '_ \ | | / __|| '_ \  / _ \ | '_ \ 
| |_) || || (__ | | | || (_) || | | |
|_.__/ |_| \___||_| |_| \___/ |_| |_|
                                     
"#;
#[cfg(not(test))]
#[tokio::main]
async fn main() -> BichonResult<()> {
    logger::initialize_logging();
    info!("{}", LOGO);
    info!("Starting bichon-server");
    info!("Version:  {}", bichon_version!());
    info!("Git:      [{}]", env!("GIT_HASH"));
    info!("GitHub:   https://github.com/rustmailer/bichon");

    if let Err(error) = initialize().await {
        eprintln!("{:?}", error);
        return Err(error);
    }

    start_http_server().await?;
    Ok(())
}

/// Initialize the system by validating settings and starting necessary tasks.
async fn initialize() -> BichonResult<()> {
    // SETTINGS.validate()?;
    SignalManager::initialize().await?;
    DataDirManager::initialize().await?;
    ensure_root_token().await?;
    RustMailerTls::initialize().await?;
    EmailClientExecutors::initialize().await?;
    PeriodicTasks::start_background_tasks();
    Ok(())
}
