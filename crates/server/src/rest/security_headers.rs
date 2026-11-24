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

//! Global response hardening headers.
//!
//! These headers are a browser-side backstop, not a replacement for input
//! handling: they bound what a successfully injected snippet can do and stop
//! the application from being framed, downgraded, or leaking URLs through
//! `Referer`.
//!
//! Two policies are applied:
//!
//! * The application policy on every response. It is tuned for the SPA
//!   (same-origin scripts, inline styles, data/blob images) and deliberately
//!   permits remote images because the rendered email body — an `srcdoc`
//!   iframe — inherits the embedding document's CSP, and the "show remote
//!   content" toggle must keep working. Scripts stay locked to `'self'`.
//! * A strict, sandboxed policy on inline responses that carry active content
//!   (`text/html`, SVG, XML). These are attachment previews: attacker-supplied
//!   bytes streamed with the attachment's own MIME type. If someone opens the
//!   URL directly it must not run with the application's origin.
//!
//! The documentation UIs (`/api-docs/*`) ship their inline bootstrap bundles
//! from the crate itself, so they get a policy that allows inline script/style
//! but still blocks everything off-origin.

use http::{header, HeaderValue};
use poem::{Endpoint, IntoResponse, Middleware, Request, Response, Result};
use std::sync::LazyLock;

use bichon_core::settings::cli::SETTINGS;

/// CSP applied to the application shell and API responses.
///
/// `img-src`/`media-src` include `http:`/`https:` on purpose: the email body
/// is rendered in a `srcdoc` iframe, which inherits this policy, and the
/// per-message "show remote content" opt-in would otherwise be dead.
pub const APP_CSP: &str = "default-src 'self'; \
     script-src 'self'; \
     style-src 'self' 'unsafe-inline'; \
     img-src 'self' data: blob: http: https:; \
     font-src 'self' data:; \
     connect-src 'self'; \
     media-src 'self' data: blob: http: https:; \
     frame-src 'self' blob:; \
     object-src 'none'; \
     base-uri 'self'; \
     form-action 'self'; \
     frame-ancestors 'none'";

/// CSP for the embedded Swagger UI / Scalar / ReDoc pages. Their HTML inlines
/// the UI bundle and bootstrap script, hence `'unsafe-inline'`; no off-origin
/// loads are needed.
const DOCS_CSP: &str = "default-src 'self'; \
     script-src 'self' 'unsafe-inline'; \
     style-src 'self' 'unsafe-inline'; \
     img-src 'self' data:; \
     font-src 'self' data:; \
     connect-src 'self'; \
     object-src 'none'; \
     base-uri 'none'; \
     frame-ancestors 'none'";

/// CSP for inline attachment previews whose MIME type is active content.
/// `sandbox` gives the document a unique origin and disables scripts, forms
/// and popups even when it is opened as a top-level navigation.
const INLINE_ACTIVE_CONTENT_CSP: &str = "default-src 'none'; \
     img-src data: blob:; \
     style-src 'unsafe-inline'; \
     font-src data:; \
     media-src data: blob:; \
     object-src 'none'; \
     base-uri 'none'; \
     form-action 'none'; \
     frame-ancestors 'none'; \
     sandbox";

/// HSTS is only meaningful when the served origin is HTTPS. It is derived
/// from the configured public URL (which is what clients actually use), or
/// from native TLS being enabled. No `includeSubDomains`/`preload`: the host
/// may share a parent domain with plain-HTTP services.
static HSTS_ENABLED: LazyLock<bool> = LazyLock::new(|| {
    SETTINGS.bichon_public_url.trim().to_ascii_lowercase().starts_with("https://")
        || SETTINGS.bichon_enable_rest_https
});

const HSTS_VALUE: &str = "max-age=15552000";

pub struct SecurityHeaders;

pub struct SecurityHeadersEndpoint<E> {
    ep: E,
}

impl<E: Endpoint> Middleware<E> for SecurityHeaders {
    type Output = SecurityHeadersEndpoint<E>;

    fn transform(&self, ep: E) -> Self::Output {
        SecurityHeadersEndpoint { ep }
    }
}

impl<E: Endpoint> Endpoint for SecurityHeadersEndpoint<E> {
    type Output = Response;

    async fn call(&self, req: Request) -> Result<Self::Output> {
        // This middleware sits outside `nest(base_url, ...)`, so the URI is
        // still the full request path.
        let path = req.uri().path().to_string();

        match self.ep.call(req).await {
            Ok(response) => {
                let mut response = response.into_response();
                apply_headers(&mut response, &path);
                Ok(response)
            }
            Err(error) => Err(error),
        }
    }
}

