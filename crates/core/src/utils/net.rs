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

use crate::error::code::ErrorCode;
use crate::raise_error;
use crate::settings::proxy::Proxy;
use crate::utils::tls::establish_tls_stream;
use crate::{error::BichonResult, imap::session::SessionStream};
use base64::{engine::general_purpose, Engine as _};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::pin::Pin;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_io_timeout::TimeoutStream;
use tokio_socks::tcp::Socks5Stream;
use tracing::error;

pub(crate) const TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ProxyScheme {
    Socks5,
    Http,
}

impl ProxyScheme {
    fn as_str(self) -> &'static str {
        match self {
            Self::Socks5 => "socks5",
            Self::Http => "http",
        }
    }
}

/// Parsed proxy address components.
#[derive(Debug, Clone)]
pub struct ProxyAddr {
    pub scheme: ProxyScheme,
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
}

impl ProxyAddr {
    pub fn standard_url(&self) -> String {
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };

        if let (Some(user), Some(pass)) = (&self.username, &self.password) {
            format!(
                "{}://{}:{}@{}:{}",
                self.scheme.as_str(),
                user,
                pass,
                host,
                self.port
            )
        } else {
            format!("{}://{}:{}", self.scheme.as_str(), host, self.port)
        }
    }
}

pub(crate) async fn establish_tcp_connection_with_timeout(
    address: SocketAddr,
    use_proxy: Option<u64>,
) -> BichonResult<Pin<Box<TimeoutStream<TcpStream>>>> {
    // Establish the TCP connection with a timeout
    let tcp_stream = connect_with_optional_proxy(use_proxy, address).await?;
    let mut timeout_stream = TimeoutStream::new(tcp_stream);

    // Set read and write timeouts. The read timeout bounds how long the sync
    // task blocks waiting for a slow IMAP server between commands; a hung
    // server therefore surfaces as a network error (and retry) instead of a
    // silently stuck download. 0 disables the read timeout (server decides).
    let read_timeout = crate::settings::runtime::imap_timeout_seconds()
        .checked_sub(1)
        .map(|seconds| Duration::from_secs(seconds.max(1)));
    timeout_stream.set_write_timeout(Some(Duration::from_secs(15)));
    timeout_stream.set_read_timeout(read_timeout);

    // Return the timeout-wrapped TCP stream as a Pin
    Ok(Box::pin(timeout_stream))
}

pub async fn establish_tls_connection(
    address: SocketAddr,
    server_hostname: &str,
    alpn_protocols: &[&str],
    use_proxy: Option<u64>,
    dangerous: bool,
) -> BichonResult<impl SessionStream> {
    // Establish the TCP connection with timeout
    let tcp_stream = establish_tcp_connection_with_timeout(address, use_proxy).await?;

    // Wrap the TCP stream with TLS encryption
    let tls_stream =
        establish_tls_stream(server_hostname, alpn_protocols, tcp_stream, dangerous).await?;

    // Return the TLS stream wrapped in a SessionStream
    Ok(tls_stream)
}

