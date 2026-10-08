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

//! JWKS retrieval and asymmetric ID token signature verification
//! (RS256/384/512, PS256/384/512, ES256/384, EdDSA) using `ring`.

use crate::error::code::ErrorCode;
use crate::error::BichonResult;
use crate::raise_error;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use ring::signature::{self, RsaPublicKeyComponents, UnparsedPublicKey};
use serde::Deserialize;
use std::sync::RwLock;
use std::time::{Duration, Instant};

const JWKS_CACHE_TTL: Duration = Duration::from_secs(3600);
/// Minimum interval between refetches triggered by an unknown `kid`, so a
/// flood of forged tokens cannot hammer the IdP.
const JWKS_MIN_REFRESH: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Jwk {
    pub kty: String,
    #[serde(default)]
    pub kid: Option<String>,
    #[serde(default, rename = "use")]
    pub use_: Option<String>,
    #[serde(default)]
    pub alg: Option<String>,
    #[serde(default)]
    pub n: Option<String>,
    #[serde(default)]
    pub e: Option<String>,
    #[serde(default)]
    pub crv: Option<String>,
    #[serde(default)]
    pub x: Option<String>,
    #[serde(default)]
    pub y: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct JwkSet {
    pub keys: Vec<Jwk>,
}

struct Cached {
    uri: String,
    set: JwkSet,
    fetched_at: Instant,
}

static JWKS_CACHE: RwLock<Option<Cached>> = RwLock::new(None);

async fn fetch(uri: &str) -> BichonResult<JwkSet> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| raise_error!(format!("JWKS client: {e}"), ErrorCode::InternalError))?;
    let resp = client
        .get(uri)
        .send()
        .await
        .map_err(|e| raise_error!(format!("JWKS fetch failed: {e}"), ErrorCode::HttpResponseError))?;
    if !resp.status().is_success() {
        return Err(raise_error!(
            format!("JWKS fetch returned {}", resp.status()),
            ErrorCode::HttpResponseError
        ));
    }
    resp.json::<JwkSet>()
        .await
        .map_err(|e| raise_error!(format!("JWKS parse failed: {e}"), ErrorCode::HttpResponseError))
}

/// Returns the key for `kid`/`alg` from the IdP's JWKS, refetching once when
/// the key is unknown (key rotation).
pub async fn find_key(jwks_uri: &str, kid: Option<&str>, alg: &str) -> BichonResult<Jwk> {
    let cached = JWKS_CACHE.read().ok().and_then(|g| {
        g.as_ref()
            .filter(|c| c.uri == jwks_uri)
            .map(|c| (c.set.clone(), c.fetched_at))
    });
    if let Some((set, fetched_at)) = &cached {
        if fetched_at.elapsed() < JWKS_CACHE_TTL {
            if let Some(k) = select_key(set, kid, alg) {
                return Ok(k);
            }
            if fetched_at.elapsed() < JWKS_MIN_REFRESH {
                return Err(no_key(kid));
            }
        }
    }
    let set = fetch(jwks_uri).await?;
    let key = select_key(&set, kid, alg);
    if let Ok(mut g) = JWKS_CACHE.write() {
        *g = Some(Cached {
            uri: jwks_uri.to_string(),
            set,
            fetched_at: Instant::now(),
        });
    }
    key.ok_or_else(|| no_key(kid))
}

fn no_key(kid: Option<&str>) -> crate::error::BichonError {
    raise_error!(
        format!("No signing key in JWKS for kid={}", kid.unwrap_or("-")),
        ErrorCode::PermissionDenied
    )
}

fn kty_for(alg: &str) -> Option<&'static str> {
    match alg {
        "RS256" | "RS384" | "RS512" | "PS256" | "PS384" | "PS512" => Some("RSA"),
        "ES256" | "ES384" => Some("EC"),
        "EdDSA" | "Ed25519" => Some("OKP"),
        _ => None,
    }
}

/// Picks the signing key: matching `kid` when the token has one, otherwise the
/// only signing key of the right type.
pub fn select_key(set: &JwkSet, kid: Option<&str>, alg: &str) -> Option<Jwk> {
    let kty = kty_for(alg)?;
    let usable = |k: &&Jwk| {
        k.kty == kty
            && k.use_.as_deref().map_or(true, |u| u == "sig")
            && k.alg.as_deref().map_or(true, |a| a == alg)
    };
    match kid {
        Some(kid) => set
            .keys
            .iter()
            .filter(usable)
            .find(|k| k.kid.as_deref() == Some(kid))
            .cloned(),
        None => {
            let mut it = set.keys.iter().filter(usable);
            match (it.next(), it.next()) {
                (Some(k), None) => Some(k.clone()),
                _ => None,
            }
        }
    }
}

fn b64(field: &Option<String>, name: &str) -> BichonResult<Vec<u8>> {
    let v = field
        .as_deref()
        .ok_or_else(|| raise_error!(format!("JWK is missing '{name}'"), ErrorCode::InvalidParameter))?;
    URL_SAFE_NO_PAD
        .decode(v)
        .map_err(|e| raise_error!(format!("JWK '{name}' is not base64url: {e}"), ErrorCode::InvalidParameter))
}