fn apply_headers(response: &mut Response, path: &str) {
    let is_docs = path.contains("/api-docs/");

    let csp = if is_docs { DOCS_CSP } else { APP_CSP };
    set_if_absent(
        response,
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(csp),
    );
    set_if_absent(
        response,
        header::X_FRAME_OPTIONS,
        HeaderValue::from_static("DENY"),
    );
    set_if_absent(
        response,
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    set_if_absent(
        response,
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    if *HSTS_ENABLED {
        set_if_absent(
            response,
            header::STRICT_TRANSPORT_SECURITY,
            HeaderValue::from_static(HSTS_VALUE),
        );
    }

    // Attachment previews stream the archived part with its original MIME
    // type. HTML/SVG/XML previews are documents and need the sandbox policy;
    // the application CSP above is too permissive for attacker bytes.
    if is_inline_active_content(response) {
        response.headers_mut().insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(INLINE_ACTIVE_CONTENT_CSP),
        );
    }
}

fn set_if_absent(response: &mut Response, name: header::HeaderName, value: HeaderValue) {
    if !response.headers().contains_key(&name) {
        response.headers_mut().insert(name, value);
    }
}

fn is_inline_active_content(response: &Response) -> bool {
    let disposition_is_inline = response
        .headers()
        .get(header::CONTENT_DISPOSITION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .trim_start()
                .get(..6)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("inline"))
        });

    if !disposition_is_inline {
        return false;
    }

    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let mime = content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();

    matches!(
        mime.as_str(),
        "text/html"
            | "application/xhtml+xml"
            | "image/svg+xml"
            | "application/xml"
            | "text/xml"
            | "application/rss+xml"
            | "application/atom+xml"
    )
}

/// A fresh CSP nonce for the entry HTML, whose inline bootstrap script sets
/// `window.__BICHON_BASE__`. UUIDv4 is 122 random bits — sufficient here and
/// already a workspace dependency.
pub fn new_nonce() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// The application CSP with a per-response nonce added to `script-src`.
pub fn index_csp(nonce: &str) -> String {
    APP_CSP.replacen(
        "script-src 'self'",
        &format!("script-src 'self' 'nonce-{nonce}'"),
        1,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use poem::Body;

    fn plain_response() -> Response {
        Response::builder().body(Body::empty())
    }

    fn html_response(inline: bool) -> Response {
        let disposition = if inline { "inline" } else { "attachment" };
        Response::builder()
            .content_type("text/html; charset=utf-8")
            .header(header::CONTENT_DISPOSITION, disposition)
            .body(Body::empty())
    }

    fn csp(response: &Response) -> &str {
        response
            .headers()
            .get(header::CONTENT_SECURITY_POLICY)
            .and_then(|value| value.to_str().ok())
            .expect("CSP header")
    }

    #[test]
    fn global_headers_are_added() {
        let mut response = plain_response();
        apply_headers(&mut response, "/api/status");
        assert_eq!(csp(&response), APP_CSP);
        assert_eq!(
            response.headers()[header::X_FRAME_OPTIONS].to_str().unwrap(),
            "DENY"
        );
        assert_eq!(
            response.headers()[header::REFERRER_POLICY].to_str().unwrap(),
            "no-referrer"
        );
        assert_eq!(
            response.headers()[header::X_CONTENT_TYPE_OPTIONS]
                .to_str()
                .unwrap(),
            "nosniff"
        );
    }

    #[test]
    fn an_existing_csp_is_preserved() {
        // The entry HTML sets its own nonce-bound policy.
        let mut response = Response::builder()
            .header(header::CONTENT_SECURITY_POLICY, "custom-policy")
            .body(Body::empty());
        apply_headers(&mut response, "/");
        assert_eq!(csp(&response), "custom-policy");
    }

    #[test]
    fn docs_routes_get_an_inline_friendly_policy() {
        let mut response = plain_response();
        apply_headers(&mut response, "/bichon/api-docs/swagger/");
        assert_eq!(csp(&response), DOCS_CSP);
        assert!(csp(&response).contains("'unsafe-inline'"));
    }

    #[test]
    fn inline_html_previews_are_sandboxed() {
        let mut response = html_response(true);
        apply_headers(&mut response, "/api/v1/preview-attachment/1/x");
        assert_eq!(csp(&response), INLINE_ACTIVE_CONTENT_CSP);
        assert!(csp(&response).ends_with("sandbox"));
    }

    #[test]
    fn inline_svg_is_sandboxed() {
        let mut response = Response::builder()
            .content_type("image/svg+xml")
            .header(header::CONTENT_DISPOSITION, "inline; filename=\"x\"")
            .body(Body::empty());
        apply_headers(&mut response, "/api/v1/preview-attachment/1/x");
        assert_eq!(csp(&response), INLINE_ACTIVE_CONTENT_CSP);
    }

    #[test]
    fn downloads_and_passive_previews_keep_the_app_policy() {
        // `attachment` disposition never renders as a document.
        let mut download = html_response(false);
        apply_headers(&mut download, "/api/v1/download-attachment/1/x");
        assert_eq!(csp(&download), APP_CSP);

        // Inline PDFs/images are not active content.
        let mut pdf = Response::builder()
            .content_type("application/pdf")
            .header(header::CONTENT_DISPOSITION, "inline")
            .body(Body::empty());
        apply_headers(&mut pdf, "/api/v1/preview-attachment/1/x");
        assert_eq!(csp(&pdf), APP_CSP);
    }

    #[test]
    fn index_csp_is_bound_to_the_nonce() {
        let csp = index_csp("abc123");
        assert!(csp.contains("script-src 'self' 'nonce-abc123'"));
        // Nonce policies must not allow inline script wholesale.
        assert!(!csp.contains("script-src 'self' 'unsafe-inline'"));
    }
}
