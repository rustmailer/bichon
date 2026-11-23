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

//use poem_openapi::Object;
use serde::{Deserialize, Serialize};
use std::{error::Error, time::Duration};

use crate::{
    database::{
        delete_impl, find_impl, insert_impl, list_all_impl, manager::DB_MANAGER, update_impl,
        MemDbModel,
    },
    error::{code::ErrorCode, BichonResult},
    id, raise_error, utc_now,
    utils::net::parse_proxy_url,
};

const PROXY_TEST_TIMEOUT: Duration = Duration::from_secs(8);
const GEO_PROVIDERS: &[GeoProvider] = &[
    GeoProvider {
        name: "ip-api.com",
        url: "http://ip-api.com/json/?fields=status,message,query,country,countryCode,regionName,city,isp,timezone,lat,lon",
    },
    GeoProvider {
        name: "ipwho.is",
        url: "https://ipwho.is/",
    },
    GeoProvider {
        name: "ipapi.co",
        url: "https://ipapi.co/json/",
    },
    GeoProvider {
        name: "internetdata.io",
        url: "https://api.internetdata.io/myip",
    },
];

struct GeoProvider {
    name: &'static str,
    url: &'static str,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Object))]
pub struct Proxy {
    /// The unique identifier for this proxy configuration.
    pub id: u64,

    /// The proxy URL (e.g., socks5://127.0.0.1:1080) used to route network requests.
    pub url: String,

    /// The creation timestamp of this record, represented as milliseconds since the Unix epoch.
    pub created_at: i64,

    /// The last update timestamp of this record, represented as milliseconds since the Unix epoch.
    pub updated_at: i64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "web-api", derive(poem_openapi::Object))]
pub struct ProxyTestResult {
    pub ip: Option<String>,
    pub country: Option<String>,
    pub region: Option<String>,
    pub city: Option<String>,
    pub isp: Option<String>,
}

impl MemDbModel for Proxy {
    fn collection() -> &'static str {
        "proxies"
    }
    fn key(&self) -> String {
        self.id.to_string()
    }
}

impl Proxy {
    /// Create a new Proxy instance with the given URL and timestamps.
    pub fn new(url: String) -> Self {
        Self {
            id: id!(64),
            url,
            created_at: utc_now!(),
            updated_at: utc_now!(),
        }
    }

    pub fn get(id: u64) -> BichonResult<Proxy> {
        let key = id.to_string();
        find_impl::<Proxy>(DB_MANAGER.db(), &key)?.ok_or_else(|| {
            raise_error!(
                format!("Proxy with id={} not found", id),
                ErrorCode::ResourceNotFound
            )
        })
    }

    pub fn list_all() -> BichonResult<Vec<Proxy>> {
        list_all_impl::<Proxy>(DB_MANAGER.db())
    }

    pub fn delete(id: u64) -> BichonResult<()> {
        delete_impl::<Proxy>(DB_MANAGER.db(), &id.to_string())
    }

    pub fn update(id: u64, url: String) -> BichonResult<()> {
        update_impl(DB_MANAGER.db(), &id.to_string(), move |current: Proxy| {
            let mut updated = current;
            updated.url = url;
            updated.updated_at = utc_now!();
            updated.validate()?;
            Ok(updated)
        })?;
        Ok(())
    }

    pub fn save(&self) -> BichonResult<()> {
        self.validate()?;
        insert_impl(DB_MANAGER.db(), self.to_owned())
    }

    /// Validate that the URL is a valid proxy URL.
    pub fn validate(&self) -> BichonResult<()> {
        parse_proxy_url(&self.url)?;
        Ok(())
    }

    pub async fn test_connectivity(&self) -> BichonResult<ProxyTestResult> {
        test_proxy_url(&self.url).await
    }

    pub async fn test(id: u64) -> BichonResult<ProxyTestResult> {
        let proxy = Self::get(id)?;
        proxy.test_connectivity().await
    }
}