/// Parse a proxy URL into its components.
///
/// Supports two formats:
/// - **Standard**: `[scheme://][user:pass@]host:port`
/// - **Non-standard** (some proxy providers): `[scheme://]host:port:username:password`
///
/// The distinguishing feature is the `@` sign in the standard format.
pub fn parse_proxy_url(input: &str) -> BichonResult<ProxyAddr> {
    // Normalize and strip scheme prefix
    let (scheme, stripped) = if let Some(rest) = input
        .strip_prefix("socks5://")
        .or_else(|| input.strip_prefix("SOCKS5://"))
        .or_else(|| input.strip_prefix("Socks5://"))
    {
        (ProxyScheme::Socks5, rest)
    } else if let Some(rest) = input
        .strip_prefix("http://")
        .or_else(|| input.strip_prefix("HTTP://"))
        .or_else(|| input.strip_prefix("Http://"))
    {
        (ProxyScheme::Http, rest)
    } else {
        return Err(raise_error!(
            "Invalid proxy URL: must start with 'http://' or 'socks5://'".into(),
            ErrorCode::InvalidParameter
        ));
    };

    if stripped.is_empty() {
        return Err(raise_error!(
            "Proxy URL has empty address after scheme.".into(),
            ErrorCode::InvalidParameter
        ));
    }

    // Check for standard format: user:pass@host:port
    if let Some(at_pos) = stripped.rfind('@') {
        let userinfo = &stripped[..at_pos];
        let hostport = &stripped[at_pos + 1..];

        let (username, password) = split_userinfo(userinfo)?;
        let (host, port) = split_hostport(hostport)?;

        return Ok(ProxyAddr {
            scheme,
            host,
            port,
            username,
            password,
        });
    }

    // No '@' — check for non-standard format: host:port:user:pass
    if stripped.starts_with('[') {
        let (host, port) = split_hostport(stripped)?;
        return Ok(ProxyAddr {
            scheme,
            host,
            port,
            username: None,
            password: None,
        });
    }

    let mut parts = stripped.split(':');
    match (
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
    ) {
        (Some(_), Some(_), None, None, None) => {
            // host:port, no auth
            let (host, port) = split_hostport(stripped)?;
            Ok(ProxyAddr {
                scheme,
                host,
                port,
                username: None,
                password: None,
            })
        }
        (Some(host), Some(port), Some(username), Some(password), None) => {
            // Non-standard: host:port:username:password
            let port = port.parse::<u16>().map_err(|_| {
                raise_error!(
                    format!("Invalid port '{}' in proxy URL.", port),
                    ErrorCode::InvalidParameter
                )
            })?;

            if host.is_empty() {
                return Err(raise_error!(
                    "Empty hostname in proxy URL.".into(),
                    ErrorCode::InvalidParameter
                ));
            }
            if host.contains(':') || host.contains('[') || host.contains(']') {
                return Err(raise_error!(
                    "IPv6 proxy hosts are not supported.".into(),
                    ErrorCode::InvalidParameter
                ));
            }
            if username.is_empty() {
                return Err(raise_error!(
                    "Empty username in proxy URL.".into(),
                    ErrorCode::InvalidParameter
                ));
            }
            if password.is_empty() {
                return Err(raise_error!(
                    "Empty password in proxy URL.".into(),
                    ErrorCode::InvalidParameter
                ));
            }

            Ok(ProxyAddr {
                scheme,
                host: host.to_string(),
                port,
                username: Some(username.to_string()),
                password: Some(password.to_string()),
            })
        }
        _ => Err(raise_error!(
            "Invalid proxy URL format. Expected '[scheme://][user:pass@]host:port' or 'scheme://host:port:user:pass'.".into(),
            ErrorCode::InvalidParameter
        )),
    }
}

/// Split "user:pass" into (Some(user), Some(pass)).
fn split_userinfo(userinfo: &str) -> BichonResult<(Option<String>, Option<String>)> {
    if userinfo.is_empty() {
        return Ok((None, None));
    }
    if let Some(colon_pos) = userinfo.find(':') {
        let user = &userinfo[..colon_pos];
        let pass = &userinfo[colon_pos + 1..];
        if user.is_empty() {
            return Err(raise_error!(
                "Empty username in proxy URL credentials.".into(),
                ErrorCode::InvalidParameter
            ));
        }
        if pass.is_empty() {
            return Err(raise_error!(
                "Empty password in proxy URL credentials.".into(),
                ErrorCode::InvalidParameter
            ));
        }
        Ok((Some(user.to_string()), Some(pass.to_string())))
    } else {
        Err(raise_error!(
            "Password cannot be empty when username is provided.".into(),
            ErrorCode::InvalidParameter
        ))
    }
}

/// Split "host:port" into (host, port). Bracketed IPv6 is accepted.
fn split_hostport(hostport: &str) -> BichonResult<(String, u16)> {
    if hostport.is_empty() {
        return Err(raise_error!(
            "Empty host:port in proxy URL.".into(),
            ErrorCode::InvalidParameter
        ));
    }

    if let Some(rest) = hostport.strip_prefix('[') {
        let Some(close_bracket) = rest.find(']') else {
            return Err(raise_error!(
                format!("Invalid IPv6 address in proxy URL: '{}'.", hostport),
                ErrorCode::InvalidParameter
            ));
        };
        let host = &rest[..close_bracket];
        let port_text = rest[close_bracket + 1..].strip_prefix(':').ok_or_else(|| {
            raise_error!(
                format!(
                    "Missing port after IPv6 address in proxy URL: '{}'.",
                    hostport
                ),
                ErrorCode::InvalidParameter
            )
        })?;
        let port = port_text.parse::<u16>().map_err(|_| {
            raise_error!(
                format!("Invalid port in proxy URL: '{}'.", hostport),
                ErrorCode::InvalidParameter
            )
        })?;

        return Ok((host.to_string(), port));
    }

    // hostname:port or ip:port — split from right
    let last_colon = hostport.rfind(':').ok_or_else(|| {
        raise_error!(
            format!("Missing port in proxy URL: '{}'.", hostport),
            ErrorCode::InvalidParameter
        )
    })?;
    let host = hostport[..last_colon].to_string();
    let port = hostport[last_colon + 1..].parse::<u16>().map_err(|_| {
        raise_error!(
            format!("Invalid port in proxy URL: '{}'.", hostport),
            ErrorCode::InvalidParameter
        )
    })?;

    if host.is_empty() {
        return Err(raise_error!(
            "Empty hostname in proxy URL.".into(),
            ErrorCode::InvalidParameter
        ));
    }
    if host.contains(':') || host.contains('[') || host.contains(']') {
        return Err(raise_error!(
            "IPv6 proxy hosts are not supported.".into(),
            ErrorCode::InvalidParameter
        ));
    }

    Ok((host, port))
}

