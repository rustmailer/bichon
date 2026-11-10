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

use crate::common::auth::WrappedContext;
use crate::rest::api::ApiTags;
use crate::rest::ApiResult;
use bichon_core::account::grant::BatchAccountRoleRequest;
use bichon_core::account::migration::{AccountModel, AccountType};
use bichon_core::account::payload::{
    filter_accessible_accounts, AccountCreateRequest, AccountUpdateRequest, MinimalAccount,
    RetentionPolicyRequest,
};
use bichon_core::account::state::{DownloadState, GapFillState, RemoteCleanupState};
use bichon_core::archive::imap::cleanup::{
    account_jobs, cancel_job, get_job, start_move_preview, start_move_run, CleanupJob, MoveRequest,
};
use bichon_core::account::stats::AccountStats;
use bichon_core::account::view::AccountResp;
use bichon_core::archive::imap::task::SYNC_TASKS;
use bichon_core::common::paginated::{paginate_vec, DataPage};
use bichon_core::error::code::ErrorCode;
use bichon_core::ext::event_bus::{emit, Event};
use bichon_core::ext::retention_policy::{
    gate_weakening_change, min_retention_allowed, RetentionChange, RetentionGateDecision,
    RetentionValues,
};
use bichon_core::raise_error;
use bichon_core::store::tantivy::envelope::ENVELOPE_MANAGER;
use bichon_core::users::permissions::Permission;
use bichon_core::users::UserModel;
use poem_openapi::param::{Path, Query};
use poem_openapi::payload::{Json, Response as OaiResponse};
use poem_openapi::OpenApi;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

pub struct AccountApi;

/// Request body for `POST /accounts/:account_id/start-download`.
/// `Default` keeps the handler working for older clients that send no body.
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize, poem_openapi::Object)]
pub struct StartDownloadRequest {
    /// When true, run the gap-fill phase (enumerate all UIDs in the download
    /// folders and download anything missing locally) after the incremental
    /// sync. Defaults to false; omitted by older clients.
    #[serde(default)]
    pub run_gap_fill: bool,
}

/// Confirmation body for a manual cleanup move.
#[derive(Clone, Debug, Default, Deserialize, Serialize, poem_openapi::Object)]
pub struct RemoteCleanupExecuteRequest {
    /// Token returned by the move preview.
    pub preview_token: String,
    /// Must be true.
    pub confirm: bool,
    /// Must be true; the source copy leaves the mailbox.
    pub acknowledge_risk: bool,
}

