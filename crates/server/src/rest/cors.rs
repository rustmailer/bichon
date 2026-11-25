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

//! CORS policy shared by the community and Pro route trees.
//!
//! The secure default is "no cross-origin access at all". The WebUI is served
//! from the same origin as the API, so it never needs CORS headers — but the
//! browser does send `Origin` on same-origin non-GET requests, and a strict
//! CORS layer would reject those. [`SameOriginPassthrough`] therefore removes
//! the header for requests that came from the app's own origin, before
//! [`cors_middleware`] sees them. Requests from anywhere else are checked
//! against `BICHON_CORS_ORIGINS`.

use std::collections::HashSet;

use bichon_core::settings::cli::SETTINGS;
use poem::http::{header, HeaderName, Method};
use poem::middleware::{Cors, Middleware};
use poem::{Endpoint, IntoResponse, Request, Response, Result};

use crate::common::timeout::TIMEOUT_HEADER;

/// Build the CORS layer.
///
/// - No configured origins: cross-origin requests are rejected outright.
/// - Otherwise: only exact matches are allowed, and only they may carry
///   credentials. The request `Origin` is echoed back for those matches; it is
///   never reflected for an origin that is not on the list.
pub fn cors_middleware(origins: Option<HashSet<String>>, max_age: i32) -> Cors {
    let origins: Vec<String> = origins.unwrap_or_default().into_iter().collect();
    if origins.is_empty() {
        tracing::info!(
            "BICHON_CORS_ORIGINS is not set: cross-origin API access is disabled"
        );
    } else {
        tracing::debug!("CORS: allowed origins = {:?}", origins);
    }

    let allow_credentials = !origins.is_empty();
    Cors::new()
        .allow_origins_fn(move |origin| origins.iter().any(|allowed| allowed == origin))
        .allow_credentials(allow_credentials)
        .allow_methods(&[
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::DELETE,
            Method::OPTIONS,
            Method::HEAD,
            Method::PATCH,
        ])
        .allow_headers(vec!["Content-Type", "Authorization", TIMEOUT_HEADER])
        .expose_headers(vec!["Accept"])
        .max_age(max_age)
}

/// Removes the `Origin` header from same-origin requests so the CORS layer
/// treats them as non-CORS requests. Without this, a strict CORS policy would
/// reject the WebUI's own POST/PUT/DELETE requests, which browsers mark with
/// `Origin` even though no cross-origin access is involved.
pub struct SameOriginPassthrough {
    public_origin: Option<(String, String)>,
}

impl SameOriginPassthrough {
    /// Use the configured public URL as an additional self-origin. This covers
    /// reverse proxies that rewrite `Host` to an internal address while
    /// forwarding (or not forwarding) `X-Forwarded-Host`.
    pub fn from_settings() -> Self {
        Self::new(&SETTINGS.bichon_public_url)
    }

    pub fn new(public_url: &str) -> Self {
        let public_origin = public_url.trim().split_once("://").and_then(|(scheme, rest)| {
            let authority = rest.split('/').next().unwrap_or("").trim();
            if authority.is_empty() {
                None
            } else {
                Some((scheme.to_ascii_lowercase(), authority.to_ascii_lowercase()))
            }
        });
        Self { public_origin }
    }

    fn is_same_origin(&self, req: &Request) -> bool {
        let Some(origin) = req.headers().get(header::ORIGIN).and_then(|v| v.to_str().ok()) else {
            return false;
        };
        let Some((scheme, authority)) = origin.split_once("://") else {
            // "null" (sandboxed documents) and malformed values are never
            // treated as self.
            return false;
        };
        let scheme = scheme.to_ascii_lowercase();

        let forwarded_host = HeaderName::from_static("x-forwarded-host");
        let candidates = [
            req.headers().get(header::HOST).and_then(|v| v.to_str().ok()),
            req.headers().get(&forwarded_host).and_then(|v| v.to_str().ok()),
        ];
        if candidates
            .into_iter()
            .flatten()
            .any(|host| authority_matches(&scheme, authority, host))
        {
            return true;
        }

        match &self.public_origin {
            Some((public_scheme, public_authority)) => {
                public_scheme == &scheme
                    && normalize(&scheme, authority) == normalize(&scheme, public_authority)
            }
            None => false,
        }
    }
}

