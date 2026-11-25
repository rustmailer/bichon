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

use std::net::SocketAddr;
use std::time::Duration;

use hickory_resolver::net::runtime::TokioRuntimeProvider;
use hickory_resolver::proto::rr::RData;
use hickory_resolver::TokioResolver;
use quick_xml::de::from_str;
use reqwest::header::LOCATION;
use reqwest::{Client, Response, Url};
use serde::Deserialize;

use crate::error::code::ErrorCode;
use crate::error::BichonResult;
use crate::raise_error;
use crate::utils::ssrf;

/// Discovery is a short, human-triggered operation; three redirects are far
/// more than any real provider uses.
const MAX_REDIRECTS: usize = 3;
/// Per-URL budget covering DNS-pinned address attempts. Autoconfig providers
/// answer quickly; a black-holed address should not consume the whole walk.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
/// Whole discovery walk budget; keeps a slow provider from stalling the API.
const DISCOVERY_BUDGET: Duration = Duration::from_secs(30);

/// Parsed result from Thunderbird-style autoconfig XML or DNS SRV fallback.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MailConfig {
    pub incoming: Vec<IncomingServer>,
    pub outgoing: Vec<OutgoingServer>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct IncomingServer {
    #[serde(rename = "@type")]
    pub protocol: String,
    pub hostname: String,
    #[serde(default)]
    pub port: u16,
    #[serde(rename = "socketType")]
    pub socket_type: String,
    pub username: String,
    /// Authentication method from the XML, e.g. "OAuth2", "password-cleartext",
    /// "password-encrypted", "GSSAPI", "NTLM".  Absent in DNS SRV fallback.
    #[serde(default)]
    pub authentication: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct OutgoingServer {
    #[serde(rename = "@type")]
    pub protocol: String,
    pub hostname: String,
    #[serde(default)]
    pub port: u16,
    #[serde(rename = "socketType")]
    pub socket_type: String,
    pub username: String,
}

// ---------------------------------------------------------------------------
// Internal XML wrapper structs matching the Thunderbird config-v1.1 schema:
// <clientConfig> → <emailProvider> → <incomingServer> / <outgoingServer>
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename = "clientConfig")]
struct ClientConfig {
    #[serde(rename = "emailProvider", default)]
    email_providers: Vec<EmailProvider>,
}

#[derive(Debug, Deserialize)]
struct EmailProvider {
    #[serde(rename = "incomingServer", default)]
    incoming_servers: Vec<IncomingServer>,
    #[serde(rename = "outgoingServer", default)]
    outgoing_servers: Vec<OutgoingServer>,
}

/// Parse Thunderbird autoconfig XML into a `MailConfig`.
/// Exposed for unit testing.
pub(crate) fn parse_autoconfig_xml(xml: &str) -> Option<MailConfig> {
    let client_config: ClientConfig = from_str(xml).ok()?;
    let provider = client_config.email_providers.into_iter().next()?;
    Some(MailConfig {
        incoming: provider.incoming_servers,
        outgoing: provider.outgoing_servers,
    })
}

// ---------------------------------------------------------------------------
// Network helpers
// ---------------------------------------------------------------------------

/// Fetch one autoconfig URL, following at most [`MAX_REDIRECTS`] hops.
///
/// Every hop goes through [`send_pinned`], which validates the scheme and
/// hostname, resolves the name and refuses private/loopback/link-local/
/// metadata addresses. Redirects are handled manually so a public URL cannot
/// `302` the server into the internal network.
async fn fetch_xml(url: &str) -> Option<MailConfig> {
    let mut current = Url::parse(url).ok()?;
    for _ in 0..=MAX_REDIRECTS {
        tracing::debug!("autoconfig: GET {current}");
        let resp = send_pinned(&current).await?;
        let status = resp.status();
        if status.is_redirection() {
            let location = resp.headers().get(LOCATION)?.to_str().ok()?;
            current = current.join(location).ok()?;
            tracing::debug!("autoconfig: redirect -> {current}");
            continue;
        }
        if !status.is_success() {
            return None;
        }
        let text = resp.text().await.ok()?;
        return parse_autoconfig_xml(&text);
    }
    None
}

