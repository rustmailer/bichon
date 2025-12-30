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

use crate::modules::account::migration::AccountModel;
use crate::modules::cache::imap::MAILBOX_MODELS;
use crate::modules::error::{code::ErrorCode, BichonError};
use crate::modules::settings::cli::SETTINGS;
use crate::modules::settings::dir::DATA_DIR_MANAGER;
use crate::modules::users::UserModel;
use crate::modules::{database::META_MODELS, error::BichonResult};
use crate::raise_error;
use native_db::{Builder, Database};
use std::sync::{Arc, LazyLock};
use tracing::info;

pub static DB_MANAGER: LazyLock<DatabaseManager> = LazyLock::new(DatabaseManager::new);

/// Metadata database instance
pub struct DatabaseManager {
    meta_db: Arc<Database<'static>>,
    /// Envelope database instance
    envelope_db: Arc<Database<'static>>,
}

impl DatabaseManager {
    fn new() -> Self {
        let meta_db = Self::init_meta_database().expect("Failed to initialize metadata database");
        let envelope_db =
            Self::init_evenlope_database().expect("Failed to initialize evenlope database");
        DatabaseManager {
            meta_db,
            envelope_db,
        }
    }

    /// Get a reference to the metadata database
    pub fn meta_db(&self) -> &Arc<Database<'static>> {
        &self.meta_db
    }

    pub fn envelope_db(&self) -> &Arc<Database<'static>> {
        &self.envelope_db
    }

    /// Initialize metadata database with a fixed or configured file path
    fn init_meta_database() -> BichonResult<Arc<Database<'static>>> {
        let mut database = Builder::new()
            .set_cache_size(
                SETTINGS
                    .bichon_metadata_cache_size
                    .unwrap_or(134217728)
                    .max(67108864),
            ) //default 128MB
            .create(&META_MODELS, DATA_DIR_MANAGER.meta_db.clone())
            .map_err(Self::handle_database_error)?;

        let rw = database
            .rw_transaction()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        rw.migrate::<AccountModel>()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        rw.migrate::<UserModel>()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        rw.commit()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

        database
            .compact()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        Ok(Arc::new(database))
    }

    fn init_evenlope_database() -> BichonResult<Arc<Database<'static>>> {
        info!(
            "Initializing envelope database at: {:?}",
            &DATA_DIR_MANAGER.mailbox_db
        );

        let mut database = Builder::new()
            .set_cache_size(
                SETTINGS
                    .bichon_envelope_cache_size
                    .unwrap_or(1073741824)
                    .max(67108864),
            ) //default 1GB
            .create(&MAILBOX_MODELS, DATA_DIR_MANAGER.mailbox_db.clone())
            .map_err(Self::handle_database_error)?;

        let rw = database
            .rw_transaction()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        rw.commit()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

        database
            .compact()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

        Ok(Arc::new(database))
    }

    fn handle_database_error(error: native_db::db_type::Error) -> BichonError {
        raise_error!(
            format!("Failed to create database: {:?}", error),
            ErrorCode::InternalError
        )
    }
}
