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

use bichon_core::ext::event_bus::{emit, Event};
use bichon_core::token::AccessTokenModel;
use bichon_core::users::UserModel;
use bichon_core::utils::rate_limit::LOGIN_RATE_LIMITER_MANAGER;
use poem::web::{Json, RealIp};
use poem::{handler, FromRequest, IntoResponse, Request, Response};
use serde::Deserialize;
use tracing::error;

#[derive(Deserialize)]
pub struct LoginPayload {
    pub username: String,
    pub password: String,
}

/// Login endpoint
///
/// Accepts a plain text password and returns the `root_token`
/// on successful authentication.
#[handler]
pub async fn login(payload: Json<LoginPayload>, req: &Request) -> Response {
    let login_username = payload.0.username.clone();

    let ip = RealIp::from_request_without_body(req)
        .await
        .ok()
        .and_then(|r| r.0);
    if let Some(ip_addr) = &ip {
        if LOGIN_RATE_LIMITER_MANAGER.check(&ip_addr.to_string()).await.is_err() {
            return Response::builder()
                .status(http::StatusCode::TOO_MANY_REQUESTS)
                .body("Too many login attempts. Please try again later.".to_string())
                .into_response();
        }
    }

    match UserModel::authenticate_user(payload.0.username, payload.0.password) {
        Ok(result) => {
            // Audit: record the successful login (user + client IP).
            let username = result
                .access_token
                .as_deref()
                .and_then(|t| AccessTokenModel::resolve_user_from_token(t).ok())
                .map(|u| u.username)
                .unwrap_or(login_username);
            if let Some(ip) = ip {
                emit(Event::UserLoggedIn { user: username, ip });
            }
            match serde_json::to_string(&result) {
                Ok(json_string) => Response::builder()
                    .status(http::StatusCode::OK)
                    .content_type("application/json")
                    .body(json_string)
                    .into_response(),
                Err(_) => Response::builder()
                    .status(http::StatusCode::INTERNAL_SERVER_ERROR)
                    .body("Internal server error during response serialization.")
                    .into_response(),
            }
        }
        Err(e) => {
            error!("Authentication failed with system error: {:?}", e);
            Response::builder()
                .status(http::StatusCode::INTERNAL_SERVER_ERROR)
                .body("Authentication system failed.".to_string())
                .into_response()
        }
    }
}
