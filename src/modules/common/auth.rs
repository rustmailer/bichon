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

use crate::{
    modules::{
        error::{code::ErrorCode, BichonResult},
        token::AccessTokenModel,
        users::{permissions::Permission, role::UserRole, UserModel},
        utils::rate_limit::RATE_LIMITER_MANAGER,
    },
    raise_error,
};
use governor::clock::{Clock, QuantaClock};
use poem::{
    web::{
        headers::{authorization::Bearer, Authorization, HeaderMapExt},
        RealIp,
    },
    Endpoint, FromRequest, Middleware, Request, RequestBody, Result,
};
use serde::Deserialize;
use std::{
    collections::{BTreeSet, HashSet},
    net::IpAddr,
    sync::Arc,
};

use super::create_api_error_response;

pub struct ApiGuard;

pub struct ApiGuardEndpoint<E> {
    ep: E,
}

impl<E: Endpoint> Middleware<E> for ApiGuard {
    type Output = ApiGuardEndpoint<E>;

    fn transform(&self, ep: E) -> Self::Output {
        ApiGuardEndpoint { ep }
    }
}

#[derive(Deserialize)]
struct Param {
    access_token: String,
}

impl<E: Endpoint> Endpoint for ApiGuardEndpoint<E> {
    type Output = E::Output;

    async fn call(&self, mut req: Request) -> Result<Self::Output> {
        let context = authorize_access(&req).await?;
        req.set_data(Arc::new(context));
        self.ep.call(req).await
    }
}

#[derive(Clone, Debug)]
pub struct ClientContext {
    pub ip_addr: Option<IpAddr>,
    pub user: UserModel,
}

impl ClientContext {
    pub async fn require_any_permission(
        &self,
        requirements: Vec<(Option<u64>, &str)>,
    ) -> BichonResult<()> {
        for (account_id, permission) in requirements {
            if self.has_permission(account_id, permission).await {
                return Ok(());
            }
        }
        Err(raise_error!(
            "Access denied: Insufficient permissions to perform this action.".into(),
            ErrorCode::Forbidden
        ))
    }

    pub async fn has_permission(&self, account_id: Option<u64>, permission: &str) -> bool {
        if self.user.is_admin().await {
            return true;
        }

        let mut global_perms = HashSet::new();
        for rid in &self.user.global_roles {
            if let Some(role) = UserRole::find(*rid).await.ok().flatten() {
                global_perms.extend(role.permissions);
            }
        }

        if self.check_global_logic(&global_perms, permission) {
            return true;
        }

        if let Some(aid) = account_id {
            if let Some(role_id) = self.user.account_access_map.get(&aid) {
                if let Some(role) = UserRole::find(*role_id).await.ok().flatten() {
                    if role.permissions.contains(&permission.to_string())
                        || self.check_account_logic(&role.permissions, permission)
                    {
                        return true;
                    }
                }
            }
        }

        false
    }

    fn check_global_logic(&self, global: &HashSet<String>, perm: &str) -> bool {
        if global.contains(perm) {
            return true;
        }

        match perm {
            Permission::DATA_READ => global.contains(Permission::DATA_READ_ALL),
            Permission::DATA_DELETE => global.contains(Permission::DATA_DELETE_ALL),
            Permission::DATA_RAW_DOWNLOAD => global.contains(Permission::DATA_RAW_DOWNLOAD_ALL),
            Permission::DATA_EXPORT_BATCH => global.contains(Permission::DATA_EXPORT_BATCH_ALL),
            Permission::ACCOUNT_MANAGE | Permission::ACCOUNT_READ_DETAILS => {
                global.contains(Permission::ACCOUNT_MANAGE_ALL)
            }
            _ => false,
        }
    }

    fn check_account_logic(&self, scoped_perms: &BTreeSet<String>, perm: &str) -> bool {
        if scoped_perms.contains(perm) {
            return true;
        }
        match perm {
            Permission::DATA_READ | Permission::ACCOUNT_READ_DETAILS => {
                scoped_perms.contains(Permission::ACCOUNT_MANAGE)
            }
            _ => false,
        }
    }

    pub async fn require_permission(
        &self,
        account_id: Option<u64>,
        permission: &str,
    ) -> BichonResult<()> {
        if self.has_permission(account_id, permission).await {
            Ok(())
        } else {
            Err(raise_error!(
                format!("Access Denied: Missing permission '{}'", permission),
                ErrorCode::Forbidden
            ))
        }
    }
}

impl<'a> FromRequest<'a> for ClientContext {
    async fn from_request(req: &'a Request, _body: &mut RequestBody) -> Result<Self> {
        extract_client_context(req).await
    }
}

pub async fn extract_client_context(req: &Request) -> Result<ClientContext> {
    let ip_addr = RealIp::from_request_without_body(req)
        .await
        .map_err(|_| {
            create_api_error_response(
                "Failed to parse client IP address",
                ErrorCode::InvalidParameter,
            )
        })?
        .0
        .ok_or_else(|| {
            create_api_error_response(
                "Failed to parse client IP address",
                ErrorCode::InvalidParameter,
            )
        })?;
    // Extract access token from Bearer header or query params
    let bearer = req
        .headers()
        .typed_get::<Authorization<Bearer>>()
        .map(|auth| auth.0.token().to_string())
        .or_else(|| req.params::<Param>().ok().map(|param| param.access_token));

    let token = bearer.ok_or_else(|| {
        create_api_error_response("Valid access token not found", ErrorCode::PermissionDenied)
    })?;

    // Validate and update access token
    let user = AccessTokenModel::resolve_user_from_token(&token)
        .await
        .map_err(|e| {
            create_api_error_response(&format!("{:#?}", e), ErrorCode::PermissionDenied)
        })?;

    return Ok(ClientContext {
        ip_addr: Some(ip_addr),
        user,
    });
}

pub async fn authorize_access(req: &Request) -> Result<ClientContext, poem::Error> {
    let context = extract_client_context(&req).await?;
    if let Some(access_control) = &context.user.acl {
        if let Some(ip_addr) = context.ip_addr {
            if let Some(whitelist) = &access_control.ip_whitelist {
                if !whitelist.contains(&ip_addr.to_string()) {
                    return Err(create_api_error_response(
                        &format!("IP {} not in whitelist", ip_addr),
                        ErrorCode::Forbidden,
                    ));
                }
            }
        }

        if let Some(rate_limit) = &access_control.rate_limit {
            if let Err(not_until) = RATE_LIMITER_MANAGER
                .check(context.user.id, rate_limit.clone())
                .await
            {
                let wait_duration = not_until.wait_time_from(QuantaClock::default().now());
                return Err(create_api_error_response(
                    &format!(
                        "Rate limit: {}/{}s. Retry after {}s",
                        rate_limit.quota,
                        rate_limit.interval,
                        wait_duration.as_secs()
                    ),
                    ErrorCode::TooManyRequest,
                ));
            }
        }
    }

    Ok(context)
}
