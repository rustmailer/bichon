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
use bichon_core::dashboard::DashboardStats;
use bichon_core::error::code::ErrorCode;
use bichon_core::ext::event_bus::{emit, Event};
use bichon_core::raise_error;
use bichon_core::settings::cli::SETTINGS;
use bichon_core::settings::proxy::{Proxy, ProxyTestResult};
use bichon_core::settings::{runtime, SystemConfigurations};
use bichon_core::users::permissions::Permission;
use bichon_core::version::{fetch_notifications, Notifications};
use poem_openapi::param::Path;
use poem_openapi::payload::{Json, PlainText};
use poem_openapi::OpenApi;

/// Effective values of the settings that can be overridden from the WebUI.
#[derive(Debug, Clone, poem_openapi::Object)]
pub struct RuntimeSettingsView {
    pub smtp_enabled: bool,
    pub smtp_port: u16,
    pub smtp_encryption: String,
    pub smtp_auth_required: bool,
    pub webui_token_expiration_hours: u32,
    pub sync_concurrency: Option<u16>,
    pub imap_timeout_seconds: u64,
    pub web_mbox_upload_limit_mb: u64,
    pub web_pst_upload_limit_mb: u64,
}

/// Full replacement of the runtime overrides.
#[derive(Debug, Clone, poem_openapi::Object)]
pub struct RuntimeSettingsUpdate {
    pub smtp_enabled: bool,
    pub smtp_port: u16,
    pub smtp_encryption: String,
    pub smtp_auth_required: bool,
    pub webui_token_expiration_hours: u32,
    /// `None` (or `0` from the UI) means automatic.
    pub sync_concurrency: Option<u16>,
    pub imap_timeout_seconds: u64,
    pub web_mbox_upload_limit_mb: u64,
    pub web_pst_upload_limit_mb: u64,
}

#[derive(Debug, Clone, poem_openapi::Object)]
pub struct RuntimeSettingsResult {
    /// True when a listener or the sync pool changed and the service must be
    /// restarted before the new value is used.
    pub restart_required: bool,
    pub settings: RuntimeSettingsView,
}

fn runtime_settings_view() -> RuntimeSettingsView {
    RuntimeSettingsView {
        smtp_enabled: runtime::smtp_enabled(),
        smtp_port: runtime::smtp_port(),
        smtp_encryption: runtime::smtp_encryption().to_string(),
        smtp_auth_required: runtime::smtp_auth_required(),
        webui_token_expiration_hours: runtime::webui_token_expiration_hours(),
        sync_concurrency: runtime::sync_concurrency(),
        imap_timeout_seconds: runtime::imap_timeout_seconds(),
        web_mbox_upload_limit_mb: runtime::web_mbox_upload_limit_mb(),
        web_pst_upload_limit_mb: runtime::web_pst_upload_limit_mb(),
    }
}

