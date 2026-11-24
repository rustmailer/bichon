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


use std::{
    num::NonZeroU32,
    sync::{Arc, LazyLock},
    time::Instant,
};

use governor::{
    clock::{QuantaClock, QuantaInstant},
    middleware::NoOpMiddleware,
    state::InMemoryState,
    Quota, RateLimiter,
};
use poem::{
    http::header, Endpoint, IntoResponse, Middleware, Request, Response, Result,
};

use super::client_ip;
use tracing::{error, info, warn, Instrument};


pub type GovRateLimiter = RateLimiter<
    governor::state::NotKeyed,
    InMemoryState,
    QuantaClock,
    NoOpMiddleware<QuantaInstant>,
>;

static RATE_LIMITER: LazyLock<LogRateLimiter> = LazyLock::new(LogRateLimiter::new);
pub struct LogRateLimiter {
    limiter: Arc<GovRateLimiter>,
}

impl LogRateLimiter {
    pub fn new() -> Self {
        let quota = Quota::per_second(NonZeroU32::new(10).unwrap());
        let limiter = RateLimiter::direct(quota);
        Self {
            limiter: Arc::new(limiter),
        }
    }

    pub async fn should_log(&self, status: u16) -> bool {
        let cost = match status {
            500_u16.. => NonZeroU32::new(1).unwrap(),         // ERROR
            400_u16..=499_u16 => NonZeroU32::new(3).unwrap(), // WARN
            _ => NonZeroU32::new(5).unwrap(),                 // INFO
        };

        self.limiter.check_n(cost).is_ok()
    }
}

#[derive(Default)]
pub struct Tracing;

impl<E: Endpoint> Middleware<E> for Tracing {
    type Output = TracingEndpoint<E>;

    fn transform(&self, ep: E) -> Self::Output {
        TracingEndpoint { inner: ep }
    }
}

/// Endpoint for the `Tracing` middleware.
pub struct TracingEndpoint<E> {
    inner: E,
}

impl<E: Endpoint> Endpoint for TracingEndpoint<E> {
    type Output = Response;

    async fn call(&self, req: Request) -> Result<Self::Output> {
        let remote_addr = client_ip::client_ip(&req)
            .map(|addr| addr.to_string())
            .unwrap_or_else(|| req.remote_addr().to_string());
        let method = req.method().clone();
        let path = req.uri().path().to_string();
        let query = req.uri().query().map(|q| q.to_string());
        let referer = req
            .headers()
            .get(header::REFERER)
            .and_then(|v| v.to_str().ok().map(|v| v.to_string()));
        let content_length = req
            .headers()
            .get(header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok().map(|v| v.to_string()));

        let span = tracing::info_span!(
            "request",
            remote_addr = %remote_addr,
            method = %method,
            path = %path,
            query = ?query,
            referer = ?referer,
            //user_agent = ?user_agent,
            // forwarded = ?forwarded,
            content_length = ?content_length,
        );

        async move {
            let now = Instant::now();
            let res = self.inner.call(req).await;
            let duration = now.elapsed();

            match res {
                Ok(resp) => {
                    let resp = resp.into_response();
                    let status = resp.status().as_u16();
                    log_response(status, duration).await;
                    Ok(resp)
                }
                Err(err) => {
                    let status = err.status().as_u16();
                    log_response(status, duration).await;
                    Err(err)
                }
            }
        }
        .instrument(span)
        .await
    }
}

#[inline]
async fn log_response(status: u16, duration: std::time::Duration) {
    if RATE_LIMITER.should_log(status).await {
        match status {
            500.. => {
                error!(
                    status = %status,
                    duration = ?duration,
                    "request completed with server error"
                );
            }
            400..=499 => {
                warn!(
                    status = %status,
                    duration = ?duration,
                    "request completed with client error"
                );
            }
            _ => {
                info!(
                    status = %status,
                    duration = ?duration,
                    "request completed successfully"
                );
            }
        }
    }
}