/// Verifies `sig` over `signing_input` with `key` for `alg`.
pub fn verify_signature(key: &Jwk, alg: &str, signing_input: &[u8], sig: &[u8]) -> BichonResult<()> {
    let bad = || raise_error!("ID token signature verification failed".into(), ErrorCode::PermissionDenied);
    match alg {
        "RS256" | "RS384" | "RS512" | "PS256" | "PS384" | "PS512" => {
            let params: &signature::RsaParameters = match alg {
                "RS256" => &signature::RSA_PKCS1_2048_8192_SHA256,
                "RS384" => &signature::RSA_PKCS1_2048_8192_SHA384,
                "RS512" => &signature::RSA_PKCS1_2048_8192_SHA512,
                "PS256" => &signature::RSA_PSS_2048_8192_SHA256,
                "PS384" => &signature::RSA_PSS_2048_8192_SHA384,
                _ => &signature::RSA_PSS_2048_8192_SHA512,
            };
            let pk = RsaPublicKeyComponents {
                n: b64(&key.n, "n")?,
                e: b64(&key.e, "e")?,
            };
            pk.verify(params, signing_input, sig).map_err(|_| bad())
        }
        "ES256" | "ES384" => {
            let (alg_ring, crv): (&signature::EcdsaVerificationAlgorithm, &str) = if alg == "ES256" {
                (&signature::ECDSA_P256_SHA256_FIXED, "P-256")
            } else {
                (&signature::ECDSA_P384_SHA384_FIXED, "P-384")
            };
            if key.crv.as_deref() != Some(crv) {
                return Err(bad());
            }
            let mut point = vec![0x04];
            point.extend(b64(&key.x, "x")?);
            point.extend(b64(&key.y, "y")?);
            UnparsedPublicKey::new(alg_ring, point)
                .verify(signing_input, sig)
                .map_err(|_| bad())
        }
        "EdDSA" | "Ed25519" => {
            if key.crv.as_deref() != Some("Ed25519") {
                return Err(bad());
            }
            UnparsedPublicKey::new(&signature::ED25519, b64(&key.x, "x")?)
                .verify(signing_input, sig)
                .map_err(|_| bad())
        }
        other => Err(raise_error!(
            format!("Unsupported ID token signing algorithm '{other}'"),
            ErrorCode::PermissionDenied
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::rand::SystemRandom;
    use ring::signature::{EcdsaKeyPair, Ed25519KeyPair, KeyPair};

    #[test]
    fn es256_roundtrip_and_tamper() {
        let rng = SystemRandom::new();
        let pkcs8 =
            EcdsaKeyPair::generate_pkcs8(&signature::ECDSA_P256_SHA256_FIXED_SIGNING, &rng).unwrap();
        let kp = EcdsaKeyPair::from_pkcs8(&signature::ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref(), &rng)
            .unwrap();
        let sig = kp.sign(&rng, b"header.payload").unwrap();
        let pk = kp.public_key().as_ref();
        let jwk = Jwk {
            kty: "EC".into(),
            kid: Some("k1".into()),
            use_: Some("sig".into()),
            alg: Some("ES256".into()),
            n: None,
            e: None,
            crv: Some("P-256".into()),
            x: Some(URL_SAFE_NO_PAD.encode(&pk[1..33])),
            y: Some(URL_SAFE_NO_PAD.encode(&pk[33..65])),
        };
        assert!(verify_signature(&jwk, "ES256", b"header.payload", sig.as_ref()).is_ok());
        assert!(verify_signature(&jwk, "ES256", b"header.payloaX", sig.as_ref()).is_err());
    }

    #[test]
    fn ed25519_roundtrip() {
        let rng = SystemRandom::new();
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng).unwrap();
        let kp = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap();
        let jwk = Jwk {
            kty: "OKP".into(),
            kid: None,
            use_: None,
            alg: None,
            n: None,
            e: None,
            crv: Some("Ed25519".into()),
            x: Some(URL_SAFE_NO_PAD.encode(kp.public_key().as_ref())),
            y: None,
        };
        let sig = kp.sign(b"abc");
        assert!(verify_signature(&jwk, "EdDSA", b"abc", sig.as_ref()).is_ok());
        assert!(verify_signature(&jwk, "EdDSA", b"abd", sig.as_ref()).is_err());
    }

    #[test]
    fn select_key_by_kid_type_and_use() {
        let rsa = |kid: &str, use_: &str| Jwk {
            kty: "RSA".into(),
            kid: Some(kid.into()),
            use_: Some(use_.into()),
            alg: Some("RS256".into()),
            n: Some("AQAB".into()),
            e: Some("AQAB".into()),
            crv: None,
            x: None,
            y: None,
        };
        let set = JwkSet {
            keys: vec![rsa("enc", "enc"), rsa("a", "sig"), rsa("b", "sig")],
        };
        assert_eq!(select_key(&set, Some("b"), "RS256").unwrap().kid.as_deref(), Some("b"));
        assert!(select_key(&set, Some("enc"), "RS256").is_none());
        assert!(select_key(&set, Some("a"), "ES256").is_none());
        // Ambiguous without kid.
        assert!(select_key(&set, None, "RS256").is_none());
        let single = JwkSet { keys: vec![rsa("a", "sig")] };
        assert!(select_key(&single, None, "RS256").is_some());
        assert!(select_key(&single, None, "none").is_none());
    }
}
