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

use std::fmt;

use bichon_core::error::code::ErrorCode;
use bichon_core::error::BichonError;
use poem::error::ResponseError;
use poem::Body;
use poem::{http::StatusCode, Error, Response};
use tracing::error;

use crate::error::code::IntoStatusCode;

pub mod auth;
pub mod client_ip;
pub mod error;
pub mod log;
pub mod recorder;
pub mod status;
pub mod timeout;
pub mod tls;
pub mod validator;

#[derive(Debug)]
pub struct BichonServerError(pub BichonError);

impl fmt::Display for BichonServerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Internal Server Error: {:?}", self.0)
    }
}

impl std::error::Error for BichonServerError {}

impl From<BichonError> for BichonServerError {
    fn from(err: BichonError) -> Self {
        BichonServerError(err)
    }
}

#[inline]
fn create_rust_mailer_error(message: &str, code: ErrorCode) -> BichonServerError {
    BichonError::Generic {
        message: message.into(),
        location: snafu::location!(),
        code,
    }
    .into()
}

#[inline]
pub fn create_api_error_response(message: &str, code: ErrorCode) -> Error {
    let rust_mailer_error = create_rust_mailer_error(message, code);
    rust_mailer_error.into()
}

impl ResponseError for BichonServerError {
    fn status(&self) -> StatusCode {
        match self.0 {
            BichonError::Generic {
                message: _,
                location: _,
                code,
            } => code.status(),
        }
    }

    fn as_response(&self) -> Response
    where
        Self: std::error::Error + Send + Sync + 'static,
    {
        match &self.0 {
            BichonError::Generic {
                message,
                location,
                code,
            } => {
                error!(
                    error_code = code.to_u32(),
                    error_message = %message,
                    error_location = ?location
                );

                let body = Body::from_json(serde_json::json!({
                    "code": code.to_u32(),
                    "message": message.to_string(),
                }))
                .unwrap();

                Response::builder().status(self.status()).body(body)
            }
        }
    }
}