fn apply_runtime_settings(
    update: RuntimeSettingsUpdate,
) -> bichon_core::error::BichonResult<RuntimeSettingsResult> {
    if update.smtp_port < 1 {
        return Err(raise_error!(
            "SMTP port must be between 1 and 65535".into(),
            ErrorCode::InvalidParameter
        ));
    }
    if !matches!(update.smtp_encryption.as_str(), "none" | "starttls" | "tls") {
        return Err(raise_error!(
            "SMTP encryption must be one of: none, starttls, tls".into(),
            ErrorCode::InvalidParameter
        ));
    }
    if !(1..=8760).contains(&update.webui_token_expiration_hours) {
        return Err(raise_error!(
            "WebUI token expiration must be between 1 and 8760 hours".into(),
            ErrorCode::InvalidParameter
        ));
    }
    if update.sync_concurrency.is_some_and(|n| n > 1024) {
        return Err(raise_error!(
            "Sync concurrency must be 0 (auto) or at most 1024".into(),
            ErrorCode::InvalidParameter
        ));
    }
    if update.imap_timeout_seconds > 86_400 {
        return Err(raise_error!(
            "IMAP timeout must be at most 86400 seconds".into(),
            ErrorCode::InvalidParameter
        ));
    }
    if !(1..=102_400).contains(&update.web_mbox_upload_limit_mb)
        || !(1..=102_400).contains(&update.web_pst_upload_limit_mb)
    {
        return Err(raise_error!(
            "Upload limits must be between 1 and 102400 MiB".into(),
            ErrorCode::InvalidParameter
        ));
    }

    // 0 from an API client means "auto", same as an empty field in the UI.
    let sync_concurrency = update.sync_concurrency.filter(|n| *n > 0);
    let before = runtime_settings_view();
    // One encrypted document per Save: the memdb encryption derives a key per
    // value, so writing nine entries was nine PBKDF2 rounds (~seconds).
    runtime::store(&runtime::Overrides {
        smtp_enabled: Some(update.smtp_enabled),
        smtp_port: Some(update.smtp_port),
        smtp_encryption: Some(update.smtp_encryption.clone()),
        smtp_auth_required: Some(update.smtp_auth_required),
        webui_token_expiration_hours: Some(update.webui_token_expiration_hours),
        sync_concurrency: Some(update.sync_concurrency.unwrap_or(0)),
        imap_timeout_seconds: Some(update.imap_timeout_seconds),
        web_mbox_upload_limit_mb: Some(update.web_mbox_upload_limit_mb),
        web_pst_upload_limit_mb: Some(update.web_pst_upload_limit_mb),
    })?;

    let settings = RuntimeSettingsView {
        smtp_enabled: update.smtp_enabled,
        smtp_port: update.smtp_port,
        smtp_encryption: update.smtp_encryption.clone(),
        smtp_auth_required: update.smtp_auth_required,
        webui_token_expiration_hours: update.webui_token_expiration_hours,
        sync_concurrency,
        imap_timeout_seconds: update.imap_timeout_seconds,
        web_mbox_upload_limit_mb: update.web_mbox_upload_limit_mb,
        web_pst_upload_limit_mb: update.web_pst_upload_limit_mb,
    };
    let restart_required = before.smtp_enabled != settings.smtp_enabled
        || before.smtp_port != settings.smtp_port
        || before.smtp_encryption != settings.smtp_encryption
        || before.smtp_auth_required != settings.smtp_auth_required
        || before.sync_concurrency != settings.sync_concurrency;
    Ok(RuntimeSettingsResult {
        restart_required,
        settings,
    })
}

pub struct SystemApi;

