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

//! Resolve the client address used for rate limiting, IP whitelists, audit
//! events, and request logs.
//!
//! Forwarding headers are attacker-controlled. They are only consulted when
//! the socket peer is a configured trusted reverse proxy; the chain is then
//! walked from right to left (the hop closest to us was appended by our own
//! proxy) and the first untrusted address wins. With no trusted proxies the
//! socket peer is the answer, so a direct client cannot move itself into a
//! fresh rate-limit bucket by sending `X-Forwarded-For`.

use std::{
    net::IpAddr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, LazyLock, RwLock,
    },
};

use bichon_core::{
    settings::{cli::SETTINGS, runtime},
    utils::net::{normalize_ip, TrustedProxies},
};
use poem::{http::HeaderMap, Addr, Request};
use tracing::{error, info, warn};

/// Compiled form of the effective list. The raw entries are kept beside it so
/// a WebUI save is picked up on the next request without a restart, while the
/// common path stays allocation- and parse-free.
static COMPILED: LazyLock<RwLock<Option<(Vec<String>, Arc<TrustedProxies>)>>> =
    LazyLock::new(|| RwLock::new(None));

fn trusted_proxies() -> Arc<TrustedProxies> {
    let entries = runtime::trusted_proxies();
    if let Some((cached, compiled)) = COMPILED.read().unwrap().as_ref() {
        if *cached == entries {
            return compiled.clone();
        }
    }
    let compiled = Arc::new(TrustedProxies::parse(&entries).unwrap_or_else(|reason| {
        // The API validates on save and clap validates on startup; this only
        // triggers if the stored document was edited behind our back.
        error!("Invalid trusted proxy configuration ({reason}); ignoring forwarding headers");
        TrustedProxies::default()
    }));
    *COMPILED.write().unwrap() = Some((entries, compiled.clone()));
    compiled
}

/// Log the effective trust mode and where it came from, once at startup.
pub fn init() {
    let trusted = trusted_proxies();
    let source = if SETTINGS.is_explicitly_set("bichon_trusted_proxies") {
        "startup configuration (CLI/environment)"
    } else if runtime::trusted_proxies_from_webui().is_some() {
        "WebUI runtime override"
    } else {
        "default"
    };

    if trusted.is_trust_all() {
        warn!(
            "BICHON_TRUSTED_PROXIES=* trusts forwarding headers from any peer (legacy compatibility mode, source: {source}). \
             Anyone who can reach the HTTP port can forge their client IP. Use explicit proxy IPs/CIDRs instead."
        );
    } else if trusted.is_empty() {
        info!(
            "No trusted reverse proxies configured (source: {source}): X-Forwarded-For / Forwarded / X-Real-IP are ignored \
             and the socket peer address is used as the client IP."
        );
    } else {
        info!("Trusting forwarding headers only from the configured reverse proxies (source: {source}).");
    }
}

/// The client address for a request, derived from the socket peer and the
/// trusted-proxy configuration.
pub fn client_ip(req: &Request) -> Option<IpAddr> {
    let peer = peer_ip(req);
    let trusted = trusted_proxies();
    warn_if_ignored(peer, req.headers(), &trusted);
    resolve_client_ip(peer, req.headers(), &trusted)
}

/// Pure resolver, exposed for tests and for callers that already decoded the
/// peer address.
pub fn resolve_client_ip(
    peer: Option<IpAddr>,
    headers: &HeaderMap,
    trusted: &TrustedProxies,
) -> Option<IpAddr> {
    let peer = peer.map(normalize_ip);

    // A request without a socket peer did not arrive over the network (the
    // in-process test client is the common case). There is no real address to
    // protect, so keep the historical header behaviour.
    if let Some(peer_addr) = peer {
        if !trusted.is_trusted(peer_addr) {
            return Some(peer_addr);
        }
    }

    if trusted.is_trust_all() {
        return legacy_ip(headers).or(peer);
    }

    let chain = forwarding_chain(headers);
    if chain.is_empty() {
        return peer;
    }

    let mut candidate = peer;
    for ip in chain.into_iter().rev() {
        let ip = normalize_ip(ip);
        if !trusted.is_trusted(ip) {
            return Some(ip);
        }
        candidate = Some(ip);
    }
    candidate
}

