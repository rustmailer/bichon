use native_db::*;
use native_model::{native_model, Model};
use serde::{Deserialize, Serialize};

use crate::{
    id, raise_error,
    modules::{
        database::{
            insert_impl, secondary_find_impl,
            manager::DB_MANAGER,
        },
        error::{BichonResult, code::ErrorCode},
    },
};

pub type MboxFileModel = MboxFile;

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[native_model(id = 9, version = 1)]
#[native_db(primary_key(pk -> String))]
pub struct MboxFile {
    #[secondary_key(unique)]
    pub id: u64,
    #[secondary_key(unique)]
    pub path: String,
    pub account_id: u64,
}

impl MboxFile {
    fn pk(&self) -> String {
        self.id.to_string()
    }

    pub fn new(path: String, account_id: u64) -> Self {
        Self {
            id: id!(64),
            path,
            account_id,
        }
    }

    pub async fn save(&self) -> BichonResult<()> {
        insert_impl(DB_MANAGER.meta_db(), self.to_owned()).await
    }

    pub async fn find_by_path(path: &str) -> BichonResult<Option<MboxFileModel>> {
        secondary_find_impl(DB_MANAGER.meta_db(), MboxFileKey::path, path.to_string()).await
    }
    
    pub async fn find_by_id(id: u64) -> BichonResult<Option<MboxFileModel>> {
        secondary_find_impl(DB_MANAGER.meta_db(), MboxFileKey::id, id).await
    }

    pub async fn register(path: String, account_id: u64) -> BichonResult<MboxFileModel> {
        if let Some(existing) = Self::find_by_path(&path).await? {
            Ok(existing)
        } else {
            let new_mbox_file = Self::new(path, account_id);
            new_mbox_file.save().await?;
            Ok(new_mbox_file)
        }
    }

    pub async fn list_by_account(account_id: u64) -> BichonResult<Vec<MboxFileModel>> {
        use crate::modules::database::manager::DB_MANAGER;
        use std::iter::Iterator;

        let db = DB_MANAGER.meta_db().clone();
        tokio::task::spawn_blocking(move || {
            let r = db.r_transaction()
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

            let all_files: Vec<MboxFileModel> = r
                .scan()
                .primary()
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
                .all()
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

            let files: Vec<MboxFileModel> = all_files
                .into_iter()
                .filter(|f| f.account_id == account_id)
                .collect();

            Ok(files)
        })
        .await
        .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
    }

    pub async fn delete_by_id(id: u64) -> BichonResult<()> {
        use crate::modules::database::delete_impl;
        use crate::modules::database::manager::DB_MANAGER;

        delete_impl(DB_MANAGER.meta_db(), move |rw| {
            rw.get()
                .secondary::<MboxFile>(MboxFileKey::id, id)
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
                .ok_or_else(|| {
                    raise_error!(
                        format!("Mbox file with id {} not found", id),
                        ErrorCode::ResourceNotFound
                    )
                })
        })
        .await
        .map(|_| ())
    }
}
