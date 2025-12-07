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


use poem::http::StatusCode;
use poem_openapi::Enum;

#[derive(Copy, Clone, Debug, Enum, Eq, PartialEq)]
#[repr(u32)]
pub enum ErrorCode {
    // Client-side errors (10000–10999)
    InvalidParameter = 10000,
    MissingConfiguration = 10020,
    Incompatible = 10030,
    PayloadTooLarge = 10070,
    RequestTimeout = 10080,
    MethodNotAllowed = 10090,

    // Authentication and authorization errors (20000–20999)
    PermissionDenied = 20000,
    AccountDisabled = 20010,
    OAuth2ItemDisabled = 20050,
    MissingRefreshToken = 20060,

    // Resource errors (30000–30999)
    ResourceNotFound = 30000,
    TooManyRequest = 30020,

    // Network connection errors (40000–40999)
    NetworkError = 40000,
    ConnectionTimeout = 40010,
    ConnectionPoolTimeout = 40020,
    HttpResponseError = 40030,

    // Mail service errors (50000–50999)
    ImapCommandFailed = 50000,
    ImapAuthenticationFailed = 50010,
    ImapUnexpectedResult = 50020,
    AutoconfigFetchFailed = 50060,
    // Internal system errors (70000–70999)
    InternalError = 70000,
    UnhandledPoemError = 70010,
    IoError = 70020,
}

impl ErrorCode {
    pub fn status(&self) -> StatusCode {
        match self {
            ErrorCode::InvalidParameter
            | ErrorCode::MissingConfiguration
            | ErrorCode::Incompatible => StatusCode::BAD_REQUEST,
            ErrorCode::PermissionDenied => StatusCode::UNAUTHORIZED,
            ErrorCode::AccountDisabled | ErrorCode::OAuth2ItemDisabled => StatusCode::FORBIDDEN,
            ErrorCode::ResourceNotFound => StatusCode::NOT_FOUND,
            ErrorCode::RequestTimeout => StatusCode::REQUEST_TIMEOUT,
            ErrorCode::PayloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            ErrorCode::TooManyRequest => StatusCode::TOO_MANY_REQUESTS,
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
            | ErrorCode::UnhandledPoemError
            | ErrorCode::IoError => StatusCode::INTERNAL_SERVER_ERROR,
            ErrorCode::MethodNotAllowed => StatusCode::METHOD_NOT_ALLOWED,
        }
    }
}
