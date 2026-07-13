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

use tantivy::{schema::Facet, IndexWriter};

use crate::{
    error::{code::ErrorCode, BichonResult},
    raise_error,
};

pub mod attachment;
pub mod dedup;
pub mod dedup_cache;
pub mod envelope;
pub mod fields;
pub mod filter;
pub mod model;
pub mod schema;
pub mod tokenizers;
pub mod wildcard;

pub fn fatal_commit(writer: &mut IndexWriter) {
    const MAX_RETRIES: usize = 3;
    const RETRY_DELAY_MS: u64 = 1000;

    for attempt in 0..=MAX_RETRIES {
        match writer.commit() {
            Ok(_) => {
                if attempt > 0 {
                    eprintln!("[INFO] Commit succeeded on attempt {}", attempt + 1);
                }
                return;
            }
            Err(e) => match &e {
                tantivy::TantivyError::IoError(io_error) => {
                    if attempt < MAX_RETRIES {
                        eprintln!(
                            "[WARN] Commit failed (attempt {}/{}): {:?}. Retrying in {}ms...",
                            attempt + 1,
                            MAX_RETRIES + 1,
                            io_error,
                            RETRY_DELAY_MS * (attempt as u64 + 1)
                        );
                        std::thread::sleep(std::time::Duration::from_millis(
                            RETRY_DELAY_MS * (attempt as u64 + 1),
                        ));
                    } else {
                        eprintln!(
                            "[FATAL] Tantivy commit failed after {} attempts: {:?}",
                            MAX_RETRIES + 1,
                            io_error
                        );
                        std::process::exit(1);
                    }
                }
                _ => {
                    eprintln!("[FATAL] Tantivy commit failed with non-IO error: {e:?}");
                    std::process::exit(1);
                }
            },
        }
    }
}

pub fn validate_facet(tag: &str) -> BichonResult<()> {
    Facet::from_text(tag)
        .map(|_| ())
        .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InvalidParameter))
}
