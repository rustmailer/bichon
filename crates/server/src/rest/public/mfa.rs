// TOTP two-factor authentication — MFA verification step (community edition).
//
// Complements the password login flow: when a user has TOTP enabled, the
// login endpoint returns a one-time `mfa_challenge` instead of an access
// token. This endpoint validates the 6-digit code (or a one-time recovery
// code) and, on success, issues the real WebUI token.

use bichon_core::database::{delete_impl, find_impl, manager::DB_MANAGER};
use bichon_core::ext::event_bus::{emit, Event};
use bichon_core::token::{AccessTokenModel, TokenType};
use bichon_core::users::{LoginResult, UserModel};
use bichon_core::utils::rate_limit::LOGIN_RATE_LIMITER_MANAGER;
use crate::common::client_ip;
use poem::web::Json;
use poem::{handler, IntoResponse, Request, Response};
use serde::Deserialize;
use tracing::error;

#[derive(Deserialize)]
pub struct MfaVerifyPayload {
    pub challenge: String,
    pub code: String,
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
    let result = LoginResult {
        success: false,
        error_message: Some(message.to_string()),
        ..Default::default()
    };
    login_result_response(&result)
}

#[handler]
pub async fn mfa_verify(payload: Json<MfaVerifyPayload>, req: &Request) -> Response {
    let ip = client_ip::client_ip(req);
    if let Some(ip_addr) = &ip {
        if LOGIN_RATE_LIMITER_MANAGER
            .check(&ip_addr.to_string())
            .await
            .is_err()
        {
            return Response::builder()
                .status(http::StatusCode::TOO_MANY_REQUESTS)
                .body("Too many verification attempts. Please try again later.".to_string())
                .into_response();
        }
    }

    let challenge = payload.0.challenge.trim().to_string();
    let code = payload.0.code.trim().to_string();
    if challenge.is_empty() || code.is_empty() {
        return failure("Missing challenge or verification code.");
    }

    let token = match find_impl::<AccessTokenModel>(DB_MANAGER.db(), &challenge) {
        Ok(Some(token)) => token,
        _ => return failure("Invalid or expired verification challenge."),
    };
    if !matches!(token.token_type, TokenType::MfaChallenge) {
        return failure("Invalid or expired verification challenge.");
    }
    if let Some(expire_at) = token.expire_at {
        if bichon_core::utc_now!() > expire_at {
            return failure("Invalid or expired verification challenge.");
        }
    }

    let user = match UserModel::find(token.user_id) {
        Ok(Some(user)) => user,
        _ => return failure("Invalid or expired verification challenge."),
    };
    if !user.totp_enabled {
        return failure("Two-factor authentication is not enabled for this account.");
    }

    let totp_ok = user.verify_totp_code(&code, 1).unwrap_or(false);
    let recovery_ok = if totp_ok {
        false
    } else {
        user.verify_recovery_code(&code).unwrap_or(false)
    };
    if !totp_ok && !recovery_ok {
        return failure("Invalid verification code.");
    }

    // Consume the one-time challenge regardless of which factor succeeded.
    let _ = delete_impl::<AccessTokenModel>(DB_MANAGER.db(), &challenge);

    let new_token = match AccessTokenModel::reset_webui_token(user.id) {
        Ok(token) => token,
        Err(e) => {
            error!("MFA login failed to issue token: {:#?}", e);
            return failure("Failed to issue access token.");
        }
    };

    if let Some(ip_addr) = &ip {
        emit(Event::UserLoggedIn {
            user: user.username.clone(),
            ip: *ip_addr,
        });
    }

    let result = LoginResult {
        success: true,
        error_message: None,
        access_token: Some(new_token),
        theme: user.theme.clone(),
        language: user.language.clone(),
        mfa_required: false,
        mfa_challenge: None,
    };
    login_result_response(&result)
}