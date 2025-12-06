use poem::web::Json;
use poem_openapi::{payload::Json as OAIJson, ApiResponse, Object, OpenApi};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::task;

use crate::modules::{
    error::{code::ErrorCode, BichonResult},
    import::mbox::import_mbox_from_path,
    mbox::migration::MboxFileModel,
    rest::api::ApiTags,
    settings::dir::DATA_DIR_MANAGER,
};

pub struct ImportApi;

#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize, Object)]
pub struct MboxImportRequest {
    pub account_id: u64,
    pub mail_folder: String,
    pub path: String,
}

#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize, Object)]
pub struct MboxFileInfo {
    pub id: u64,
    pub path: String,
    pub account_id: u64,
}

#[derive(ApiResponse)]
pub enum MboxImportResponse {
    #[oai(status = 200)]
    Ok(OAIJson<String>),
    #[oai(status = 400)]
    BadRequest(OAIJson<String>),
    #[oai(status = 500)]
    InternalServerError(OAIJson<String>),
}

#[derive(ApiResponse)]
pub enum MboxListResponse {
    #[oai(status = 200)]
    Ok(OAIJson<Vec<MboxFileInfo>>),
    #[oai(status = 500)]
    InternalServerError(OAIJson<String>),
}

#[derive(ApiResponse)]
pub enum MboxDeleteResponse {
    #[oai(status = 200)]
    Ok(OAIJson<String>),
    #[oai(status = 404)]
    NotFound(OAIJson<String>),
    #[oai(status = 500)]
    InternalServerError(OAIJson<String>),
}

#[OpenApi]
impl ImportApi {
    #[oai(path = "/import/mbox", method = "post", tag = "ApiTags::Import")]
    async fn import_mbox(&self, Json(request): Json<MboxImportRequest>) -> MboxImportResponse {
        let allowed_dir = DATA_DIR_MANAGER.root_dir.join("mbox_import");
        if !allowed_dir.exists() {
            if let Err(e) = std::fs::create_dir_all(&allowed_dir) {
                return MboxImportResponse::InternalServerError(OAIJson(format!(
                    "Failed to create allowed mbox import directory: {}",
                    e
                )));
            }
        }

        let path = match PathBuf::from(&request.path).canonicalize() {
            Ok(path) => path,
            Err(e) => return MboxImportResponse::BadRequest(OAIJson(format!("Invalid path: {}", e))),
        };

        if !path.starts_with(&allowed_dir) {
            return MboxImportResponse::BadRequest(OAIJson(
                "Path is not within the allowed import directory".to_string(),
            ));
        }

        task::spawn(async move {
            if let Err(e) =
                import_mbox_from_path(&path, request.account_id, request.mail_folder).await
            {
                tracing::error!("Failed to import mbox file: {:?}", e);
            }
        });

        MboxImportResponse::Ok(OAIJson("Mbox import started in the background".to_string()))
    }

    #[oai(path = "/import/mbox/:account_id", method = "get", tag = "ApiTags::Import")]
    async fn list_mbox_files(&self, account_id: poem::web::Path<u64>) -> MboxListResponse {
        match MboxFileModel::list_by_account(*account_id).await {
            Ok(files) => {
                let infos: Vec<MboxFileInfo> = files
                    .into_iter()
                    .map(|f| MboxFileInfo {
                        id: f.id,
                        path: f.path,
                        account_id: f.account_id,
                    })
                    .collect();
                MboxListResponse::Ok(OAIJson(infos))
            }
            Err(e) => {
                MboxListResponse::InternalServerError(OAIJson(format!("Failed to list mbox files: {:?}", e)))
            }
        }
    }

    #[oai(path = "/import/mbox/:id", method = "delete", tag = "ApiTags::Import")]
    async fn delete_mbox_file(&self, id: poem::web::Path<u64>) -> MboxDeleteResponse {
        let file_id = *id;
        match MboxFileModel::delete_by_id(file_id).await {
            Ok(_) => MboxDeleteResponse::Ok(OAIJson("Mbox file deleted successfully".to_string())),
            Err(e) => {
                if e.to_string().contains("not found") {
                    MboxDeleteResponse::NotFound(OAIJson(format!("Mbox file not found: {}", file_id)))
                } else {
                    MboxDeleteResponse::InternalServerError(OAIJson(format!("Failed to delete mbox file: {:?}", e)))
                }
            }
        }
    }
}