/// Send one request to a validated, public destination.
///
/// The name is resolved here, checked, and then pinned with
/// `ClientBuilder::resolve`, so the connection cannot be re-resolved to a
/// private address between the check and the connect (DNS rebinding). Proxies
/// are disabled for the same reason: an environment proxy would do its own
/// resolution and bypass the pin.
async fn send_pinned(url: &Url) -> Option<Response> {
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    if !url.username().is_empty() || url.password().is_some() {
        return None;
    }
    let host = url.host_str()?.to_string();
    let port = url.port_or_known_default()?;
    let addrs = ssrf::resolve_public_addrs(&host, port).await?;
    let url = url.clone();

    // One budget for all address attempts: a black-holed CDN address must not
    // multiply the per-URL timeout by the size of the A record set.
    tokio::time::timeout(REQUEST_TIMEOUT, async move {
        for addr in addrs {
            let client = match Client::builder()
                .timeout(REQUEST_TIMEOUT)
                .redirect(reqwest::redirect::Policy::none())
                .no_proxy()
                .resolve(&host, SocketAddr::new(addr, port))
                .build()
            {
                Ok(client) => client,
                Err(_) => continue,
            };
            if let Ok(resp) = client.get(url.clone()).send().await {
                return Some(resp);
            }
        }
        None
    })
    .await
    .ok()
    .flatten()
}

