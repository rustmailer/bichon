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

use crate::modules::account::migration::{AccountV1, AccountV2, AccountV3};
use crate::modules::autoconfig::CachedMailSettings;
use crate::modules::error::code::ErrorCode;
use crate::modules::error::BichonResult;
use crate::modules::oauth2::entity::OAuth2;
use crate::modules::oauth2::pending::OAuth2PendingEntity;
use crate::modules::oauth2::token::OAuth2AccessToken;
use crate::modules::settings::proxy::Proxy;
use crate::modules::settings::system::SystemSetting;
use crate::modules::token::AccessTokenModel;
use crate::modules::users::role::UserRole;
use crate::modules::users::{BichonUser, BichonUserV2};
use crate::raise_error;
use db_type::{KeyOptions, ToKeyDefinition};
use itertools::Itertools;
use native_db::*;
use serde::Serialize;
use std::sync::{Arc, LazyLock};
use transaction::RwTransaction;

pub mod manager;

pub static META_MODELS: LazyLock<Models> = LazyLock::new(|| {
    let mut adapter = ModelsAdapter::new();
    adapter.register_metadata_models();
    adapter.models
});

pub struct ModelsAdapter {
    pub models: Models,
}

impl ModelsAdapter {
    pub fn new() -> Self {
        ModelsAdapter {
            models: Models::new(),
        }
    }

    pub fn register_model<T: ToInput>(&mut self) {
        self.models.define::<T>().expect("failed to define model ");
    }

    pub fn register_metadata_models(&mut self) {
        //Starting from version 0.2.0, `AccessToken` is deprecated/no longer used, but its ID must not be reused, otherwise it may cause model errors.
        //self.register_model::<AccessToken>();
        self.register_model::<SystemSetting>();
        self.register_model::<CachedMailSettings>();
        self.register_model::<AccountV1>();
        self.register_model::<AccountV2>();
        self.register_model::<AccountV3>();
        self.register_model::<OAuth2>();
        self.register_model::<OAuth2PendingEntity>();
        self.register_model::<OAuth2AccessToken>();
        self.register_model::<Proxy>();
        self.register_model::<UserRole>();
        self.register_model::<BichonUser>();
        self.register_model::<BichonUserV2>();
        self.register_model::<AccessTokenModel>();
    }
}

pub async fn insert_impl<T: ToInput + Clone + Send + 'static>(
    database: &Arc<Database<'static>>,
    item: T,
) -> BichonResult<()> {
    let db = database.clone();
    tokio::task::spawn_blocking(move || {
        let rw_transaction = db
            .rw_transaction()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        rw_transaction
            .insert(item)
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        rw_transaction
            .commit()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        Ok(())
    })
    .await
    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
}

pub async fn batch_insert_impl<T: ToInput + Clone + Send + 'static>(
    database: &Arc<Database<'static>>,
    batch: Vec<T>,
) -> BichonResult<()> {
    let db = database.clone();
    tokio::task::spawn_blocking(move || {
        let rw_transaction = db
            .rw_transaction()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        for item in batch {
            rw_transaction
                .insert(item)
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        }
        rw_transaction
            .commit()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        Ok(())
    })
    .await
    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
}

pub async fn batch_upsert_impl<T: ToInput + Clone + Send + 'static>(
    database: &Arc<Database<'static>>,
    batch: Vec<T>,
) -> BichonResult<()> {
    let db = database.clone();
    tokio::task::spawn_blocking(move || {
        let rw_transaction = db
            .rw_transaction()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        for item in batch {
            rw_transaction
                .upsert(item)
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        }
        rw_transaction
            .commit()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        Ok(())
    })
    .await
    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
}

pub async fn upsert_impl<T: ToInput + Clone + Send + 'static>(
    database: &Arc<Database<'static>>,
    item: T,
) -> BichonResult<()> {
    let db = database.clone();
    tokio::task::spawn_blocking(move || {
        let rw_transaction = db
            .rw_transaction()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        rw_transaction
            .upsert(item)
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        rw_transaction
            .commit()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        Ok(())
    })
    .await
    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
}