/// Try to connect via SOCKS5 proxy or TCP with timeout.
async fn connect_with_optional_proxy(
    use_proxy: Option<u64>,
    address: SocketAddr,
) -> BichonResult<TcpStream> {
    if let Some(proxy_id) = use_proxy {
        let proxy = Proxy::get(proxy_id)?;
        let addr = parse_proxy_url(&proxy.url)?;
        return if addr.scheme == ProxyScheme::Http {
            connect_via_http_proxy(&addr, address).await
        } else {
            connect_via_socks5_proxy(&addr, address).await
        };
    }
    // Fallback to direct TCP connection
    timeout(TIMEOUT, TcpStream::connect(address))
        .await
        .map_err(|_| {
            error!(
                "TCP connection to {} timed out after {}s",
                address,
                TIMEOUT.as_secs()
            );
            raise_error!(
                format!(
                    "TCP connection to {} timed out after {}s",
                    address,
                    TIMEOUT.as_secs()
                ),
                ErrorCode::ConnectionTimeout
            )
        })?
        .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::NetworkError))
}

async fn connect_via_socks5_proxy(
    addr: &ProxyAddr,
    address: SocketAddr,
) -> BichonResult<TcpStream> {
    let proxy_addr = (addr.host.as_str(), addr.port);
    let result = if let (Some(user), Some(pass)) = (&addr.username, &addr.password) {
        timeout(
            TIMEOUT,
            Socks5Stream::connect_with_password(proxy_addr, address, user.as_str(), pass.as_str()),
        )
        .await
    } else {
        timeout(TIMEOUT, Socks5Stream::connect(proxy_addr, address)).await
    };

    result
        .map_err(|_| {
            error!(
                "SOCKS5 proxy connection to {} via {}:{} timed out after {}s",
                address,
                addr.host,
                addr.port,
                TIMEOUT.as_secs()
            );
            raise_error!(
                format!(
                    "SOCKS5 proxy connection to {} via {}:{} timed out after {}s",
                    address,
                    addr.host,
                    addr.port,
                    TIMEOUT.as_secs()
                ),
                ErrorCode::ConnectionTimeout
            )
        })?
        .map(|s| s.into_inner())
        .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::NetworkError))
}

async fn connect_via_http_proxy(addr: &ProxyAddr, address: SocketAddr) -> BichonResult<TcpStream> {
    let mut stream = timeout(TIMEOUT, TcpStream::connect((addr.host.as_str(), addr.port)))
        .await
        .map_err(|_| {
            raise_error!(
                format!(
                    "HTTP proxy connection to {}:{} timed out after {}s",
                    addr.host,
                    addr.port,
                    TIMEOUT.as_secs()
                ),
                ErrorCode::ConnectionTimeout
            )
        })?
        .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::NetworkError))?;

    let mut request = format!(
        "CONNECT {address} HTTP/1.1\r\nHost: {address}\r\nProxy-Connection: keep-alive\r\n"
    );
    if let (Some(user), Some(pass)) = (&addr.username, &addr.password) {
        let auth = general_purpose::STANDARD.encode(format!("{user}:{pass}"));
        request.push_str(&format!("Proxy-Authorization: Basic {auth}\r\n"));
    }
    request.push_str("\r\n");

    timeout(TIMEOUT, stream.write_all(request.as_bytes()))
        .await
        .map_err(|_| {
            raise_error!(
                format!(
                    "HTTP proxy CONNECT to {} via {}:{} timed out after {}s",
                    address,
                    addr.host,
                    addr.port,
                    TIMEOUT.as_secs()
                ),
                ErrorCode::ConnectionTimeout
            )
        })?
        .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::NetworkError))?;

    let mut response = Vec::new();
    timeout(TIMEOUT, async {
        let mut byte = [0u8; 1];
        while !response.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte).await?;
            response.push(byte[0]);
            if response.len() > 8192 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "HTTP proxy CONNECT response headers are too large",
                ));
            }
        }
        Ok::<(), std::io::Error>(())
    })
    .await
    .map_err(|_| {
        raise_error!(
            format!(
                "HTTP proxy CONNECT response from {}:{} timed out after {}s",
                addr.host,
                addr.port,
                TIMEOUT.as_secs()
            ),
            ErrorCode::ConnectionTimeout
        )
    })?
    .map_err(|e| {
        if e.kind() == std::io::ErrorKind::InvalidData {
            raise_error!(e.to_string(), ErrorCode::NetworkError)
        } else {
            raise_error!(format!("{:#?}", e), ErrorCode::NetworkError)
        }
    })?;

    let response = String::from_utf8_lossy(&response);
    if response.starts_with("HTTP/1.1 200") || response.starts_with("HTTP/1.0 200") {
        Ok(stream)
    } else {
        Err(raise_error!(
            format!(
                "HTTP proxy CONNECT to {} via {}:{} failed: {}",
                address,
                addr.host,
                addr.port,
                response.lines().next().unwrap_or("invalid response")
            ),
            ErrorCode::NetworkError
        ))
    }
}

