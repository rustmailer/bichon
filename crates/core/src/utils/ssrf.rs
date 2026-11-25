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

//! Outbound-request address policy (SSRF defence).
//!
//! Used by server-side discovery that follows a hostname supplied by a user —
//! currently the autoconfig fetch. Keep the rule simple: only public unicast
//! addresses may be contacted, and only ordinary dotted hostnames may be
//! resolved in the first place. IP literals are refused outright because no
//! real mail provider publishes autoconfig on one.

use std::net::{IpAddr, SocketAddr};

/// True when an address is a public unicast address that a discovery request
/// may connect to. Everything local, shared, reserved or otherwise
/// non-routable is rejected.
pub fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            if v4.is_unspecified()
                || v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_multicast()
            {
                return false;
            }
            let o = v4.octets();
            // 0.0.0.0/8 "this network"
            if o[0] == 0 {
                return false;
            }
            // 100.64.0.0/10 shared address space (some clouds host metadata here)
            if o[0] == 100 && (64..=127).contains(&o[1]) {
                return false;
            }
            // 192.0.0.0/24 IETF protocol assignments
            if o[0] == 192 && o[1] == 0 && o[2] == 0 {
                return false;
            }
            // 198.18.0.0/15 benchmarking
            if o[0] == 198 && (o[1] == 18 || o[1] == 19) {
                return false;
            }
            // 240.0.0.0/4 reserved (includes the limited broadcast address)
            if o[0] >= 240 {
                return false;
            }
            true
        }
        IpAddr::V6(v6) => {
            // ::ffff:a.b.c.d and the deprecated ::a.b.c.d embed an IPv4 address.
            if let Some(v4) = v6.to_ipv4() {
                return is_public_ip(IpAddr::V4(v4));
            }
            if v6.is_unspecified() || v6.is_loopback() || v6.is_multicast() {
                return false;
            }
            let s = v6.segments();
            if (s[0] & 0xfe00) == 0xfc00 {
                // fc00::/7 unique local
                return false;
            }
            if (s[0] & 0xffc0) == 0xfe80 {
                // fe80::/10 link local
                return false;
            }
            if s[0] == 0x2001 && s[1] == 0x0db8 {
                // 2001:db8::/32 documentation
                return false;
            }
            if s[0] == 0x0064 && s[1] == 0xff9b {
                // 64:ff9b::/32 NAT64 (well-known and local-use prefixes)
                return false;
            }
            true
        }
    }
}

/// True when `host` is an ordinary public-looking DNS name. This is a cheap
/// pre-check; the resolved addresses must still pass [`is_public_ip`].
pub fn hostname_is_acceptable(host: &str) -> bool {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() || host.len() > 253 || !host.is_ascii() {
        return false;
    }
    // IP literals (including bracketed IPv6) are never valid discovery hosts.
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    if bare.parse::<IpAddr>().is_ok() {
        return false;
    }
    // A single label is always local (`intranet`, `localhost`, a container name).
    if !host.contains('.') {
        return false;
    }
    const BLOCKED_SUFFIXES: &[&str] = &[
        "localhost",
        "local",
        "localdomain",
        "internal",
        "intranet",
        "home.arpa",
        "lan",
        "corp",
        "home",
    ];
    if BLOCKED_SUFFIXES
        .iter()
        .any(|suffix| host == *suffix || host.ends_with(&format!(".{suffix}")))
    {
        return false;
    }
    // A numeric last label (`127.1`, `0x7f.1` is covered by label syntax below)
    // is an address in a legacy notation, not a domain.
    let tld = host.rsplit('.').next().unwrap_or("");
    if tld.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    host.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-')
    })
}

/// Resolve `host` and return its addresses only when every answer is public.
///
/// Requiring *all* answers to be public matters: a name that mixes a public
/// and a private address is a rebinding attempt, not a legitimate provider.
pub async fn resolve_public_addrs(host: &str, port: u16) -> Option<Vec<IpAddr>> {
    if !hostname_is_acceptable(host) {
        return None;
    }
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    let mut addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), port))
        .await
        .ok()?
        .collect();
    if addrs.is_empty() || !addrs.iter().all(|addr| is_public_ip(addr.ip())) {
        return None;
    }
    // Try IPv4 first: many hosts have an AAAA record but no working IPv6
    // route, and a pinned connection cannot fall back on its own.
    addrs.sort_by_key(|addr| match addr.ip() {
        IpAddr::V4(_) => 0,
        IpAddr::V6(_) => 1,
    });
    Some(addrs.into_iter().map(|addr| addr.ip()).collect())
}

/// Convenience wrapper for callers that only need a yes/no answer (DNS SRV
/// targets, IMAP guesses).
pub async fn host_resolves_public(host: &str, port: u16) -> bool {
    resolve_public_addrs(host, port).await.is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    fn v4(s: &str) -> bool {
        is_public_ip(IpAddr::V4(s.parse::<Ipv4Addr>().unwrap()))
    }

    fn v6(s: &str) -> bool {
        is_public_ip(IpAddr::V6(s.parse::<Ipv6Addr>().unwrap()))
    }

    #[test]
    fn ipv4_ranges() {
        for public in ["8.8.8.8", "1.1.1.1", "172.32.0.1", "100.128.0.1"] {
            assert!(v4(public), "{public} must be public");
        }
        for blocked in [
            "0.0.0.0",
            "0.1.2.3",
            "127.0.0.1",
            "10.0.0.1",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "100.100.100.200",
            "192.0.0.1",
            "192.0.2.1",
            "198.18.0.1",
            "198.51.100.1",
            "203.0.113.1",
            "224.0.0.1",
            "255.255.255.255",
        ] {
            assert!(!v4(blocked), "{blocked} must be blocked");
        }
    }

    #[test]
    fn ipv6_ranges() {
        for public in ["2606:4700:4700::1111", "2001:4860:4860::8888"] {
            assert!(v6(public), "{public} must be public");
        }
        for blocked in [
            "::",
            "::1",
            "fc00::1",
            "fd00:ec2::254",
            "fe80::1",
            "2001:db8::1",
            "ff02::1",
            "::ffff:127.0.0.1",
            "::ffff:169.254.169.254",
            "64:ff9b::7f00:1",
        ] {
            assert!(!v6(blocked), "{blocked} must be blocked");
        }
        assert!(v6("::ffff:8.8.8.8"), "public IPv4-mapped address may pass");
    }

    #[test]
    fn hostname_policy() {
        for ok in ["example.com", "autoconfig.example.com", "example.com.", "xn--bcher-kva.example"] {
            assert!(hostname_is_acceptable(ok), "{ok} must be accepted");
        }
        for bad in [
            "localhost",
            "autoconfig.localhost",
            "mail.local",
            "server.internal",
            "intranet",
            "127.0.0.1",
            "[::1]",
            "127.1",
            "169.254.169.254",
            "example.123",
            "bad..example.com",
            "-bad.example.com",
            "bad-.example.com",
            "exa mple.com",
        ] {
            assert!(!hostname_is_acceptable(bad), "{bad} must be rejected");
        }
    }
}