pub async fn update_impl<T: ToInput + Clone + std::fmt::Debug + Send + 'static>(
    database: &Arc<Database<'static>>,
    current: impl FnOnce(&RwTransaction) -> BichonResult<T> + Send + 'static,
    updated: impl FnOnce(&T) -> BichonResult<T> + Send + 'static,
) -> BichonResult<T> {
    let db = database.clone();
    tokio::task::spawn_blocking(move || {
        let rw = db
            .rw_transaction()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        let current_item = current(&rw)?;
        let updated_item = updated(&current_item)?;
        rw.update(current_item, updated_item.clone())
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        rw.commit()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        Ok(updated_item)
    })
    .await
    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
}

// pub async fn batch_update_impl<T: ToInput + Clone + std::fmt::Debug + Send + 'static>(
//     database: &Arc<Database<'static>>,
//     filter: impl FnOnce(&RwTransaction) -> RustMailerResult<Vec<T>> + Send + 'static,
//     updated: impl FnOnce(&Vec<T>) -> RustMailerResult<Vec<(T, T)>> + Send + 'static,
// ) -> RustMailerResult<Vec<T>> {
//     let db = database.clone();
//     tokio::task::spawn_blocking(move || {
//         let rw = db
//             .rw_transaction()
//             .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
//         let targets = filter(&rw)?;
//         let tuples = updated(&targets)?;
//         for (old, updated) in tuples {
//             rw.update(old, updated)
//                 .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
//         }
//         rw.commit()
//             .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
//         Ok(targets)
//     })
//     .await
//     .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
// }

pub async fn async_find_impl<T: ToInput + Clone + Send + 'static>(
    database: &Arc<Database<'static>>,
    key: impl ToKey + Send + 'static,
) -> BichonResult<Option<T>> {
    let db = database.clone();
    tokio::task::spawn_blocking(move || {
        let r_transaction = db
            .r_transaction()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        let entity: Option<T> = r_transaction
            .get()
            .primary(key)
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        Ok(entity)
    })
    .await
    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
}

// pub fn find_impl<T: ToInput + Clone + Send + 'static>(
//     database: &Arc<Database<'static>>,
//     key: &str,
// ) -> BichonResult<Option<T>> {
//     let db = database.clone();
//     let r_transaction = db
//         .r_transaction()
//         .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
//     let entity: Option<T> = r_transaction
//         .get()
//         .primary(key)
//         .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
//     Ok(entity)
// }

pub async fn delete_impl<T: ToInput + Clone + Send + 'static>(
    database: &Arc<Database<'static>>,
    delete: impl FnOnce(&RwTransaction) -> BichonResult<T> + Send + 'static,
) -> BichonResult<()> {
    let db = database.clone();
    tokio::task::spawn_blocking(move || {
        let rw_transaction = db
            .rw_transaction()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        let to_delete = delete(&rw_transaction)?;
        rw_transaction
            .remove::<T>(to_delete)
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        rw_transaction
            .commit()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        Ok(())
    })
    .await
    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
}

pub async fn batch_delete_impl<T: ToInput + Clone + Send + 'static>(
    database: &Arc<Database<'static>>,
    delete: impl FnOnce(&RwTransaction) -> BichonResult<Vec<T>> + Send + 'static,
) -> BichonResult<usize> {
    let db = database.clone();
    tokio::task::spawn_blocking(move || {
        let rw_transaction = db
            .rw_transaction()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        let to_delete = delete(&rw_transaction)?;
        let delete_count = to_delete.len();
        for item in to_delete {
            rw_transaction
                .remove(item)
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        }
        rw_transaction
            .commit()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        Ok(delete_count)
    })
    .await
    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
}

pub async fn list_all_impl<T: ToInput + Clone + Send + 'static>(
    database: &Arc<Database<'static>>,
) -> BichonResult<Vec<T>> {
    let db = database.clone();
    tokio::task::spawn_blocking(move || {
        let r_transaction = db
            .r_transaction()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        let entities: Vec<T> = r_transaction
            .scan()
            .primary()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
            .all()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
            .try_collect()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        Ok(entities)
    })
    .await
    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
}

pub async fn with_transaction(
    database: &Arc<Database<'static>>,
    f: impl FnOnce(&RwTransaction) -> BichonResult<()> + Send + 'static,
) -> BichonResult<()> {
    let db: Arc<Database<'_>> = database.clone();
    tokio::task::spawn_blocking(move || {
        let rw_transaction = db
            .rw_transaction()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        f(&rw_transaction)?;
        rw_transaction
            .commit()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        Ok(())
    })
    .await
    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
}