async fn test_proxy_url(url: &str) -> BichonResult<ProxyTestResult> {
    let proxy_url = parse_proxy_url(url)?.standard_url();
    let client = reqwest::Client::builder()
        .timeout(PROXY_TEST_TIMEOUT)
        .proxy(reqwest::Proxy::all(&proxy_url).map_err(|_| {
            raise_error!(
                "Failed to configure proxy. Please check the proxy configuration.".into(),
                ErrorCode::InvalidParameter
            )
        })?)
        .build()
        .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

    let mut last_error = None;
    for provider in GEO_PROVIDERS {
        match test_geo_provider(&client, provider).await {
            Ok(result) => return Ok(result),
            Err(err) => last_error = Some(err.to_string()),
        }
    }

    Err(raise_error!(
        format!(
            "Proxy check failed with all geo providers: {}",
            last_error.unwrap_or_else(|| "unknown error".into())
        ),
        ErrorCode::NetworkError
    ))
}

async fn test_geo_provider(
    client: &reqwest::Client,
    provider: &GeoProvider,
) -> BichonResult<ProxyTestResult> {
    let value = client
        .get(provider.url)
        .send()
        .await
        .map_err(|e| {
            raise_error!(
                proxy_request_error_message(
                    &format!("Proxy check request failed via {}", provider.name),
                    &e
                ),
                ErrorCode::NetworkError
            )
        })?
        .error_for_status()
        .map_err(|e| {
            raise_error!(
                proxy_request_error_message(
                    &format!("Proxy check request failed via {}", provider.name),
                    &e
                ),
                ErrorCode::NetworkError
            )
        })?
        .json::<serde_json::Value>()
        .await
        .map_err(|e| {
            raise_error!(
                proxy_request_error_message(
                    &format!("Failed to read proxy check response from {}", provider.name),
                    &e
                ),
                ErrorCode::NetworkError
            )
        })?;

    proxy_test_result_from_value(provider.name, &value)
}

/// Top-level keys of a JSON object, for warning messages when a provider
/// changes its payload. Values are deliberately left out: the response can
/// carry the caller's address.
fn top_level_keys(value: &serde_json::Value) -> Vec<&str> {
    value
        .as_object()
        .map(|obj| obj.keys().map(String::as_str).collect())
        .unwrap_or_default()
}

fn proxy_test_result_from_value(
    provider: &str,
    value: &serde_json::Value,
) -> BichonResult<ProxyTestResult> {
    if provider == "ip-api.com" && value["status"].as_str() == Some("fail") {
        return Err(raise_error!(
            format!(
                "ip-api.com proxy check failed: {}",
                value["message"].as_str().unwrap_or("unknown error")
            ),
            ErrorCode::NetworkError
        ));
    }
    if provider == "ipwho.is" && value["success"].as_bool() == Some(false) {
        return Err(raise_error!(
            format!(
                "ipwho.is proxy check failed: {}",
                value["message"].as_str().unwrap_or("unknown error")
            ),
            ErrorCode::NetworkError
        ));
    }
    if provider == "ipapi.co" && value["error"].as_bool() == Some(true) {
        return Err(raise_error!(
            format!(
                "ipapi.co proxy check failed: {}",
                value["reason"].as_str().unwrap_or("unknown error")
            ),
            ErrorCode::NetworkError
        ));
    }

    let ip_key = if provider == "ip-api.com" {
        "query"
    } else {
        "ip"
    };
    let ip = value[ip_key].as_str().ok_or_else(|| {
        tracing::warn!(
            "{provider} proxy check: response has no `{ip_key}` field (top-level keys: {:?})",
            top_level_keys(value)
        );
        raise_error!(
            format!("{provider} did not return an IP address"),
            ErrorCode::NetworkError
        )
    })?;
    if provider == "internetdata.io" {
        // Location and network come nested; city and region only on a paid key.
        // The fallbacks below are defensive: if the payload shape changes, keep
        // the fields we can still read and warn instead of dropping them.
        let location = &value["location"];
        if !location.is_object() {
            tracing::warn!(
                "internetdata.io proxy check: response has no `location` object (top-level keys: {:?})",
                top_level_keys(value)
            );
        }

        let country = location["country_name"].as_str().or_else(|| {
            tracing::warn!(
                "internetdata.io proxy check: missing location.country_name, falling back to location.country"
            );
            location["country"].as_str()
        });

        let isp = value["company"]["name"].as_str().or_else(|| {
            tracing::warn!(
                "internetdata.io proxy check: missing company.name, falling back to asn.name"
            );
            value["asn"]["name"].as_str()
        });
        if isp.is_none() {
            tracing::warn!(
                "internetdata.io proxy check: response has neither company.name nor asn.name"
            );
        }

        return Ok(ProxyTestResult {
            ip: Some(ip.to_string()),
            country: country.map(str::to_string),
            region: location["region"].as_str().map(str::to_string),
            city: location["city"].as_str().map(str::to_string),
            isp: isp.map(str::to_string),
        });
    }

    let connection = &value["connection"];

    Ok(ProxyTestResult {
        ip: Some(ip.to_string()),
        country: value[if provider == "ipapi.co" {
            "country_name"
        } else {
            "country"
        }]
        .as_str()
        .map(str::to_string),
        region: value[if provider == "ip-api.com" {
            "regionName"
        } else {
            "region"
        }]
        .as_str()
        .map(str::to_string),
        city: value["city"].as_str().map(str::to_string),
        isp: if provider == "ipapi.co" {
            value["org"].as_str().map(str::to_string)
        } else if provider == "ip-api.com" {
            value["isp"].as_str().map(str::to_string)
        } else {
            connection["isp"].as_str().map(str::to_string)
        },
    })
}