pub fn peer_ip(req: &Request) -> Option<IpAddr> {
    match &req.remote_addr().0 {
        Addr::SocketAddr(addr) => Some(addr.ip()),
        _ => None,
    }
}

fn warn_if_ignored(peer: Option<IpAddr>, headers: &HeaderMap, trusted: &TrustedProxies) {
    static WARNED: AtomicBool = AtomicBool::new(false);
    if trusted.is_trust_all() || !has_forwarding_headers(headers) {
        return;
    }
    let Some(peer) = peer else {
        return;
    };
    if trusted.is_trusted(peer) || WARNED.swap(true, Ordering::Relaxed) {
        return;
    }
    warn!(
        "Ignoring forwarding headers from untrusted peer {peer}: configure BICHON_TRUSTED_PROXIES if this request came through a reverse proxy"
    );
}

fn has_forwarding_headers(headers: &HeaderMap) -> bool {
    ["x-forwarded-for", "forwarded", "x-real-ip"]
        .iter()
        .any(|name| headers.contains_key(*name))
}

/// The old (poem `RealIp`) order: X-Real-IP, then the first Forwarded hop,
/// then the first X-Forwarded-For hop. Used only in the explicit `*` mode.
fn legacy_ip(headers: &HeaderMap) -> Option<IpAddr> {
    if let Some(ip) = single_header_ip(headers, "x-real-ip") {
        return Some(ip);
    }
    if let Some(value) = headers.get("forwarded") {
        if let Some(ip) = parse_forwarded(value).into_iter().next() {
            return Some(ip);
        }
    }
    if let Some(value) = headers.get("x-forwarded-for") {
        if let Some(ip) = parse_forwarded_for(value).into_iter().next() {
            return Some(ip);
        }
    }
    None
}

fn forwarding_chain(headers: &HeaderMap) -> Vec<IpAddr> {
    if let Some(value) = headers.get("x-forwarded-for") {
        let chain = parse_forwarded_for(value);
        if !chain.is_empty() {
            return chain;
        }
    }
    if let Some(value) = headers.get("forwarded") {
        let chain = parse_forwarded(value);
        if !chain.is_empty() {
            return chain;
        }
    }
    if let Some(ip) = single_header_ip(headers, "x-real-ip") {
        return vec![ip];
    }
    Vec::new()
}

fn single_header_ip(headers: &HeaderMap, name: &str) -> Option<IpAddr> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .and_then(parse_ip_token)
}

fn parse_forwarded_for(value: &poem::http::HeaderValue) -> Vec<IpAddr> {
    value
        .to_str()
        .ok()
        .into_iter()
        .flat_map(|value| value.split(','))
        .filter_map(parse_ip_token)
        .collect()
}

/// Minimal RFC 7239 parser: we only need the `for=` identifiers.
fn parse_forwarded(value: &poem::http::HeaderValue) -> Vec<IpAddr> {
    let mut out = Vec::new();
    let Ok(value) = value.to_str() else {
        return out;
    };
    for element in value.split(',') {
        for pair in element.split(';') {
            let Some((name, raw)) = pair.split_once('=') else {
                continue;
            };
            if name.trim().eq_ignore_ascii_case("for") {
                if let Some(ip) = parse_ip_token(raw) {
                    out.push(ip);
                }
            }
        }
    }
    out
}

