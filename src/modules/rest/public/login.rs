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

use crate::modules::users::UserModel;
use poem::{handler, web::Json, IntoResponse, Response};
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
pub async fn login(payload: Json<LoginPayload>) -> Response {
    let payload = payload.0;
    match UserModel::authenticate_user(payload.username, payload.password).await {
        Ok(result) => match serde_json::to_string(&result) {
            Ok(json_string) => Response::builder()
                .status(http::StatusCode::OK)
                .content_type("application/json")
                .body(json_string)
                .into_response(),
            Err(_) => Response::builder()
                .status(http::StatusCode::INTERNAL_SERVER_ERROR)
                .body("Internal server error during response serialization.")
                .into_response(),
        },
        Err(e) => {
            error!("Authentication failed with system error: {:?}", e);
            Response::builder()
                .status(http::StatusCode::INTERNAL_SERVER_ERROR)
                .body("Authentication system failed.".to_string())
                .into_response()
        }
    }
}