#[OpenApi(prefix_path = "/api/v1", tag = "ApiTags::System")]
impl SystemApi {
    /// Retrieves important system notifications for the Bichon service.
    ///
    /// This endpoint returns a consolidated view of all critical system notifications including:
    /// - Available version updates
    /// - License expiration warnings
    #[oai(
        method = "get",
        path = "/notifications",
        operation_id = "get_notifications"
    )]
    async fn get_notifications(&self, context: WrappedContext) -> ApiResult<Json<Notifications>> {
        // The default-password reminder is admin-only: revealing to ordinary
        // users that the admin password is still the public default would be
        // a privilege-escalation hint, not a reminder.
        let is_admin = context.has_permission(None, Permission::USER_MANAGE);
        let notification = fetch_notifications(is_admin)
            .await
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        Ok(Json(notification))
    }

    /// Get overall dashboard statistics.
    ///
    /// Returns various aggregated metrics about the mail system, such as
    /// total email count, total storage size, index usage, top senders,
    /// recent activity histogram, and more.
    #[oai(
        method = "get",
        path = "/dashboard-stats",
        operation_id = "get_dashboard_stats"
    )]
    async fn get_dashboard_stats(
        &self,
        context: WrappedContext,
    ) -> ApiResult<Json<DashboardStats>> {
        let stats = DashboardStats::get(context.0).await?;
        Ok(Json(stats))
    }

    /// Get the full list of proxy configurations.
    #[oai(method = "get", path = "/list-proxy", operation_id = "list_proxy")]
    async fn list_proxy(&self, _context: WrappedContext) -> ApiResult<Json<Vec<Proxy>>> {
        //The proxy list is visible to all users.
        let proxies = Proxy::list_all()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        Ok(Json(proxies))
    }

    /// Delete a specific proxy configuration by ID. Requires root permission.
    #[oai(path = "/proxy/:id", method = "delete", operation_id = "remove_proxy")]
    async fn remove_proxy(
        &self,
        /// The ID of the proxy configuration to delete.
        id: Path<u64>,
        context: WrappedContext,
    ) -> ApiResult<()> {
        context.require_permission(None, Permission::ROOT)?;
        let id = id.0;
        let url = Proxy::get(id)
            .ok()
            .map(|p| p.url)
            .unwrap_or_else(|| format!("proxy-{id}"));
        Proxy::delete(id)?;
        emit(Event::ProxyRemoved {
            user: context.user.username.clone(),
            url,
        });
        Ok(())
    }

    /// Retrieve a specific proxy configuration by ID. Requires root permission.
    #[oai(path = "/proxy/:id", method = "get", operation_id = "get_proxy")]
    async fn get_proxy(
        &self,
        /// The ID of the proxy configuration to retrieve.
        id: Path<u64>,
        context: WrappedContext,
    ) -> ApiResult<Json<Proxy>> {
        context.require_permission(None, Permission::ROOT)?;
        Ok(Json(Proxy::get(id.0)?))
    }

    /// Test whether a proxy can reach a geo lookup service. Requires root permission.
    #[oai(path = "/proxy/:id/test", method = "post", operation_id = "test_proxy")]
    async fn test_proxy(
        &self,
        id: Path<u64>,
        context: WrappedContext,
    ) -> ApiResult<Json<ProxyTestResult>> {
        context.require_permission(None, Permission::ROOT)?;
        Ok(Json(Proxy::test(id.0).await?))
    }

    /// Create a new proxy configuration. Requires root permission.
    #[oai(path = "/proxy", method = "post", operation_id = "create_proxy")]
    async fn create_proxy(&self, url: PlainText<String>, context: WrappedContext) -> ApiResult<()> {
        context.require_permission(None, Permission::ROOT)?;
        let url = url.0;
        let entity = Proxy::new(url.clone());
        entity.save()?;
        emit(Event::ProxyCreated {
            user: context.user.username.clone(),
            url,
        });
        Ok(())
    }

    /// Update the URL of a specific proxy by ID. Requires root permission.
    #[oai(path = "/proxy/:id", method = "post", operation_id = "update_proxy")]
    async fn update_proxy(
        &self,
        id: Path<u64>,
        url: PlainText<String>,
        context: WrappedContext,
    ) -> ApiResult<()> {
        context.require_permission(None, Permission::ROOT)?;
        let id = id.0;
        let url = url.0;
        Proxy::update(id, url.clone())?;
        emit(Event::ProxyUpdated {
            user: context.user.username.clone(),
            url,
        });
        Ok(())
    }

    /// Get system configurations.
    ///
    /// Returns a read-only snapshot of the server configuration
    /// resolved at startup. Sensitive values are not exposed.
    #[oai(
        method = "get",
        path = "/system-configurations",
        operation_id = "get_system_configurations"
    )]
    async fn get_system_configurations(
        &self,
        context: WrappedContext,
    ) -> ApiResult<Json<SystemConfigurations>> {
        context.require_permission(None, Permission::ROOT)?;
        let config: SystemConfigurations = SystemConfigurations::from(&*SETTINGS);
        Ok(Json(config))
    }

    /// Effective runtime settings that can be changed from the WebUI.
    #[oai(
        method = "get",
        path = "/runtime-settings",
        operation_id = "get_runtime_settings"
    )]
    async fn get_runtime_settings(
        &self,
        context: WrappedContext,
    ) -> ApiResult<Json<RuntimeSettingsView>> {
        context.require_permission(None, Permission::ROOT)?;
        Ok(Json(runtime_settings_view()))
    }

    /// Replace every runtime override. SMTP and sync-concurrency changes need
    /// a restart; the response says so.
    #[oai(
        method = "post",
        path = "/runtime-settings",
        operation_id = "update_runtime_settings"
    )]
    async fn update_runtime_settings(
        &self,
        context: WrappedContext,
        Json(update): Json<RuntimeSettingsUpdate>,
    ) -> ApiResult<Json<RuntimeSettingsResult>> {
        context.require_permission(None, Permission::ROOT)?;
        Ok(Json(apply_runtime_settings(update)?))
    }

    /// Clear every runtime override and fall back to the startup values.
    #[oai(
        method = "delete",
        path = "/runtime-settings",
        operation_id = "reset_runtime_settings"
    )]
    async fn reset_runtime_settings(
        &self,
        context: WrappedContext,
    ) -> ApiResult<Json<RuntimeSettingsResult>> {
        context.require_permission(None, Permission::ROOT)?;
        runtime::clear()?;
        Ok(Json(RuntimeSettingsResult {
            restart_required: true,
            settings: runtime_settings_view(),
        }))
    }
}