// --- Trusted reverse proxies -------------------------------------------------

/// A single CIDR block used to describe a trusted reverse proxy.
///
/// Parsing accepts a bare address (`127.0.0.1`, `::1`) as a host route, or an
/// address plus prefix (`10.0.0.0/8`, `2001:db8::/32`). IPv4-mapped IPv6
/// addresses are normalised to IPv4 before matching, so `::ffff:127.0.0.1`
/// and `127.0.0.1` are the same peer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cidr {
    network: IpAddr,
    prefix: u8,
}

impl Cidr {
    pub fn parse(value: &str) -> Result<Self, String> {
        let value = value.trim();
        if value.is_empty() {
            return Err("empty network entry".to_string());
        }
        let (addr_part, prefix_part) = match value.split_once('/') {
            Some((addr, prefix)) => (addr.trim(), Some(prefix.trim())),
            None => (value, None),
        };
        let addr: IpAddr = addr_part
            .parse()
            .map_err(|_| format!("invalid IP address '{addr_part}'"))?;
        let addr = normalize_ip(addr);
        let max_prefix = if addr.is_ipv4() { 32 } else { 128 };
        let prefix = match prefix_part {
            Some(prefix) => prefix
                .parse::<u8>()
                .map_err(|_| format!("invalid prefix length '{prefix}'"))?,
            None => max_prefix,
        };
        if prefix > max_prefix {
            return Err(format!(
                "prefix length {prefix} is too large for address {addr}"
            ));
        }
        Ok(Self {
            network: mask_ip(addr, prefix),
            prefix,
        })
    }

    pub fn contains(&self, ip: IpAddr) -> bool {
        let ip = normalize_ip(ip);
        match (self.network, ip) {
            (IpAddr::V4(network), IpAddr::V4(ip)) => {
                let mask = ipv4_mask(self.prefix);
                (u32::from(network) & mask) == (u32::from(ip) & mask)
            }
            (IpAddr::V6(network), IpAddr::V6(ip)) => {
                let mask = ipv6_mask(self.prefix);
                (u128::from(network) & mask) == (u128::from(ip) & mask)
            }
            _ => false,
        }
    }
}

/// The set of peers whose forwarding headers may be believed.
///
/// `trust_all` exists only for backwards compatibility with deployments that
/// cannot enumerate their proxies; `*` is deliberately explicit because it
/// restores the old (spoofable) behaviour.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrustedProxies {
    cidrs: Vec<Cidr>,
    trust_all: bool,
}

impl TrustedProxies {
    pub fn parse<I, S>(entries: I) -> Result<Self, String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut cidrs = Vec::new();
        let mut trust_all = false;
        for entry in entries {
            let entry = entry.as_ref().trim();
            if entry.is_empty() {
                continue;
            }
            if entry == "*" {
                trust_all = true;
            } else {
                cidrs.push(Cidr::parse(entry)?);
            }
        }
        Ok(Self { cidrs, trust_all })
    }

    pub fn with_trust_all(trust_all: bool) -> Self {
        Self {
            cidrs: Vec::new(),
            trust_all,
        }
    }

    pub fn is_trust_all(&self) -> bool {
        self.trust_all
    }

    pub fn is_empty(&self) -> bool {
        self.cidrs.is_empty() && !self.trust_all
    }

    pub fn is_trusted(&self, ip: IpAddr) -> bool {
        self.trust_all || self.cidrs.iter().any(|cidr| cidr.contains(ip))
    }
}

