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

use crate::logger::file::setup_file_logger;
use crate::settings::cli::SETTINGS;
use chrono::Local;
use std::process;
use tracing::Level;
use tracing_log::LogTracer;
use tracing_subscriber::fmt::{format::Writer, time::FormatTime};

mod file;

struct LocalTimer;

impl FormatTime for LocalTimer {
    fn format_time(&self, w: &mut Writer<'_>) -> std::fmt::Result {
        write!(w, "{}", Local::now().format("%Y-%m-%dT%H:%M:%S%.3f%:z"))
    }
}

pub fn initialize_logging() {
    let level = validate_log_level(&SETTINGS.bichon_log_level);
    if matches!(level, Level::DEBUG) || matches!(level, Level::TRACE) {
        // async-imap trace events include raw commands and parser buffers,
        // which may contain credentials or complete message literals.
        LogTracer::builder()
            .ignore_crate("async_imap")
            .init()
            .unwrap();
    }
    if SETTINGS.bichon_log_to_file {
        setup_file_logger(level).unwrap();
    } else {
        setup_stdout_logger(level).unwrap();
    }
}

fn setup_stdout_logger(level: Level) -> Result<(), tracing::dispatcher::SetGlobalDefaultError> {
    let with_ansi = SETTINGS.bichon_ansi_logs;

    let format = tracing_subscriber::fmt::format()
        .with_level(true)
        .with_target(true)
        .with_timer(LocalTimer);

    let subscriber = tracing_subscriber::fmt()
        .with_max_level(level)
        .with_ansi(with_ansi)
        .with_writer(std::io::stdout)
        .event_format(format)
        .finish();

    tracing::subscriber::set_global_default(subscriber)
}

fn validate_log_level(value: &String) -> Level {
    match value.parse::<Level>() {
        Ok(level) => level,
        Err(_) => {
            eprintln!(
                "Invalid log level specified. Use one of: error, warn, info, debug, trace. 
                The log level you currently specified is 'rustmailer_log_level'='{}'",
                value
            );
            process::exit(1);
        }
    }
}
