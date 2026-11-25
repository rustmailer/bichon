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

// Forced password change for the built-in administrator.
//
// A local-password login while the default administrator password is still in
// use returns no session: only a short-lived, one-time ticket. This endpoint
// is the only thing that ticket unlocks, so the password must be changed
// before any WebUI token exists.

use bichon_core::users::{LoginResult, UserModel};
use bichon_core::utils::rate_limit::LOGIN_RATE_LIMITER_MANAGER;
use crate::common::client_ip;
use poem::web::Json;
use poem::{handler, IntoResponse, Request, Response};
use serde::Deserialize;
use tracing::error;

#[derive(Deserialize)]
pub struct ChangePasswordPayload {
    pub ticket: String,
    pub new_password: String,
}

fn login_result_response(result: &LoginResult) -> Response {
    match serde_json::to_string(result) {
        Ok(json_string) => Response::builder()
            .status(http::StatusCode::OK)
            .content_type("application/json")
            .body(json_string)
            .into_response(),
        Err(_) => Response::builder()
            .status(http::StatusCode::INTERNAL_SERVER_ERROR)
            .body("Internal server error during response serialization.".to_string())
            .into_response(),
    }
}

fn failure(message: &str) -> Response {
    login_result_response(&LoginResult {
        success: false,
        error_message: Some(message.to_string()),
        ..Default::default()
    })
}

#[handler]
pub async fn change_password(payload: Json<ChangePasswordPayload>, req: &Request) -> Response {
    let ip = client_ip::client_ip(req);
    if let Some(ip_addr) = &ip {
        if LOGIN_RATE_LIMITER_MANAGER
            .check(&ip_addr.to_string())
            .await
            .is_err()
        {
            return Response::builder()
                .status(http::StatusCode::TOO_MANY_REQUESTS)
                .body("Too many attempts. Please try again later.".to_string())
                .into_response();
        }
    }

    let ticket = payload.0.ticket.trim();
    if ticket.is_empty() {
        return failure("Missing password change ticket.");
    }

    match UserModel::complete_password_change(ticket, &payload.0.new_password) {
        Ok(result) => login_result_response(&result),
        Err(e) => {
            error!("Forced password change failed: {:#?}", e);
            failure(&e.to_string())
        }
    }
}
