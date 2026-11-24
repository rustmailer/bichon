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

use crate::common::client_ip;
use bichon_core::ext::event_bus::{emit, Event};
use bichon_core::token::{AccessTokenModel, TokenType};
use bichon_core::users::UserModel;
use poem::{
    handler,
    web::headers::{authorization::Bearer, Authorization, HeaderMapExt},
    IntoResponse, Request, Response,
};
use serde::Deserialize;

#[derive(Deserialize)]
struct Param {
    access_token: String,
}

/// Revoke the access token that carried this request.
///
/// Logout is deliberately outside `ApiGuard` and always answers 204: it must
/// work for a token that is already expired or already gone, and it must
/// never turn "please sign me out" into an error page. A missing or invalid
/// token is a no-op — there is nothing left to revoke.
///
/// The endpoint only deletes the presented token, so Pro/Enterprise dual
/// control cannot gate it: a user must always be able to end their own
/// session.
#[handler]
pub async fn logout(req: &Request) -> Response {
    let bearer = req
        .headers()
        .typed_get::<Authorization<Bearer>>()
        .map(|auth| auth.0.token().to_string())
        .or_else(|| {
            req.params::<Param>()
                .ok()
                .map(|param| param.access_token)
        });

    if let Some(token) = bearer {
        let token = token.trim();
        // Resolve before deleting so the audit row can name the owner. Only
        // WebUI sessions are "logout"; an API token presented here is just
        // revoked and stays out of the login audit trail.
        if let Ok(Some(model)) = AccessTokenModel::find_by_secret(token) {
            if matches!(model.token_type, TokenType::WebUI) {
                let user = UserModel::find(model.user_id)
                    .ok()
                    .flatten()
                    .map(|u| u.username)
                    .unwrap_or_else(|| format!("user-{}", model.user_id));
                emit(Event::UserLoggedOut {
                    user,
                    ip: client_ip::client_ip(req),
                });
            }
        }
        // Ignore the result: absent/already-revoked tokens are success.
        let _ = AccessTokenModel::delete(token);
    }

    Response::builder()
        .status(http::StatusCode::NO_CONTENT)
        .body("")
        .into_response()
}
