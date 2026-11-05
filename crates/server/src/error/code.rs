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

use bichon_core::error::code::ErrorCode;
use poem::http::StatusCode;

pub trait IntoStatusCode {
    fn status(&self) -> StatusCode;
}

impl IntoStatusCode for ErrorCode {
    fn status(&self) -> StatusCode {
        match self {
            ErrorCode::InvalidParameter
            | ErrorCode::MissingConfiguration
            | ErrorCode::Incompatible => StatusCode::BAD_REQUEST,
            ErrorCode::PermissionDenied => StatusCode::UNAUTHORIZED,
            ErrorCode::AccountDisabled | ErrorCode::OAuth2ItemDisabled | ErrorCode::Forbidden => {
                StatusCode::FORBIDDEN
            }
            ErrorCode::ResourceNotFound => StatusCode::NOT_FOUND,
            // The request itself was fine; the submitted message content
            // could not be parsed (e.g. a broken EML body).
            ErrorCode::EmailParseError => StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::RequestTimeout => StatusCode::REQUEST_TIMEOUT,
            ErrorCode::PayloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            // The IMAP server is rate-limiting us; same meaning as 429 when
            // this bubbles up through the REST layer.
            ErrorCode::TooManyRequest | ErrorCode::ImapServerThrottled => {
                StatusCode::TOO_MANY_REQUESTS
            }
            ErrorCode::AlreadyExists => StatusCode::CONFLICT,
            ErrorCode::InternalError
            | ErrorCode::AutoconfigFetchFailed
            | ErrorCode::ImapCommandFailed
            | ErrorCode::ImapUnexpectedResult
            | ErrorCode::HttpResponseError
            | ErrorCode::ImapAuthenticationFailed
            | ErrorCode::MissingRefreshToken
            | ErrorCode::NetworkError
            | ErrorCode::ConnectionTimeout
            | ErrorCode::ConnectionPoolTimeout
            | ErrorCode::UnhandledPoemError => StatusCode::INTERNAL_SERVER_ERROR,
            ErrorCode::MethodNotAllowed => StatusCode::METHOD_NOT_ALLOWED,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_parameter_is_bad_request() {
        assert_eq!(
            ErrorCode::InvalidParameter.status(),
            StatusCode::BAD_REQUEST
        );
    }

    #[test]
    fn permission_denied_is_unauthorized() {
        assert_eq!(
            ErrorCode::PermissionDenied.status(),
            StatusCode::UNAUTHORIZED
        );
    }

    #[test]
    fn forbidden_is_forbidden() {
        assert_eq!(ErrorCode::Forbidden.status(), StatusCode::FORBIDDEN);
    }

    #[test]
    fn resource_not_found_is_not_found() {
        assert_eq!(
            ErrorCode::ResourceNotFound.status(),
            StatusCode::NOT_FOUND
        );
    }

    #[test]
    fn internal_error_is_internal_server_error() {
        assert_eq!(
            ErrorCode::InternalError.status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[test]
    fn too_many_request_is_429() {
        assert_eq!(
            ErrorCode::TooManyRequest.status(),
            StatusCode::TOO_MANY_REQUESTS
        );
    }

    #[test]
    fn already_exists_is_conflict() {
        assert_eq!(ErrorCode::AlreadyExists.status(), StatusCode::CONFLICT);
    }

    #[test]
    fn method_not_allowed_is_405() {
        assert_eq!(
            ErrorCode::MethodNotAllowed.status(),
            StatusCode::METHOD_NOT_ALLOWED
        );
    }

    #[test]
    fn every_error_code_maps_to_valid_status() {
        // Ensure all variants produce a status code in the 4xx or 5xx range
        let codes = [
            ErrorCode::InvalidParameter,
            ErrorCode::MissingConfiguration,
            ErrorCode::Incompatible,
            ErrorCode::PermissionDenied,
            ErrorCode::AccountDisabled,
            ErrorCode::OAuth2ItemDisabled,
            ErrorCode::Forbidden,
            ErrorCode::ResourceNotFound,
            ErrorCode::EmailParseError,
            ErrorCode::RequestTimeout,
            ErrorCode::PayloadTooLarge,
            ErrorCode::TooManyRequest,
            ErrorCode::ImapServerThrottled,
            ErrorCode::AlreadyExists,
            ErrorCode::InternalError,
            ErrorCode::AutoconfigFetchFailed,
            ErrorCode::ImapCommandFailed,
            ErrorCode::ImapUnexpectedResult,
            ErrorCode::HttpResponseError,
            ErrorCode::ImapAuthenticationFailed,
            ErrorCode::MissingRefreshToken,
            ErrorCode::NetworkError,
            ErrorCode::ConnectionTimeout,
            ErrorCode::ConnectionPoolTimeout,
            ErrorCode::UnhandledPoemError,
            ErrorCode::MethodNotAllowed,
        ];
        for code in &codes {
            let status = code.status();
            assert!(
                status.is_client_error() || status.is_server_error(),
                "{:?} should map to 4xx or 5xx, got {}",
                code,
                status
            );
        }
    }
}
