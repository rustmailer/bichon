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
use crate::error::BichonResult;
use crate::oidc::jwks::{verify_signature, Jwk};
use crate::raise_error;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use ring::{digest, hmac};
use serde::Deserialize;
use serde_json::{Map, Value};

/// Constant-time byte comparison, so nonce checks do not leak timing.
fn eq_ct(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[derive(Debug, Clone, Deserialize)]
pub struct IdTokenClaims {
    pub iss: String,
    #[serde(default)]
    pub aud: Value,
    pub sub: String,
    pub exp: i64,
    #[serde(default)]
    pub iat: Option<i64>,
    #[serde(default)]
    pub nbf: Option<i64>,
    #[serde(default)]
    pub azp: Option<String>,
    #[serde(default)]
    pub nonce: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub email_verified: Option<bool>,
    #[serde(default)]
    pub preferred_username: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub given_name: Option<String>,
    /// Every other claim (groups, custom claims), for configurable lookups.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Deserialize)]
pub struct Header {
    pub alg: String,
    #[serde(default)]
    pub kid: Option<String>,
}

pub struct VerifyParams<'a> {
    pub expected_issuer: &'a str,
    pub expected_audience: &'a str,
    pub expected_nonce: &'a str,
    /// Client secret bytes, used only for HS256.
    pub client_secret: &'a [u8],
    /// Public key from the IdP's JWKS, required for asymmetric algorithms.
    pub key: Option<&'a Jwk>,
    /// Clock skew tolerance in seconds.
    pub clock_skew_secs: i64,
    /// Current unix time in seconds.
    pub now_secs: i64,
}

fn split_jwt(token: &str) -> BichonResult<(&str, &str, &str)> {
    let mut parts = token.split('.');
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(h), Some(p), Some(s), None) => Ok((h, p, s)),
        _ => Err(raise_error!(
            "ID token is not a well-formed JWT (expected 3 segments)".into(),
            ErrorCode::InvalidParameter
        )),
    }
}

fn b64url_decode(s: &str) -> BichonResult<Vec<u8>> {
    URL_SAFE_NO_PAD.decode(s).map_err(|e| {
        raise_error!(
            format!("Failed to base64url-decode ID token segment: {}", e),
            ErrorCode::InvalidParameter
        )
    })
}

/// Reads the (unverified) JOSE header to choose the verification key.
pub fn peek_header(token: &str) -> BichonResult<Header> {
    let (h, _, _) = split_jwt(token)?;
    serde_json::from_slice(&b64url_decode(h)?).map_err(|e| {
        raise_error!(
            format!("Failed to parse ID token header: {}", e),
            ErrorCode::InvalidParameter
        )
    })
}

fn audience_matches(claim: &Value, expected: &str) -> bool {
    match claim {
        Value::String(s) => s == expected,
        Value::Array(arr) => arr.iter().any(|v| v.as_str() == Some(expected)),
        _ => false,
    }
}

fn verify_hs256(secret: &[u8], signing_input: &[u8], signature: &[u8]) -> BichonResult<()> {
    // IdPs differ on the HS256 key: the raw client secret (OIDC Core 10.1) or
    // SHA-256 of it. Both need the shared secret, so accept either.
    let raw = hmac::Key::new(hmac::HMAC_SHA256, secret);
    if hmac::verify(&raw, signing_input, signature).is_ok() {
        return Ok(());
    }
    let derived = digest::digest(&digest::SHA256, secret);
    hmac::verify(&hmac::Key::new(hmac::HMAC_SHA256, derived.as_ref()), signing_input, signature)
        .map_err(|_| {
            raise_error!(
                "ID token HS256 signature verification failed".into(),
                ErrorCode::PermissionDenied
            )
        })
}