/// Accept `1.2.3.4`, `[2001:db8::1]`, `[2001:db8::1]:443`, `1.2.3.4:8080`,
/// and quoted forms. RFC 7239 obfuscated identifiers (`_hidden`) are skipped.
fn parse_ip_token(raw: &str) -> Option<IpAddr> {
    let raw = raw.trim().trim_matches('"').trim();
    if raw.is_empty() || raw.starts_with('_') {
        return None;
    }
    let candidate = match raw.strip_prefix('[') {
        Some(rest) => rest.split(']').next().unwrap_or(rest).trim(),
        None => raw,
    };
    if let Ok(ip) = candidate.parse::<IpAddr>() {
        return Some(ip);
    }
    if let Some((host, port)) = candidate.rsplit_once(':') {
        if !host.is_empty() && port.chars().all(|c| c.is_ascii_digit()) {
            if let Ok(ip) = host.parse::<IpAddr>() {
                return Some(ip);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use poem::http::{HeaderMap, HeaderValue};

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(
                poem::http::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                HeaderValue::from_str(value).unwrap(),
            );
        }
        map
    }

    fn ip(value: &str) -> IpAddr {
        value.parse().unwrap()
    }

    fn trusted(entries: &[&str]) -> TrustedProxies {
        TrustedProxies::parse(entries.iter().copied()).unwrap()
    }

    #[test]
    fn spoofed_forwarded_for_does_not_change_the_login_rate_limit_key() {
        // The peer is a direct client, not a proxy: eleven different spoofed
        // headers must all resolve to the same address, so the limiter keeps
        // counting against the real client.
        let trusted = TrustedProxies::default();
        let peer = Some(ip("203.0.113.77"));
        let resolved: Vec<_> = (0..11)
            .map(|i| {
                let header = headers(&[("x-forwarded-for", &format!("198.51.100.{i}"))]);
                resolve_client_ip(peer, &header, &trusted).unwrap()
            })
            .collect();
        assert!(resolved.iter().all(|addr| *addr == ip("203.0.113.77")));
    }

    #[test]
    fn spoofed_header_cannot_satisfy_an_ip_whitelist() {
        use std::collections::BTreeSet;

        let trusted = TrustedProxies::default();
        let peer = ip("203.0.113.77");
        let whitelist: BTreeSet<String> = ["10.0.0.5".to_string()].into_iter().collect();
        let header = headers(&[("x-forwarded-for", "10.0.0.5")]);
        let resolved = resolve_client_ip(Some(peer), &header, &trusted).unwrap();
        assert!(!whitelist.contains(&resolved.to_string()));
    }

    #[test]
    fn trusted_proxy_uses_the_rightmost_untrusted_hop() {
        let trusted = trusted(&["10.0.0.0/8"]);
        let peer = Some(ip("10.0.0.2"));
        // The client prepends a lie; our proxy appends the real client address.
        let header = headers(&[("x-forwarded-for", "6.6.6.6, 203.0.113.9")]);
        assert_eq!(
            resolve_client_ip(peer, &header, &trusted),
            Some(ip("203.0.113.9"))
        );
    }

    #[test]
    fn several_trusted_hops_are_skipped() {
        let trusted = trusted(&["10.0.0.0/8"]);
        let peer = Some(ip("10.0.0.2"));
        let header = headers(&[(
            "x-forwarded-for",
            "6.6.6.6, 203.0.113.9, 10.1.2.3",
        )]);
        assert_eq!(
            resolve_client_ip(peer, &header, &trusted),
            Some(ip("203.0.113.9"))
        );
    }

    #[test]
    fn a_chain_of_only_trusted_hops_falls_back_to_the_leftmost() {
        let trusted = trusted(&["10.0.0.0/8"]);
        let peer = Some(ip("10.0.0.2"));
        let header = headers(&[("x-forwarded-for", "10.0.0.3, 10.0.0.4")]);
        assert_eq!(
            resolve_client_ip(peer, &header, &trusted),
            Some(ip("10.0.0.3"))
        );
    }

    #[test]
    fn x_real_ip_and_forwarded_are_supported_from_trusted_peers() {
        let trusted = trusted(&["10.0.0.0/8"]);
        let peer = Some(ip("10.0.0.2"));

        let real_ip = headers(&[("x-real-ip", "198.51.100.20")]);
        assert_eq!(
            resolve_client_ip(peer, &real_ip, &trusted),
            Some(ip("198.51.100.20"))
        );

        let forwarded = headers(&[(
            "forwarded",
            "for=198.51.100.21;proto=https, for=10.0.0.9",
        )]);
        assert_eq!(
            resolve_client_ip(peer, &forwarded, &trusted),
            Some(ip("198.51.100.21"))
        );
    }

    #[test]
    fn ipv6_and_ipv4_mapped_addresses_are_normalised() {
        let trusted_v6 = trusted(&["2001:db8::/32"]);
        let peer = Some(ip("2001:db8::2"));
        let header = headers(&[("x-forwarded-for", "2001:db8::9")]);
        assert_eq!(
            resolve_client_ip(peer, &header, &trusted_v6),
            Some(ip("2001:db8::9"))
        );

        let trusted_mapped = trusted(&["127.0.0.1", "10.0.0.0/8"]);
        let peer = Some(ip("::ffff:10.0.0.2"));
        let header = headers(&[("x-forwarded-for", "::ffff:203.0.113.9")]);
        assert_eq!(
            resolve_client_ip(peer, &header, &trusted_mapped),
            Some(ip("203.0.113.9"))
        );
    }

    #[test]
    fn untrusted_peer_ignores_x_real_ip_too() {
        let trusted = trusted(&["10.0.0.0/8"]);
        let peer = Some(ip("203.0.113.77"));
        let header = headers(&[("x-real-ip", "10.0.0.5")]);
        assert_eq!(
            resolve_client_ip(peer, &header, &trusted),
            Some(ip("203.0.113.77"))
        );
    }

    #[test]
    fn legacy_trust_all_matches_the_old_first_hop_behaviour() {
        let trusted = TrustedProxies::with_trust_all(true);
        let peer = Some(ip("203.0.113.77"));

        let real_ip = headers(&[
            ("x-real-ip", "198.51.100.20"),
            ("x-forwarded-for", "198.51.100.21"),
        ]);
        assert_eq!(
            resolve_client_ip(peer, &real_ip, &trusted),
            Some(ip("198.51.100.20")),
            "X-Real-IP keeps precedence in legacy mode"
        );

        let xff = headers(&[("x-forwarded-for", "198.51.100.21, 10.0.0.9")]);
        assert_eq!(
            resolve_client_ip(peer, &xff, &trusted),
            Some(ip("198.51.100.21")),
            "legacy mode takes the leftmost (old spoofable) address"
        );
    }

    #[test]
    fn malformed_headers_fall_back_to_the_peer() {
        let trusted = trusted(&["10.0.0.0/8"]);
        let peer = Some(ip("10.0.0.2"));
        let header = headers(&[("x-forwarded-for", "unknown, _hidden")]);
        assert_eq!(resolve_client_ip(peer, &header, &trusted), peer);
    }

    #[test]
    fn a_request_without_a_socket_peer_keeps_embedded_header_behaviour() {
        // The in-process test client has no peer address; existing tests rely
        // on the forwarding header there. This path cannot be reached by a
        // network client.
        let trusted = TrustedProxies::default();
        let header = headers(&[("x-forwarded-for", "127.0.0.1")]);
        assert_eq!(
            resolve_client_ip(None, &header, &trusted),
            Some(ip("127.0.0.1"))
        );
    }

    #[tokio::test]
    async fn login_rate_limit_cannot_be_reset_with_spoofed_headers() {
        use crate::rest::public::login::login;
        use poem::{
            http::{uri::Scheme, Method, Uri},
            web::{LocalAddr, RemoteAddr},
            Addr, Body, Endpoint, Request, RequestParts,
        };
        use std::net::SocketAddr;

        crate::tests::setup().await;

        // A peer no other test uses, so the process-wide limiter starts empty
        // for this key. The peer is not a trusted proxy (trust is empty in
        // tests), so the eleven different X-Forwarded-For values must not
        // create fresh buckets.
        let peer: SocketAddr = "203.0.113.88:40000".parse().unwrap();
        let payload = serde_json::json!({
            "username": "spoof-probe",
            "password": "wrong-password",
        });
        let body = serde_json::to_vec(&payload).unwrap();

        let mut statuses = Vec::new();
        for attempt in 0..11 {
            let (http_parts, _) = poem::http::Request::builder()
                .method(Method::POST)
                .uri(Uri::from_static("/api/login"))
                .header("content-type", "application/json")
                .header("x-forwarded-for", format!("198.51.100.{attempt}"))
                .body(())
                .unwrap()
                .into_parts();
            let parts: RequestParts = (
                http_parts,
                LocalAddr::default(),
                RemoteAddr(Addr::socket(peer)),
                Scheme::HTTP,
            )
                .into();
            let req = Request::from_parts(parts, Body::from(body.clone()));
            let response = login.call(req).await.expect("login endpoint must respond");
            statuses.push(response.status().as_u16());
        }

        assert!(
            statuses[..10].iter().all(|status| *status != 429),
            "the limiter must count against the peer, not the spoofed header: {statuses:?}"
        );
        assert_eq!(
            statuses[10], 429,
            "the eleventh attempt must be rate-limited despite a fresh spoofed header each time: {statuses:?}"
        );
    }
}