fn authority_matches(scheme: &str, origin_authority: &str, candidate: &str) -> bool {
    // `X-Forwarded-Host` may contain a comma-separated chain; the first entry
    // is the client-facing host.
    let candidate = candidate.split(',').next().unwrap_or("").trim();
    !candidate.is_empty() && normalize(scheme, origin_authority) == normalize(scheme, candidate)
}

/// Lower-case the authority and drop the scheme's default port so that
/// `https://example.com`, `https://example.com:443` and a `Host` header of
/// `example.com` all compare equal.
fn normalize(scheme: &str, authority: &str) -> String {
    let mut authority = authority.trim().trim_end_matches('/').to_ascii_lowercase();
    let default_port = match scheme {
        "https" => Some(":443"),
        "http" => Some(":80"),
        _ => None,
    };
    if let Some(port) = default_port {
        if let Some(stripped) = authority.strip_suffix(port) {
            authority = stripped.to_string();
        }
    }
    authority
}

impl<E: Endpoint> Middleware<E> for SameOriginPassthrough {
    type Output = SameOriginPassthroughEndpoint<E>;

    fn transform(&self, ep: E) -> Self::Output {
        SameOriginPassthroughEndpoint {
            inner: ep,
            public_origin: self.public_origin.clone(),
        }
    }
}

pub struct SameOriginPassthroughEndpoint<E> {
    inner: E,
    public_origin: Option<(String, String)>,
}

impl<E: Endpoint> Endpoint for SameOriginPassthroughEndpoint<E> {
    type Output = Response;

    async fn call(&self, mut req: Request) -> Result<Self::Output> {
        let passthrough = SameOriginPassthrough {
            public_origin: self.public_origin.clone(),
        };
        if passthrough.is_same_origin(&req) {
            req.headers_mut().remove(header::ORIGIN);
        }
        self.inner.call(req).await.map(IntoResponse::into_response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn same_origin(public_url: &str, origin: &str, host: Option<&str>, forwarded: Option<&str>) -> bool {
        let mut builder = Request::builder()
            .uri_str("/")
            .header(header::ORIGIN, origin);
        if let Some(host) = host {
            builder = builder.header(header::HOST, host);
        }
        if let Some(forwarded) = forwarded {
            builder = builder.header("x-forwarded-host", forwarded);
        }
        SameOriginPassthrough::new(public_url).is_same_origin(&builder.finish())
    }

    #[test]
    fn host_authority_is_self() {
        assert!(same_origin("http://localhost:15630", "http://127.0.0.1:15630", Some("127.0.0.1:15630"), None));
        assert!(same_origin("http://localhost:15630", "https://archive.example.com", Some("archive.example.com"), None));
        assert!(!same_origin("http://localhost:15630", "https://evil.example", Some("archive.example.com"), None));
    }

    #[test]
    fn default_ports_are_normalized() {
        assert!(same_origin("http://localhost", "https://archive.example.com", Some("archive.example.com:443"), None));
        assert!(same_origin("http://localhost", "http://archive.example.com", Some("archive.example.com:80"), None));
        assert!(!same_origin("http://localhost", "https://archive.example.com", Some("archive.example.com:80"), None));
    }

    #[test]
    fn forwarded_host_and_public_url_are_accepted() {
        assert!(same_origin("http://localhost", "https://archive.example.com", Some("10.0.0.5:15630"), Some("archive.example.com")));
        assert!(same_origin("https://archive.example.com", "https://archive.example.com", Some("10.0.0.5:15630"), None));
        assert!(!same_origin("https://archive.example.com", "http://archive.example.com", Some("10.0.0.5:15630"), None));
    }

    #[test]
    fn null_and_malformed_origins_are_not_self() {
        assert!(!same_origin("http://localhost", "null", Some("archive.example.com"), None));
        assert!(!same_origin("http://localhost", "archive.example.com", Some("archive.example.com"), None));
    }
}