/// Verifies the signature (HS256 with the client secret, or an asymmetric
/// algorithm with a JWKS key) and the standard claims (`iss`, `aud`, `azp`,
/// `exp`, `nbf`, `nonce`), and returns the claims.
pub fn verify_and_parse(token: &str, params: &VerifyParams<'_>) -> BichonResult<IdTokenClaims> {
    let (h_b64, p_b64, s_b64) = split_jwt(token)?;
    let header = peek_header(token)?;
    let signing_input = format!("{}.{}", h_b64, p_b64);
    let signature = b64url_decode(s_b64)?;

    match header.alg.as_str() {
        "HS256" => verify_hs256(params.client_secret, signing_input.as_bytes(), &signature)?,
        "none" | "" => {
            return Err(raise_error!(
                "Unsigned ID tokens are not accepted".into(),
                ErrorCode::PermissionDenied
            ))
        }
        alg => {
            let key = params.key.ok_or_else(|| {
                raise_error!(
                    format!("No JWKS key available to verify a {alg} ID token"),
                    ErrorCode::PermissionDenied
                )
            })?;
            verify_signature(key, alg, signing_input.as_bytes(), &signature)?;
        }
    }

    let claims: IdTokenClaims = serde_json::from_slice(&b64url_decode(p_b64)?).map_err(|e| {
        raise_error!(
            format!("Failed to parse ID token claims: {}", e),
            ErrorCode::InvalidParameter
        )
    })?;

    if claims.iss.trim_end_matches('/') != params.expected_issuer.trim_end_matches('/') {
        return Err(raise_error!(
            format!(
                "ID token issuer mismatch: expected '{}', got '{}'",
                params.expected_issuer, claims.iss
            ),
            ErrorCode::PermissionDenied
        ));
    }
    if !audience_matches(&claims.aud, params.expected_audience) {
        return Err(raise_error!(
            "ID token audience does not include this client".into(),
            ErrorCode::PermissionDenied
        ));
    }
    // OIDC Core 3.1.3.7: with several audiences, azp must be this client.
    if matches!(&claims.aud, Value::Array(a) if a.len() > 1)
        && claims.azp.as_deref() != Some(params.expected_audience)
    {
        return Err(raise_error!(
            "ID token azp does not match this client".into(),
            ErrorCode::PermissionDenied
        ));
    }
    if params.now_secs > claims.exp + params.clock_skew_secs {
        return Err(raise_error!("ID token has expired".into(), ErrorCode::PermissionDenied));
    }
    if claims.nbf.is_some_and(|nbf| params.now_secs + params.clock_skew_secs < nbf) {
        return Err(raise_error!("ID token is not yet valid".into(), ErrorCode::PermissionDenied));
    }
    let nonce_ok = claims
        .nonce
        .as_deref()
        .is_some_and(|n| eq_ct(n.as_bytes(), params.expected_nonce.as_bytes()));
    if !nonce_ok {
        return Err(raise_error!(
            "ID token nonce mismatch, possible replay".into(),
            ErrorCode::PermissionDenied
        ));
    }

    Ok(claims)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::rand::SystemRandom;
    use ring::signature::{KeyPair, RsaKeyPair, RsaPublicKeyComponents, RSA_PKCS1_SHA256};

    const RSA_PK8: &[u8] = include_bytes!("testdata/rsa2048-test.pk8");

    fn payload(nonce: &str, exp: i64, extra: &str) -> String {
        format!(
            r#"{{"iss":"https://auth.example.com","aud":"bichon","sub":"user1","exp":{exp},"nonce":"{nonce}","email":"user1@example.com","preferred_username":"user1"{extra}}}"#
        )
    }

    fn hs_token(secret_key: &[u8], payload: &str) -> String {
        let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"HS256","typ":"JWT"}"#);
        let input = format!("{}.{}", header, URL_SAFE_NO_PAD.encode(payload));
        let sig = hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, secret_key), input.as_bytes());
        format!("{}.{}", input, URL_SAFE_NO_PAD.encode(sig.as_ref()))
    }

    /// RS256 token and matching JWK, as Authelia issues them.
    fn rs_token(payload: &str, kid: &str) -> (String, Jwk) {
        let kp = RsaKeyPair::from_pkcs8(RSA_PK8).unwrap();
        let header = URL_SAFE_NO_PAD.encode(format!(r#"{{"alg":"RS256","kid":"{kid}","typ":"JWT"}}"#));
        let input = format!("{}.{}", header, URL_SAFE_NO_PAD.encode(payload));
        let mut sig = vec![0; kp.public().modulus_len()];
        kp.sign(&RSA_PKCS1_SHA256, &SystemRandom::new(), input.as_bytes(), &mut sig)
            .unwrap();
        let pk = RsaPublicKeyComponents::<Vec<u8>>::from(kp.public_key());
        let jwk = Jwk {
            kty: "RSA".into(),
            kid: Some(kid.into()),
            use_: Some("sig".into()),
            alg: Some("RS256".into()),
            n: Some(URL_SAFE_NO_PAD.encode(&pk.n)),
            e: Some(URL_SAFE_NO_PAD.encode(&pk.e)),
            crv: None,
            x: None,
            y: None,
        };
        (format!("{}.{}", input, URL_SAFE_NO_PAD.encode(sig)), jwk)
    }

    fn params<'a>(secret: &'a [u8], key: Option<&'a Jwk>) -> VerifyParams<'a> {
        VerifyParams {
            expected_issuer: "https://auth.example.com",
            expected_audience: "bichon",
            expected_nonce: "test-nonce",
            client_secret: secret,
            key,
            clock_skew_secs: 60,
            now_secs: 1_900_000_000,
        }
    }

    const SECRET: &[u8] = b"test-client-secret-with-enough-length-123";

    #[test]
    fn hs256_raw_and_derived_keys() {
        let p = payload("test-nonce", 2_000_000_000, "");
        assert!(verify_and_parse(&hs_token(SECRET, &p), &params(SECRET, None)).is_ok());
        let derived = digest::digest(&digest::SHA256, SECRET);
        assert!(verify_and_parse(&hs_token(derived.as_ref(), &p), &params(SECRET, None)).is_ok());
        assert!(verify_and_parse(&hs_token(b"attacker-key-000000000000", &p), &params(SECRET, None)).is_err());
    }

    #[test]
    fn rs256_verifies_with_jwk_and_keeps_extra_claims() {
        let p = payload("test-nonce", 2_000_000_000, r#","groups":["admins","users"]"#);
        let (tok, jwk) = rs_token(&p, "k1");
        let claims = verify_and_parse(&tok, &params(SECRET, Some(&jwk))).expect("valid RS256");
        assert_eq!(claims.sub, "user1");
        assert_eq!(claims.extra["groups"], serde_json::json!(["admins", "users"]));
        assert!(verify_and_parse(&tok, &params(SECRET, None)).is_err(), "no key");
        let mut parts: Vec<&str> = tok.split('.').collect();
        let forged = URL_SAFE_NO_PAD.encode(p.replace("user1", "admin"));
        parts[1] = &forged;
        assert!(verify_and_parse(&parts.join("."), &params(SECRET, Some(&jwk))).is_err(), "tampered");
    }

    #[test]
    fn rejects_bad_claims() {
        let ok = payload("test-nonce", 2_000_000_000, "");
        let cases = [
            ("nonce", payload("other-nonce", 2_000_000_000, "")),
            ("exp", payload("test-nonce", 1_800_000_000, "")),
            ("aud", ok.replace("\"bichon\"", "\"other\"")),
            ("iss", ok.replace("auth.example.com", "evil.example.com")),
            ("nbf", payload("test-nonce", 2_000_000_000, r#","nbf":1950000000"#)),
            ("azp", ok.replace("\"aud\":\"bichon\"", "\"aud\":[\"bichon\",\"x\"],\"azp\":\"x\"")),
        ];
        for (what, p) in cases {
            let (tok, jwk) = rs_token(&p, "k1");
            assert!(verify_and_parse(&tok, &params(SECRET, Some(&jwk))).is_err(), "{what}");
        }
    }

    #[test]
    fn rejects_alg_none() {
        let h = URL_SAFE_NO_PAD.encode(r#"{"alg":"none"}"#);
        let p = URL_SAFE_NO_PAD.encode(payload("test-nonce", 2_000_000_000, ""));
        assert!(verify_and_parse(&format!("{h}.{p}."), &params(SECRET, None)).is_err());
    }
}