#[OpenApi(prefix_path = "/api/v1", tag = "ApiTags::Account")]
impl AccountApi {
    /// Get account details by account ID
    #[oai(
        path = "/account/:account_id",
        method = "get",
        operation_id = "get_account"
    )]
    async fn get_account(
        &self,
        /// The account ID to retrieve
        account_id: Path<u64>,
        context: WrappedContext,
    ) -> ApiResult<Json<AccountModel>> {
        let account_id = account_id.0;
        context.require_permission(Some(account_id), Permission::ACCOUNT_READ_DETAILS)?;
        Ok(Json(AccountModel::get(account_id)?))
    }

    /// Delete an account by ID - WARNING: This permanently removes the account and all associated resources
    #[oai(
        path = "/account/:account_id",
        method = "delete",
        operation_id = "remove_account"
    )]
    async fn remove_account(
        &self,
        /// The account ID to delete
        account_id: Path<u64>,
        context: WrappedContext,
    ) -> ApiResult<()> {
        let account_id = account_id.0;
        context.require_permission(Some(account_id), Permission::ACCOUNT_MANAGE)?;
        let email = AccountModel::find(account_id)?
            .map(|a| a.email)
            .unwrap_or_else(|| format!("account-{account_id}"));
        AccountModel::delete(account_id).await?;
        emit(Event::AccountRemoved {
            removed_by: context.user.username.clone(),
            account_id,
            email,
        });
        Ok(())
    }

    /// Create a new account
    #[oai(path = "/account", method = "post", operation_id = "create_account")]
    async fn create_account(
        &self,
        /// Account creation request payload
        payload: Json<AccountCreateRequest>,
        context: WrappedContext,
    ) -> ApiResult<Json<AccountModel>> {
        context.require_permission(None, Permission::ACCOUNT_CREATE)?;
        // Retention is a compliance control, not an ordinary account setting:
        // only a system root may create an account with a non-zero window or
        // lock. The minimum lock is additionally Enterprise-only.
        let min_days = payload.0.min_retention_days.unwrap_or(0);
        if payload.0.retention_days.unwrap_or(0) > 0 || min_days > 0 {
            context.require_permission(None, Permission::ROOT)?;
        }
        if min_days > 0 && !min_retention_allowed() {
            return Err(raise_error!(
                "The minimum-retention lock requires an Enterprise license".to_string(),
                ErrorCode::Forbidden
            )
            .into());
        }
        // The view rule (a view delegates read access, so every source needs
        // `account:manage`) is enforced inside `create_account`, where a second
        // caller cannot skip it.
        let account = AccountModel::create_account(context.user.id, payload.0, &context).await?;
        emit(Event::AccountCreated {
            created_by: context.user.username.clone(),
            account_id: account.id,
            email: account.email.clone(),
        });
        if account.retention_days.is_some() || account.min_retention_days.is_some() {
            emit(Event::RetentionPolicyChanged {
                user: context.user.username.clone(),
                account_id: account.id,
                retention_days_before: None,
                retention_days_after: account.retention_days,
                min_retention_days_before: None,
                min_retention_days_after: account.min_retention_days,
            });
        }
        Ok(Json(account))
    }

    /// Update an existing account
    #[oai(
        path = "/account/:account_id",
        method = "post",
        operation_id = "update_account"
    )]
    async fn update_account(
        &self,
        /// The account ID to update
        account_id: Path<u64>,
        /// Account update request payload
        payload: Json<AccountUpdateRequest>,
        context: WrappedContext,
    ) -> ApiResult<()> {
        let account_id = account_id.0;
        context.require_permission(Some(account_id), Permission::ACCOUNT_MANAGE)?;
        // Retention is changed through the dedicated endpoint: a weakening
        // change may need a second signature, and this handler has no way to
        // freeze only the two retention fields out of an arbitrary account
        // update.
        if payload.0.retention_days.is_some()
            || payload.0.clear_retention_days == Some(true)
            || payload.0.min_retention_days.is_some()
            || payload.0.clear_min_retention_days == Some(true)
        {
            return Err(raise_error!(
                "Retention fields are changed via POST /api/v1/account/:account_id/retention"
                    .to_string(),
                ErrorCode::InvalidParameter
            )
            .into());
        }
        // Read the scope *before* the write: the audit record has to say what
        // the view looked like beforehand, and after the update that value is
        // gone. `AccountModel::update` enforces the source-management rule
        // (editing a view can widen it), so a second caller cannot skip it.
        let before = AccountModel::get(account_id)?;
        let before_view = before.view.clone();
        AccountModel::update(account_id, payload.0, &context)?;
        let updated = AccountModel::find(account_id)?;
        let email = updated
            .as_ref()
            .map(|a| a.email.clone())
            .unwrap_or_else(|| format!("account-{account_id}"));
        emit(Event::AccountUpdated {
            updated_by: context.user.username.clone(),
            account_id,
            email,
            view_change: bichon_core::account::migration::ViewConfigChange::between(
                before_view.as_ref(),
                updated.as_ref().and_then(|a| a.view.as_ref()),
            ),
        });
        if let Some(after) = updated.as_ref() {
            if before.retention_days != after.retention_days
                || before.min_retention_days != after.min_retention_days
            {
                emit(Event::RetentionPolicyChanged {
                    user: context.user.username.clone(),
                    account_id,
                    retention_days_before: before.retention_days,
                    retention_days_after: after.retention_days,
                    min_retention_days_before: before.min_retention_days,
                    min_retention_days_after: after.min_retention_days,
                });
            }
        }
        Ok(())
    }

    /// Change an account's retention policy.
    ///
    /// Root-only. A change that weakens protection (shorter maximum retention,
    /// or a shorter/removed minimum-retention lock) may be queued as a
    /// dual-control request; a strengthening change always applies directly.
    /// The retention fields are rejected by the general account-update
    /// endpoint so this is the only path that can alter them.
    #[oai(
        path = "/account/:account_id/retention",
        method = "post",
        operation_id = "update_account_retention"
    )]
    async fn update_account_retention(
        &self,
        account_id: Path<u64>,
        payload: Json<RetentionPolicyRequest>,
        context: WrappedContext,
    ) -> ApiResult<OaiResponse<Json<Value>>> {
        let account_id = account_id.0;
        context.require_permission(None, Permission::ROOT)?;
        let before = AccountModel::get(account_id)?;
        let before_values = RetentionValues {
            retention_days: before.retention_days.filter(|days| *days > 0),
            min_retention_days: before.min_retention_days.filter(|days| *days > 0),
        };
        let (retention_days, min_retention_days) = payload
            .0
            .resolve(before_values.retention_days, before_values.min_retention_days)?;

        // The lock is Enterprise-only. Unchanged values are allowed on a
        // lapsed licence so operators can still edit other settings.
        if min_retention_days != before_values.min_retention_days
            && !min_retention_allowed()
        {
            return Err(raise_error!(
                "The minimum-retention lock requires an Enterprise license".to_string(),
                ErrorCode::Forbidden
            )
            .into());
        }

        let change = RetentionChange {
            account_id,
            before: before_values,
            after: RetentionValues {
                retention_days,
                min_retention_days,
            },
        };
        let decision = if change.is_weakening() {
            gate_weakening_change(
                &change,
                context.user.id,
                &context.user.username,
                context.user.is_admin(),
            )?
        } else {
            RetentionGateDecision::Execute
        };

        match decision {
            RetentionGateDecision::Queued {
                request_id,
                expires_at,
            } => Ok(OaiResponse::new(Json(serde_json::json!({
                "ok": false,
                "pending": true,
                "account_id": account_id,
                "request_id": request_id,
                "expires_at": expires_at,
            })))
            .status(http::StatusCode::ACCEPTED)),
            RetentionGateDecision::Execute => {
                let updated = AccountModel::apply_retention_policy(
                    account_id,
                    retention_days,
                    min_retention_days,
                    &context.user.username,
                )?;
                Ok(OaiResponse::new(Json(serde_json::json!({
                    "ok": true,
                    "pending": false,
                    "account_id": account_id,
                    "retention_days": updated.retention_days,
                    "min_retention_days": updated.min_retention_days,
                }))))
            }
        }
    }

    /// List accounts with optional pagination parameters
    #[oai(path = "/accounts", method = "get", operation_id = "list_accounts")]
    async fn list_accounts(
        &self,
        /// Optional. The page number to retrieve (starting from 1).
        page: Query<Option<u64>>,
        /// Optional. The number of items per page.
        page_size: Query<Option<u64>>,
        /// Optional. Whether to sort the list in descending order.
        desc: Query<Option<bool>>,
        context: WrappedContext,
    ) -> ApiResult<Json<DataPage<AccountResp>>> {
        let is_admin = context.user.is_admin();
        let sort_desc = desc.0.unwrap_or(true);

        let user_map: HashMap<u64, UserModel> = UserModel::list_all()?
            .into_iter()
            .map(|u| (u.id, u))
            .collect();
        let page_data: DataPage<AccountModel> = if is_admin {
            AccountModel::paginate_list(page.0, page_size.0, desc.0)?
        } else {
            let authorized_ids: HashSet<u64> =
                context.user.account_access_map.keys().cloned().collect();

            if authorized_ids.is_empty() {
                return Ok(Json(DataPage {
                    current_page: page.0,
                    page_size: page_size.0,
                    total_items: 0,
                    items: vec![],
                    total_pages: Some(0),
                }));
            }

            let mut accounts: Vec<AccountModel> = AccountModel::list_all()?
                .into_iter()
                .filter(|acct| authorized_ids.contains(&acct.id))
                .collect();

            accounts.sort_by(|a, b| {
                if sort_desc {
                    b.created_at.cmp(&a.created_at)
                } else {
                    a.created_at.cmp(&b.created_at)
                }
            });

            paginate_vec(&accounts, page.0, page_size.0).map(DataPage::from)?
        };

        let items = page_data
            .items
            .into_iter()
            .map(|account| AccountResp::from_model(account, &user_map))
            .collect();

        Ok(Json(DataPage {
            current_page: page_data.current_page,
            page_size: page_data.page_size,
            total_items: page_data.total_items,
            total_pages: page_data.total_pages,
            items,
        }))
    }

    /// Get the running state of an account
    #[oai(
        path = "/accounts/:account_id/download-stats",
        method = "get",
        operation_id = "accounts_download_state"
    )]
    async fn accounts_download_state(
        &self,
        /// The account ID to check state for
        account_id: Path<u64>,
        context: WrappedContext,
    ) -> ApiResult<Json<DownloadState>> {
        let account_id = account_id.0;
        AccountModel::check_account_exists(account_id)?;
        context.require_permission(Some(account_id), Permission::ACCOUNT_READ_DETAILS)?;
        let state = DownloadState::get(account_id)?;
        let state = state.unwrap_or(DownloadState::empty(account_id));
        Ok(Json(state))
    }

    /// Get the gap-fill history of an account (independent of download sessions)
    #[oai(
        path = "/accounts/:account_id/gap-fill-stats",
        method = "get",
        operation_id = "accounts_gap_fill_state"
    )]
    async fn accounts_gap_fill_state(
        &self,
        /// The account ID to check gap-fill state for
        account_id: Path<u64>,
        context: WrappedContext,
    ) -> ApiResult<Json<GapFillState>> {
        let account_id = account_id.0;
        AccountModel::check_account_exists(account_id)?;
        context.require_permission(Some(account_id), Permission::ACCOUNT_READ_DETAILS)?;
        let state = GapFillState::get(account_id)?;
        let state = state.unwrap_or(GapFillState {
            account_id,
            active: None,
            history: Vec::new(),
        });
        Ok(Json(state))
    }

    /// Get the source-mailbox cleanup state of an account
    #[oai(
        path = "/accounts/:account_id/remote-cleanup/state",
        method = "get",
        operation_id = "accounts_remote_cleanup_state"
    )]
    async fn accounts_remote_cleanup_state(
        &self,
        /// The account ID to check cleanup state for
        account_id: Path<u64>,
        context: WrappedContext,
    ) -> ApiResult<Json<RemoteCleanupState>> {
        let account_id = account_id.0;
        AccountModel::check_account_exists(account_id)?;
        context.require_permission(Some(account_id), Permission::ACCOUNT_READ_DETAILS)?;
        let state = RemoteCleanupState::get(account_id)?.unwrap_or(RemoteCleanupState {
            account_id,
            ..Default::default()
        });
        Ok(Json(state))
    }

    /// Starts a read-only move preview as a background job.
    ///
    /// Verifying every candidate takes several IMAP round trips per message,
    /// so the preview can outlive an HTTP request. The response is the job
    /// immediately; poll `GET /accounts/:account_id/remote-cleanup/jobs/:job_id`
    /// for the result and `POST .../cancel` to stop it.
    #[oai(
        path = "/accounts/:account_id/remote-cleanup/preview",
        method = "post",
        operation_id = "accounts_remote_cleanup_preview"
    )]
    async fn accounts_remote_cleanup_preview(
        &self,
        /// The account ID to preview cleanup for
        account_id: Path<u64>,
        context: WrappedContext,
    ) -> ApiResult<Json<CleanupJob>> {
        let account_id = account_id.0;
        let account = AccountModel::check_account_exists(account_id)?;
        context.require_permission(Some(account_id), Permission::ACCOUNT_MANAGE)?;
        if !matches!(account.account_type, AccountType::IMAP) {
            return Err(raise_error!(
                "Source-mailbox cleanup is only supported for IMAP accounts".into(),
                ErrorCode::InvalidParameter
            ))?;
        }
        Ok(Json(start_move_preview(
            account,
            context.user.username.clone(),
        )?))
    }

    /// Starts a previewed move into bichon_auto_created_trash as a background
    /// job. The preview token is still validated by the worker.
    #[oai(
        path = "/accounts/:account_id/remote-cleanup/execute",
        method = "post",
        operation_id = "accounts_remote_cleanup_execute"
    )]
    async fn accounts_remote_cleanup_execute(
        &self,
        /// The account ID to run cleanup for
        account_id: Path<u64>,
        body: Json<RemoteCleanupExecuteRequest>,
        context: WrappedContext,
    ) -> ApiResult<Json<CleanupJob>> {
        let account_id = account_id.0;
        let account = AccountModel::check_account_exists(account_id)?;
        context.require_permission(Some(account_id), Permission::ACCOUNT_MANAGE)?;
        if !body.confirm || !body.acknowledge_risk {
            return Err(raise_error!(
                "cleanup execution requires confirm=true and acknowledge_risk=true".into(),
                ErrorCode::InvalidParameter
            ))?;
        }
        Ok(Json(start_move_run(
            account,
            MoveRequest::manual(context.user.username.clone(), body.preview_token.clone()),
        )?))
    }

    /// List the cleanup jobs still known for an account, newest first.
    ///
    /// Jobs live in memory and stay readable for `JOB_TTL_MS` after finishing.
    /// A client that lost its job id (page reload, dialog reopened) uses this
    /// to reattach to a running job or show its finished report instead of
    /// starting a second one, which the server refuses.
    #[oai(
        path = "/accounts/:account_id/remote-cleanup/jobs",
        method = "get",
        operation_id = "accounts_remote_cleanup_jobs"
    )]
    async fn accounts_remote_cleanup_jobs(
        &self,
        /// The account to list cleanup jobs for
        account_id: Path<u64>,
        context: WrappedContext,
    ) -> ApiResult<Json<Vec<CleanupJob>>> {
        let account_id = account_id.0;
        AccountModel::check_account_exists(account_id)?;
        // A cleanup job can move mail, so its result is not merely account
        // metadata; the same permission that guards reading a single job.
        context.require_permission(Some(account_id), Permission::ACCOUNT_MANAGE)?;
        Ok(Json(account_jobs(account_id)))
    }

    /// Poll a running or recently finished cleanup job.
    ///
    /// `status` is `Running` until the worker ends; a finished job keeps its
    /// result (`preview` or `run`) readable for a while, so a client
    /// that lost the connection can still pick it up. `error` explains a
    /// `Failed` job and a stop reason is attached to `Cancelled` ones.
    #[oai(
        path = "/accounts/:account_id/remote-cleanup/jobs/:job_id",
        method = "get",
        operation_id = "accounts_remote_cleanup_job"
    )]
    async fn accounts_remote_cleanup_job(
        &self,
        /// The account the job belongs to
        account_id: Path<u64>,
        /// The job id returned when it was started
        job_id: Path<String>,
        context: WrappedContext,
    ) -> ApiResult<Json<CleanupJob>> {
        let account_id = account_id.0;
        AccountModel::check_account_exists(account_id)?;
        // A cleanup job can move or delete mail, so its result is not merely
        // account metadata.
        context.require_permission(Some(account_id), Permission::ACCOUNT_MANAGE)?;
        Ok(Json(get_job(account_id, &job_id.0)?))
    }

    /// Ask a running cleanup job to stop. The worker checks between messages,
    /// so the job ends at the next message boundary; poll the job to see the
    /// final status and counters.
    #[oai(
        path = "/accounts/:account_id/remote-cleanup/jobs/:job_id/cancel",
        method = "post",
        operation_id = "accounts_remote_cleanup_job_cancel"
    )]
    async fn accounts_remote_cleanup_job_cancel(
        &self,
        /// The account the job belongs to
        account_id: Path<u64>,
        /// The job id to cancel
        job_id: Path<String>,
        context: WrappedContext,
    ) -> ApiResult<Json<CleanupJob>> {
        let account_id = account_id.0;
        AccountModel::check_account_exists(account_id)?;
        context.require_permission(Some(account_id), Permission::ACCOUNT_MANAGE)?;
        Ok(Json(cancel_job(account_id, &job_id.0)?))
    }

    /// Start a manual download task for an account
    #[oai(
        path = "/accounts/:account_id/start-download",
        method = "post",
        operation_id = "accounts_start_download"
    )]
    async fn accounts_start_download(
        &self,
        /// The account ID to start download for
        account_id: Path<u64>,
        body: Json<StartDownloadRequest>,
        context: WrappedContext,
    ) -> ApiResult<()> {
        let account_id = account_id.0;
        let account = AccountModel::check_account_exists(account_id)?;
        if !matches!(account.account_type, AccountType::IMAP) {
            return Err(raise_error!(
                format!("Manual download is not supported for '{:#?}' accounts. Only IMAP accounts are supported.", account.account_type),
                ErrorCode::InvalidParameter
            ))?;
        }
        context.require_permission(Some(account_id), Permission::ACCOUNT_MANAGE)?;
        SYNC_TASKS.start_manual_task(account_id, body.run_gap_fill).await?;
        emit(Event::AccountDownloadStarted {
            user: context.user.username.clone(),
            account_id,
            run_gap_fill: body.run_gap_fill,
        });
        Ok(())
    }

    /// Cancel a running manual download task
    #[oai(
        path = "/accounts/:account_id/cancel-download",
        method = "post",
        operation_id = "accounts_cancel_download"
    )]
    async fn accounts_cancel_download(
        &self,
        /// The account ID to cancel download for
        account_id: Path<u64>,
        context: WrappedContext,
    ) -> ApiResult<()> {
        let account_id = account_id.0;
        let account = AccountModel::check_account_exists(account_id)?;

        if !matches!(account.account_type, AccountType::IMAP) {
            return Err(raise_error!(
                "This operation is only supported for IMAP accounts.".into(),
                ErrorCode::InvalidParameter
            ))?;
        }
        context.require_permission(Some(account_id), Permission::ACCOUNT_MANAGE)?;

        if !SYNC_TASKS.is_manual_running(account_id).await {
            return Err(raise_error!(
                "No running manual task found for this account.".into(),
                ErrorCode::ResourceNotFound
            ))?;
        }
        SYNC_TASKS.cancel_manual_task(account_id).await;
        emit(Event::AccountDownloadStopped {
            user: context.user.username.clone(),
            account_id,
        });
        Ok(())
    }

    /// Get the stats of an account
    #[oai(
        path = "/accounts/:account_id/stats",
        method = "get",
        operation_id = "accounts_stats"
    )]
    async fn accounts_stats(
        &self,
        /// The account ID to check state for
        account_id: Path<u64>,
        context: WrappedContext,
    ) -> ApiResult<Json<AccountStats>> {
        let account_id = account_id.0;
        AccountModel::check_account_exists(account_id)?;
        context.require_permission(Some(account_id), Permission::ACCOUNT_READ_DETAILS)?;
        let state = ENVELOPE_MANAGER.get_account_stats(account_id)?;
        Ok(Json(state))
    }

    /// Get a minimal list of active accounts for use in selectors when creating account-related resources
    ///
    /// This endpoint provides a lightweight list of accounts containing only essential information (id and name).
    /// It's primarily designed for UI selectors/dropdowns when creating or associating resources with accounts.
    #[oai(
        path = "/minimal-account-list",
        method = "get",
        operation_id = "minimal_accounts_list"
    )]
    async fn minimal_accounts_list(
        &self,
        only_nosync: Query<Option<bool>>,
        context: WrappedContext,
    ) -> ApiResult<Json<Vec<MinimalAccount>>> {
        let is_admin = context.user.is_admin();
        let only_nosync = only_nosync.0.unwrap_or_default();

        let minimal_list = AccountModel::minimal_list(only_nosync)?;
        if is_admin {
            return Ok(Json(minimal_list));
        }

        let authorized_ids: Vec<u64> = context.user.account_access_map.keys().cloned().collect();
        let result = filter_accessible_accounts(&minimal_list, &authorized_ids);
        Ok(Json(result))
    }

    #[oai(path = "/accounts/access/assignments", method = "post")]
    async fn batch_assign_account_role(
        &self,
        req: Json<BatchAccountRoleRequest>,
        context: WrappedContext,
    ) -> ApiResult<()> {
        let req = req.0;
        req.validate_existence()?;
        let role_name = bichon_core::users::role::UserRole::find(req.role_id)?
            .map(|r| r.name)
            .unwrap_or_else(|| format!("role-{}", req.role_id));
        let target_users: Vec<String> = req
            .user_ids
            .iter()
            .filter_map(|uid| UserModel::find(*uid).ok().flatten())
            .map(|u| u.username)
            .collect();
        let account_count = req.account_ids.len();
        req.do_assign(&context)?;
        emit(Event::AccountRoleAssigned {
            user: context.user.username.clone(),
            target_user: target_users.join(", "),
            account_count,
            roles: vec![role_name],
        });
        Ok(())
    }
}