fn proxy_request_error_message(context: &str, err: &reqwest::Error) -> String {
    let kind = if err.is_timeout() {
        "timed out"
    } else if err.is_connect() {
        "could not connect through the proxy"
    } else if err.is_status() {
        "received an error response"
    } else {
        "request failed"
    };
    let mut message = format!("{context}: {kind}: {err}");
    let mut source = err.source();

    while let Some(err) = source {
        message.push_str(&format!(": {err}"));
        source = err.source();
    }

    message
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_proxy_urls() {
        let urls = vec![
            "socks5://127.0.0.1:1080",
            "http://127.0.0.1:8080",
            "socks5://proxy.example.com:1080",
            "socks5://user:pass@proxy.example.com:1080",
            "http://user:pass@proxy.example.com:8080",
            "socks5://[::1]:1080",
            "socks5://user:pass@[::1]:1080",
            // Non-standard format: host:port:user:pass
            "socks5://server.nodeprovider.com:8080:username123:passwordhere",
            "http://server.nodeprovider.com:8080:username123:passwordhere",
        ];

        for url in urls {
            let proxy = Proxy::new(url.to_string());
            assert!(proxy.validate().is_ok(), "URL should be valid: {}", url);
        }
    }

    #[test]
    fn test_invalid_proxy_urls() {
        for url in ["socks5://user@proxy.example.com:1080", "socks5://::1:1080"] {
            let proxy = Proxy::new(url.to_string());
            assert!(proxy.validate().is_err(), "URL should be invalid: {}", url);
        }
    }

    #[test]
    fn test_ipv6_proxy_urls_render_with_brackets() {
        let addr = parse_proxy_url("socks5://[::1]:1080").unwrap();
        assert_eq!(addr.standard_url(), "socks5://[::1]:1080");

        let addr = parse_proxy_url("socks5://user:pass@[::1]:1080").unwrap();
        assert_eq!(addr.standard_url(), "socks5://user:pass@[::1]:1080");
    }

    #[test]
    fn proxy_test_result_rejects_empty_provider_response() {
        let result = proxy_test_result_from_value("ipwho.is", &serde_json::json!({}));
        assert!(result.is_err());
    }

    #[test]
    fn proxy_test_result_reads_internetdata_nested_fields() {
        let result = proxy_test_result_from_value(
            "internetdata.io",
            &serde_json::json!({
                "ip": "8.8.8.8",
                "location": { "country": "US", "country_name": "United States" },
                "asn": { "asn": 15169, "name": "Google LLC" },
                "company": { "name": "Google LLC", "domain": "google.com" }
            }),
        )
        .unwrap();
        assert_eq!(result.ip.as_deref(), Some("8.8.8.8"));
        assert_eq!(result.country.as_deref(), Some("United States"));
        assert_eq!(result.region, None);
        assert_eq!(result.isp.as_deref(), Some("Google LLC"));
    }

    #[test]
    fn proxy_test_result_internetdata_free_tier_shape() {
        // The keyless /myip answer: country-level location only, with the
        // network and company blocks alongside it. Extra fields must not
        // disturb the parse.
        let result = proxy_test_result_from_value(
            "internetdata.io",
            &serde_json::json!({
                "ip": "8.8.8.8",
                "is_bogon": false,
                "is_tor": false,
                "location": {
                    "continent": "NA",
                    "continent_name": "North America",
                    "country": "US",
                    "country_name": "United States",
                    "timezone": "America/Chicago"
                },
                "asn": {
                    "asn": 15169,
                    "name": "Google LLC",
                    "domain": "google.com",
                    "type": "business"
                },
                "company": { "name": "Google LLC", "domain": "google.com" },
                "abuse": { "email": "abuse@example.com" }
            }),
        )
        .unwrap();
        assert_eq!(result.ip.as_deref(), Some("8.8.8.8"));
        assert_eq!(result.country.as_deref(), Some("United States"));
        assert_eq!(result.region, None);
        assert_eq!(result.city, None);
        assert_eq!(result.isp.as_deref(), Some("Google LLC"));
    }

    #[test]
    fn proxy_test_result_internetdata_falls_back_to_country_code() {
        let result = proxy_test_result_from_value(
            "internetdata.io",
            &serde_json::json!({
                "ip": "8.8.8.8",
                "location": { "country": "US" },
                "company": { "name": "Google LLC" }
            }),
        )
        .unwrap();
        assert_eq!(result.country.as_deref(), Some("US"));
    }

    #[test]
    fn proxy_test_result_internetdata_falls_back_to_asn_name() {
        let result = proxy_test_result_from_value(
            "internetdata.io",
            &serde_json::json!({
                "ip": "8.8.8.8",
                "location": { "country_name": "United States" },
                "asn": { "asn": 15169, "name": "Google LLC" }
            }),
        )
        .unwrap();
        assert_eq!(result.isp.as_deref(), Some("Google LLC"));
    }

    #[test]
    fn proxy_test_result_internetdata_reads_paid_location_fields() {
        let result = proxy_test_result_from_value(
            "internetdata.io",
            &serde_json::json!({
                "ip": "8.8.8.8",
                "location": {
                    "country_name": "United States",
                    "region": "California",
                    "city": "Mountain View"
                },
                "company": { "name": "Google LLC" }
            }),
        )
        .unwrap();
        assert_eq!(result.region.as_deref(), Some("California"));
        assert_eq!(result.city.as_deref(), Some("Mountain View"));
    }

    #[test]
    fn proxy_test_result_internetdata_requires_ip() {
        let result = proxy_test_result_from_value(
            "internetdata.io",
            &serde_json::json!({ "location": { "country_name": "United States" } }),
        );
        assert!(result.is_err());
    }

    #[test]
    fn proxy_test_result_internetdata_tolerates_non_object_payloads() {
        for payload in [serde_json::json!("unexpected"), serde_json::json!([1, 2])] {
            let result = proxy_test_result_from_value("internetdata.io", &payload);
            assert!(result.is_err(), "payload should be rejected: {payload}");
        }
    }

    #[test]
    fn geo_providers_try_internetdata_last() {
        let last = GEO_PROVIDERS.last().expect("at least one geo provider");
        assert_eq!(last.name, "internetdata.io");
        assert_eq!(last.url, "https://api.internetdata.io/myip");
    }

    #[test]
    fn proxy_test_result_rejects_provider_error_response() {
        let result = proxy_test_result_from_value(
            "ipwho.is",
            &serde_json::json!({ "success": false, "message": "reserved range" }),
        );
        assert!(result.is_err());
    }
}