// For tables with a creation timestamp, place the creation time at the front of the primary key.
// This allows sorting by time, as the data is stored in dictionary order based on the primary key.
// If reverse sorting by time is needed, the iterator can be reversed.
pub async fn paginate_query_primary_scan_all_impl<
    T: ToInput + Serialize + std::fmt::Debug + std::marker::Unpin + Send + Sync + 'static,
>(
    database: &Arc<Database<'static>>,
    page: Option<u64>,
    page_size: Option<u64>,
    desc: Option<bool>,
) -> BichonResult<Paginated<T>> {
    let db = database.clone();

    tokio::task::spawn_blocking(move || {
        let r_transaction = db
            .r_transaction()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        let total_items = r_transaction
            .len()
            .primary::<T>()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

        // Validate page and page_size
        let (offset, total_pages) = if let (Some(p), Some(s)) = (page, page_size) {
            if p == 0 || s == 0 {
                return Err(raise_error!(
                    "'page' and 'page_size' must be greater than 0.".into(),
                    ErrorCode::InvalidParameter
                ));
            }
            let offset = (p - 1) * s;
            let total_pages = if total_items > 0 {
                (total_items as f64 / s as f64).ceil() as u64
            } else {
                0
            };
            (Some(offset), Some(total_pages))
        } else {
            (None, None)
        };

        // Handle empty result early
        if let Some(offset) = offset {
            if offset >= total_items {
                return Ok(Paginated::new(
                    page,
                    page_size,
                    total_items,
                    total_pages,
                    vec![],
                ));
            }
        }

        let scan = r_transaction
            .scan()
            .primary()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        let iter = scan
            .all()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

        // Collect items based on the reverse flag and pagination
        let items: Vec<T> = match desc {
            Some(true) => iter
                .rev()
                .skip(offset.unwrap_or(0) as usize)
                .take(page_size.unwrap_or(total_items) as usize)
                .try_collect()
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?,
            _ => iter
                .skip(offset.unwrap_or(0) as usize)
                .take(page_size.unwrap_or(total_items) as usize)
                .try_collect()
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?,
        };

        Ok(Paginated::new(
            page,
            page_size,
            total_items,
            total_pages,
            items,
        ))
    })
    .await
    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
}

pub async fn filter_by_secondary_key_impl<T: ToInput + Clone + Send + 'static>(
    database: &Arc<Database<'static>>,
    key_def: impl ToKeyDefinition<KeyOptions> + Send + 'static,
    start_with: impl ToKey + Send + 'static,
) -> BichonResult<Vec<T>> {
    let db = database.clone();
    tokio::task::spawn_blocking(move || {
        let r_transaction = db
            .r_transaction()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        let entities: Vec<T> = r_transaction
            .scan()
            .secondary(key_def)
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
            .start_with(start_with)
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
            .try_collect()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        Ok(entities)
    })
    .await
    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
}

pub async fn count_by_unique_secondary_key_impl<T: ToInput + Clone + Send + 'static>(
    database: &Arc<Database<'static>>,
    key_def: impl ToKeyDefinition<KeyOptions> + Send + 'static,
) -> BichonResult<usize> {
    let db = database.clone();
    tokio::task::spawn_blocking(move || {
        let r_transaction = db
            .r_transaction()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        let count = r_transaction
            .scan()
            .secondary::<T>(key_def)
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
            .all()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
            .count();
        Ok(count)
    })
    .await
    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
}

pub async fn secondary_find_impl<T: ToInput + Clone + Send + 'static>(
    database: &Arc<Database<'static>>,
    key_def: impl ToKeyDefinition<KeyOptions> + Send + 'static,
    key: impl ToKey + Send + 'static,
) -> BichonResult<Option<T>> {
    let db = database.clone();
    tokio::task::spawn_blocking(move || {
        let r_transaction = db
            .r_transaction()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

        let entities: Option<T> = r_transaction
            .get()
            .secondary(key_def, key)
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

        Ok(entities)
    })
    .await
    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
}

#[derive(Debug)]
pub struct Paginated<T> {
    pub page: Option<u64>,
    pub page_size: Option<u64>,
    pub total_items: u64,
    pub total_pages: Option<u64>,
    pub items: Vec<T>,
}

impl<T> Paginated<T> {
    pub fn new(
        page: Option<u64>,
        page_size: Option<u64>,
        total_items: u64,
        total_pages: Option<u64>,
        items: Vec<T>,
    ) -> Self {
        Paginated {
            page,
            page_size,
            total_items,
            total_pages,
            items,
        }
    }
}