/// Fold IPv4-mapped IPv6 addresses (`::ffff:a.b.c.d`) into IPv4 so the same
/// client cannot occupy two rate-limit buckets.
pub fn normalize_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => IpAddr::V6(v6),
        },
        v4 => v4,
    }
}

fn ipv4_mask(prefix: u8) -> u32 {
    if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix)
    }
}

fn ipv6_mask(prefix: u8) -> u128 {
    if prefix == 0 {
        0
    } else {
        u128::MAX << (128 - prefix)
    }
}

fn mask_ip(ip: IpAddr, prefix: u8) -> IpAddr {
    match ip {
        IpAddr::V4(v4) => IpAddr::V4(Ipv4Addr::from(u32::from(v4) & ipv4_mask(prefix))),
        IpAddr::V6(v6) => IpAddr::V6(Ipv6Addr::from(u128::from(v6) & ipv6_mask(prefix))),
    }
}

#[cfg(test)]
mod trusted_proxy_tests {
    use super::*;

    fn ip(value: &str) -> IpAddr {
        value.parse().unwrap()
    }

    #[test]
    fn bare_addresses_are_full_length_routes() {
        let v4 = Cidr::parse("127.0.0.1").unwrap();
        assert!(v4.contains(ip("127.0.0.1")));
        assert!(!v4.contains(ip("127.0.0.2")));

        let v6 = Cidr::parse("::1").unwrap();
        assert!(v6.contains(ip("::1")));
        assert!(!v6.contains(ip("::2")));
    }

    #[test]
    fn cidr_blocks_match_only_their_network() {
        let private = Cidr::parse("10.0.0.0/8").unwrap();
        assert!(private.contains(ip("10.1.2.3")));
        assert!(!private.contains(ip("11.1.2.3")));

        let docker = Cidr::parse("172.17.0.0/16").unwrap();
        assert!(docker.contains(ip("172.17.255.254")));
        assert!(!docker.contains(ip("172.18.0.1")));

        let v6 = Cidr::parse("2001:db8::/32").unwrap();
        assert!(v6.contains(ip("2001:db8:1::1")));
        assert!(!v6.contains(ip("2001:db9::1")));
    }

    #[test]
    fn host_bits_in_the_network_are_masked() {
        let cidr = Cidr::parse("10.1.2.3/8").unwrap();
        assert!(cidr.contains(ip("10.200.0.1")));
        assert!(!cidr.contains(ip("11.0.0.1")));
    }

    #[test]
    fn zero_prefix_matches_every_address_of_that_family() {
        let v4 = Cidr::parse("0.0.0.0/0").unwrap();
        assert!(v4.contains(ip("1.2.3.4")));
        assert!(!v4.contains(ip("::1")));
    }

    #[test]
    fn ipv4_mapped_ipv6_is_normalised() {
        let cidr = Cidr::parse("127.0.0.1").unwrap();
        assert!(cidr.contains(ip("::ffff:127.0.0.1")));
        assert_eq!(normalize_ip(ip("::ffff:10.0.0.1")), ip("10.0.0.1"));
    }

    #[test]
    fn invalid_entries_are_rejected() {
        assert!(Cidr::parse("not-an-ip").is_err());
        assert!(Cidr::parse("10.0.0.0/33").is_err());
        assert!(Cidr::parse("2001:db8::/129").is_err());
        assert!(Cidr::parse("10.0.0.0/not-a-number").is_err());
        assert!(TrustedProxies::parse(["10.0.0.0/8", "bogus"]).is_err());
    }

    #[test]
    fn multiple_entries_and_trust_all() {
        let trusted = TrustedProxies::parse(["127.0.0.1", "10.0.0.0/8"]).unwrap();
        assert!(!trusted.is_empty());
        assert!(trusted.is_trusted(ip("127.0.0.1")));
        assert!(trusted.is_trusted(ip("10.9.8.7")));
        assert!(!trusted.is_trusted(ip("192.0.2.1")));

        let all = TrustedProxies::parse(["*"]).unwrap();
        assert!(all.is_trust_all());
        assert!(all.is_trusted(ip("203.0.113.9")));

        assert!(TrustedProxies::default().is_empty());
    }
}
