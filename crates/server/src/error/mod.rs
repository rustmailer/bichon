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

use crate::error::code::IntoStatusCode;
use bichon_core::error::BichonError;
use poem::http::StatusCode;
use poem_openapi::{payload::Json, ApiResponse, Object};
use std::fmt::Formatter;

pub mod code;
pub mod handler;

#[derive(Debug, Clone, Object)]
pub struct ApiError {
    pub message: String,
    pub code: u32,
}

impl From<BichonError> for ApiErrorResponse {
    fn from(error: BichonError) -> Self {
        match error {
            BichonError::Generic {
                message,
                location,
                code,
            } => {
                tracing::error!(
                    "API error occurred: [{:#?}] {} at {:?}",
                    code,
                    message,
                    location
                );
                let api_error = ApiError {
                    message,
                    code: code as u32,
                };
                ApiErrorResponse::Generic(code.status(), Json(api_error))
            }
        }
    }
}

impl ApiError {
    pub fn new(message: String, code: u32) -> Self {
        Self { message, code }
    }

    pub fn new_with_error_code<ErrorType: std::fmt::Display>(
        error: ErrorType,
        code: u32,
    ) -> ApiError {
        Self::new(format!("{:#}", error), code)
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "Error({}): {}", self.code, self.message)
    }
}

impl std::error::Error for ApiError {}

#[derive(Debug, Clone, ApiResponse)]
pub enum ApiErrorResponse {
    Generic(StatusCode, Json<ApiError>),
}
