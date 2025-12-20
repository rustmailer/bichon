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


use crate::modules::common::auth::ClientContext;
use crate::modules::dashboard::DashboardStats;
use crate::modules::error::code::ErrorCode;
use crate::modules::rest::api::ApiTags;
use crate::modules::rest::ApiResult;
use crate::modules::settings::proxy::Proxy;
use crate::modules::version::{fetch_notifications, Notifications};
use crate::raise_error;
use poem_openapi::param::Path;
use poem_openapi::payload::{Json, PlainText};
use poem_openapi::OpenApi;

pub struct SystemApi;

#[OpenApi(prefix_path = "/api/v1", tag = "ApiTags::System")]
impl SystemApi {
    /// Retrieves important system notifications for the RustMail service.
    ///
    /// This endpoint returns a consolidated view of all critical system notifications including:
    /// - Available version updates
    /// - License expiration warnings
    #[oai(
        method = "get",
        path = "/notifications",
        operation_id = "get_notifications"
    )]
    async fn get_notifications(&self) -> ApiResult<Json<Notifications>> {
        let notification = fetch_notifications()
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
    async fn get_dashboard_stats(&self) -> ApiResult<Json<DashboardStats>> {
        let stats = DashboardStats::get().await?;
        Ok(Json(stats))
    }

    /// Get the full list of SOCKS5 proxy configurations.
    #[oai(method = "get", path = "/list-proxy", operation_id = "list_proxy")]
    async fn list_proxy(&self) -> ApiResult<Json<Vec<Proxy>>> {
        let proxies = Proxy::list_all()
            .await
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        Ok(Json(proxies))
    }

    /// Delete a specific proxy configuration by ID. Requires root permission.
    #[oai(path = "/proxy/:id", method = "delete", operation_id = "remove_proxy")]
    async fn remove_proxy(
        &self,
        /// The ID of the proxy configuration to delete.
        id: Path<u64>,
        context: ClientContext,
    ) -> ApiResult<()> {
        context.require_root()?;
        Ok(Proxy::delete(id.0).await?)
    }

    /// Retrieve a specific proxy configuration by ID. Requires root permission.
    #[oai(path = "/proxy/:id", method = "get", operation_id = "get_proxy")]
    async fn get_proxy(
        &self,
        /// The ID of the proxy configuration to retrieve.
        id: Path<u64>,
        context: ClientContext,
    ) -> ApiResult<Json<Proxy>> {
        context.require_root()?;
        Ok(Json(Proxy::get(id.0).await?))
    }

    /// Create a new proxy configuration. Requires root permission.
    #[oai(path = "/proxy", method = "post", operation_id = "create_proxy")]
    async fn create_proxy(&self, url: PlainText<String>, context: ClientContext) -> ApiResult<()> {
        context.require_root()?;
        let entity = Proxy::new(url.0);
        Ok(entity.save().await?)
    }

    /// Update the URL of a specific proxy by ID. Requires root permission.
    #[oai(path = "/proxy/:id", method = "post", operation_id = "update_proxy")]
    async fn update_proxy(
        &self,
        id: Path<u64>,
        url: PlainText<String>,
        context: ClientContext,
    ) -> ApiResult<()> {
        context.require_root()?;
        Ok(Proxy::update(id.0, url.0).await?)
    }
}