async fn lookup_srv(domain: &str) -> Option<MailConfig> {
    let resolver = TokioResolver::builder(TokioRuntimeProvider::default())
        .ok()?
        .build()
        .ok()?;

    let imap_srv = format!("_imaps._tcp.{}.", domain);
    let imap_lookup = resolver.srv_lookup(imap_srv).await.ok()?;
    let imap_record = imap_lookup.answers().first()?;
    let (imap_host, imap_port) = match &imap_record.data {
        RData::SRV(srv) => {
            let host = srv.target.to_string().trim_end_matches('.').to_string();
            (host, srv.port)
        }
        _ => return None,
    };

    let smtp_srv = format!("_submission._tcp.{}.", domain);
    let smtp_lookup = resolver.srv_lookup(smtp_srv).await.ok()?;
    let smtp_record = smtp_lookup.answers().first()?;
    let (smtp_host, smtp_port) = match &smtp_record.data {
        RData::SRV(srv) => {
            let host = srv.target.to_string().trim_end_matches('.').to_string();
            (host, srv.port)
        }
        _ => return None,
    };

    // The discovered hosts are later connected to by the archiver, so they
    // must not point at loopback/private/link-local addresses either.
    if !ssrf::host_resolves_public(&imap_host, imap_port).await
        || !ssrf::host_resolves_public(&smtp_host, smtp_port).await
    {
        return None;
    }

    Some(MailConfig {
        incoming: vec![IncomingServer {
            protocol: "imap".to_string(),
            hostname: imap_host,
            port: imap_port,
            socket_type: "SSL".to_string(),
            username: "%EMAILADDRESS%".to_string(),
            authentication: String::new(),
        }],
        outgoing: vec![OutgoingServer {
            protocol: "smtp".to_string(),
            hostname: smtp_host,
            port: smtp_port,
            socket_type: "STARTTLS".to_string(),
            username: "%EMAILADDRESS%".to_string(),
        }],
    })
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Discover mail server configuration for a domain using the Thunderbird
/// autoconfig protocol (ISPDB), DNS SRV, MX fallback, and finally guessing.
///
/// Probe order:
/// 1. `https://autoconfig.{domain}/mail/config-v1.1.xml`
/// 2. `http://autoconfig.{domain}/mail/config-v1.1.xml`
/// 3. `https://{domain}/.well-known/autoconfig/mail/config-v1.1.xml`
/// 4. `http://{domain}/.well-known/autoconfig/mail/config-v1.1.xml`
/// 5. DNS SRV records (`_imaps._tcp` / `_submission._tcp`)
/// 6. Thunderbird central ISPDB (`https://autoconfig.thunderbird.net/v1.1/{domain}`)
/// 7. MX lookup → ISPDB for MX domain
/// 8. MX lookup → ISP autoconfig for MX domain
/// 9. GuessConfig — probe common hostnames + ports
pub async fn fetch(domain: &str) -> BichonResult<MailConfig> {
    let domain = domain.trim().trim_end_matches('.').to_ascii_lowercase();
    // Refuse obviously local or malformed names before doing any network work.
    if !ssrf::hostname_is_acceptable(&domain) {
        return Err(raise_error!(
            format!("No autoconfig found for domain: {domain}"),
            ErrorCode::ResourceNotFound
        ));
    }
    // A whole discovery walk is six HTTP candidates, SRV, MX and up to six TCP
    // probes. Bound it so a slow or black-holed host cannot hold the request
    // open indefinitely.
    match tokio::time::timeout(DISCOVERY_BUDGET, discover(&domain)).await {
        Ok(result) => result,
        Err(_) => Err(raise_error!(
            format!("Autoconfig discovery timed out for domain: {domain}"),
            ErrorCode::InternalError
        )),
    }
}

async fn discover(domain: &str) -> BichonResult<MailConfig> {
    // ── ISP autoconfig (HTTPS, then HTTP) ──────────────────────────
    if let Some(config) =
        fetch_xml(&format!("https://autoconfig.{domain}/mail/config-v1.1.xml")).await
    {
        return Ok(config);
    }
    if let Some(config) =
        fetch_xml(&format!("http://autoconfig.{domain}/mail/config-v1.1.xml")).await
    {
        return Ok(config);
    }

    // ── Well-known path (HTTPS, then HTTP) ─────────────────────────
    if let Some(config) = fetch_xml(&format!(
        "https://{domain}/.well-known/autoconfig/mail/config-v1.1.xml"
    ))
    .await
    {
        return Ok(config);
    }
    if let Some(config) = fetch_xml(&format!(
        "http://{domain}/.well-known/autoconfig/mail/config-v1.1.xml"
    ))
    .await
    {
        return Ok(config);
    }

    // ── DNS SRV records ────────────────────────────────────────────
    if let Some(config) = lookup_srv(&domain).await {
        return Ok(config);
    }

    // ── Thunderbird central ISPDB ──────────────────────────────────
    if let Some(config) =
        fetch_xml(&format!("https://autoconfig.thunderbird.net/v1.1/{domain}")).await
    {
        return Ok(config);
    }

    // ── MX fallback ────────────────────────────────────────────────
    if let Some(config) = fetch_for_mx(&domain).await {
        return Ok(config);
    }

    // ── GuessConfig ────────────────────────────────────────────────
    if let Some(config) = crate::autoconfig::guess::guess_config(&domain).await {
        return Ok(config);
    }

    Err(raise_error!(
        format!("No autoconfig found for domain: {domain}"),
        ErrorCode::InternalError
    ))
}

/// DNS MX lookup → retry ISPDB and ISP autoconfig for the MX domain.
///
/// Many self-hosted domains have their MX pointed at Google, Microsoft, etc.
/// The MX domain's ISPDB entry covers the original domain.
async fn fetch_for_mx(domain: &str) -> Option<MailConfig> {
    let mx_domain = lookup_mx_domain(domain).await?;
    if mx_domain == domain.to_ascii_lowercase() {
        return None; // same domain, already tried above
    }

    // Try ISPDB for the MX domain
    if let Some(config) =
        fetch_xml(&format!("https://autoconfig.thunderbird.net/v1.1/{mx_domain}")).await
    {
        return Some(config);
    }

    // Try ISP autoconfig for the MX domain (HTTPS then HTTP)
    if let Some(config) =
        fetch_xml(&format!("https://autoconfig.{mx_domain}/mail/config-v1.1.xml")).await
    {
        return Some(config);
    }
    if let Some(config) =
        fetch_xml(&format!("http://autoconfig.{mx_domain}/mail/config-v1.1.xml")).await
    {
        return Some(config);
    }

    None
}

/// DNS MX lookup → extract the second-level domain of the first MX hostname.
async fn lookup_mx_domain(domain: &str) -> Option<String> {
    let resolver = TokioResolver::builder(TokioRuntimeProvider::default())
        .ok()?
        .build()
        .ok()?;
    let lookup = resolver.mx_lookup(domain).await.ok()?;
    let record = lookup.answers().first()?;
    let mx_host = match &record.data {
        RData::MX(mx) => mx.exchange.to_string().trim_end_matches('.').to_string(),
        _ => return None,
    };

    // Extract a reasonable base domain from the MX hostname.
    // E.g., "aspmx.l.google.com" → "google.com"
    //       "company.mail.protection.outlook.com" → "outlook.com"
    extract_base_domain(&mx_host)
}

/// Extract the top two labels from a hostname as a rough base domain.
fn extract_base_domain(host: &str) -> Option<String> {
    let parts: Vec<&str> = host.split('.').collect();
    if parts.len() >= 2 {
        Some(parts[parts.len() - 2..].join("."))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// These are refused before any DNS lookup or socket is opened.
    #[tokio::test]
    async fn fetch_xml_refuses_non_public_targets() {
        for url in [
            "http://127.0.0.1/mail/config-v1.1.xml",
            "http://localhost/mail/config-v1.1.xml",
            "http://169.254.169.254/latest/meta-data/",
            "http://[::1]/mail/config-v1.1.xml",
            "ftp://example.com/mail/config-v1.1.xml",
            "http://user:pass@example.com/x",
        ] {
            assert!(fetch_xml(url).await.is_none(), "{url} must be refused");
        }
    }

    #[tokio::test]
    async fn fetch_rejects_local_domains_without_network() {
        for domain in ["localhost", "127.0.0.1", "intranet", "mail.local"] {
            assert!(fetch(domain).await.is_err(), "{domain} must be refused");
        }
    }

    #[test]
    fn test_parse_autoconfig_xml() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<clientConfig version="1.1">
    <emailProvider id="example.com">
        <domain>example.com</domain>
        <incomingServer type="imap">
            <hostname>imap.example.com</hostname>
            <port>993</port>
            <socketType>SSL</socketType>
            <username>%EMAILADDRESS%</username>
            <authentication>password-cleartext</authentication>
        </incomingServer>
        <outgoingServer type="smtp">
            <hostname>smtp.example.com</hostname>
            <port>587</port>
            <socketType>STARTTLS</socketType>
            <username>%EMAILADDRESS%</username>
        </outgoingServer>
    </emailProvider>
</clientConfig>"#;

        let config = parse_autoconfig_xml(xml).expect("should parse valid XML");
        assert_eq!(config.incoming.len(), 1);
        assert_eq!(config.incoming[0].protocol, "imap");
        assert_eq!(config.incoming[0].hostname, "imap.example.com");
        assert_eq!(config.incoming[0].port, 993);
        assert_eq!(config.incoming[0].socket_type, "SSL");
        assert_eq!(config.incoming[0].username, "%EMAILADDRESS%");
        assert_eq!(config.incoming[0].authentication, "password-cleartext");

        assert_eq!(config.outgoing.len(), 1);
        assert_eq!(config.outgoing[0].protocol, "smtp");
        assert_eq!(config.outgoing[0].hostname, "smtp.example.com");
        assert_eq!(config.outgoing[0].port, 587);
        assert_eq!(config.outgoing[0].socket_type, "STARTTLS");
        assert_eq!(config.outgoing[0].username, "%EMAILADDRESS%");
    }

    #[test]
    fn test_parse_autoconfig_xml_invalid() {
        assert!(parse_autoconfig_xml("not xml").is_none());
        assert!(parse_autoconfig_xml("<clientConfig></clientConfig>").is_none());
    }

    #[test]
    fn test_extract_base_domain() {
        assert_eq!(
            extract_base_domain("aspmx.l.google.com"),
            Some("google.com".to_string())
        );
        assert_eq!(
            extract_base_domain("company.mail.protection.outlook.com"),
            Some("outlook.com".to_string())
        );
        assert_eq!(
            extract_base_domain("mx.example.com"),
            Some("example.com".to_string())
        );
        assert_eq!(
            extract_base_domain("example.com"),
            Some("example.com".to_string())
        );
        assert_eq!(extract_base_domain("localhost"), None);
    }

    #[tokio::test]
    async fn test_lookup_srv_gmail() {
        // Gmail should have SRV records for IMAPS and SMTP submission
        let config = lookup_srv("gmail.com").await;
        assert!(config.is_some(), "Gmail should have SRV records");
        let config = config.unwrap();
        assert_eq!(config.incoming.len(), 1);
        assert_eq!(config.incoming[0].protocol, "imap");
        assert_eq!(config.incoming[0].socket_type, "SSL");
        assert!(!config.incoming[0].hostname.is_empty());
        assert!(config.incoming[0].port > 0);
        assert_eq!(config.outgoing.len(), 1);
        assert_eq!(config.outgoing[0].protocol, "smtp");
        assert_eq!(config.outgoing[0].socket_type, "STARTTLS");
        assert!(!config.outgoing[0].hostname.is_empty());
        assert!(config.outgoing[0].port > 0);
    }

    #[tokio::test]
    async fn test_lookup_srv_nonexistent() {
        // A domain without SRV records should return None
        let config = lookup_srv("this-domain-definitely-does-not-exist-12345.com").await;
        assert!(config.is_none());
    }
}
