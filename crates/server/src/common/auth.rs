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

use bichon_core::{
    common::auth::ClientContext, error::code::ErrorCode, token::AccessTokenModel,
    utils::rate_limit::RATE_LIMITER_MANAGER,
};
use governor::clock::{Clock, QuantaClock};
use poem::{
    web::headers::{authorization::Bearer, Authorization, HeaderMapExt},
    Endpoint, FromRequest, Middleware, Request, RequestBody, Result,
};
use serde::Deserialize;
use std::{ops::Deref, sync::Arc};

use super::{client_ip, create_api_error_response};

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

pub struct WrappedContext(pub ClientContext);

impl Deref for WrappedContext {
    type Target = ClientContext;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'a> FromRequest<'a> for WrappedContext {
    async fn from_request(req: &'a Request, _body: &mut RequestBody) -> Result<Self> {
        let ctx = extract_client_context(req).await?;
        Ok(WrappedContext(ctx))
    }
}

/// The one endpoint a metrics-only token may be used on.
///
/// The route is nested under the configured base path (e.g. `/bichon`), so
/// match by suffix instead of an exact path.
fn is_metrics_endpoint(path: &str) -> bool {
    path.trim_end_matches('/').ends_with("/api/v1/metrics")
}

pub async fn extract_client_context(req: &Request) -> Result<ClientContext> {
    let ip_addr = client_ip::client_ip(req).ok_or_else(|| {
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

    // Validate and update the access token. A metrics-only token is accepted
    // only on the metrics endpoint; every other route uses the strict
    // resolver, which rejects it even though the token itself is valid.
    let user = if is_metrics_endpoint(req.uri().path()) {
        AccessTokenModel::resolve_user_from_token_for_metrics(&token)
    } else {
        AccessTokenModel::resolve_user_from_token(&token)
    }
    .map_err(|e| create_api_error_response(&format!("{:#?}", e), ErrorCode::PermissionDenied))?;

    return Ok(ClientContext {
        ip_addr: Some(ip_addr),
        peer_ip: client_ip::peer_ip(req),
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
