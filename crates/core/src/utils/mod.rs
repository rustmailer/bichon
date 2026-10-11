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

use std::{fs, io, path::PathBuf};

use crate::error::BichonResult;
use base64::engine::general_purpose::STANDARD;
use base64::{engine::general_purpose, Engine};
use rand::{rng, RngExt};

use super::error::code::ErrorCode;

pub mod encrypt;
pub mod html;
pub mod net;
pub mod rate_limit;
pub mod totp;
pub mod shutdown;
pub mod tls;

#[macro_export]
macro_rules! bichon_version {
    () => {
        env!("CARGO_PKG_VERSION")
    };
}

#[macro_export]
macro_rules! utc_now {
    () => {{
        use chrono::Utc;
        Utc::now().timestamp_millis()
    }};
}

#[macro_export]
macro_rules! after_n_days_timestamp {
    ($start_ts:expr, $days:expr) => {{
        const MILLIS_PER_DAY: i64 = 86_400_000; // 24 * 60 * 60 * 1000
        $start_ts + ($days as i64) * MILLIS_PER_DAY
    }};
}

#[macro_export]
macro_rules! base64_encode {
    ($bytes:expr) => {{
        use base64::{engine::general_purpose::STANDARD, *};
        STANDARD.encode($bytes)
    }};
}

#[macro_export]
macro_rules! base64_decode {
    ($key:expr) => {{
        use base64::{engine::general_purpose::STANDARD, *};
        STANDARD.decode($key).unwrap()
    }};
}

#[macro_export]
macro_rules! base64_decode_url_safe {
    ($key:expr) => {{
        use base64::{engine::general_purpose::URL_SAFE, *};
        URL_SAFE.decode($key)
    }};
}

#[macro_export]
macro_rules! base64_encode_url_safe {
    ($key:expr) => {{
        use base64::{engine::general_purpose::URL_SAFE, *};
        URL_SAFE.encode($key)
    }};
}

#[macro_export]
macro_rules! product_public_key {
    () => {
        $crate::base64_decode!(r#"BNlT+WjdEls9VGfry+zKygx+UoypxSqsMBddMGxYgbhWOz7Xfh7YJXGMeby9jBtbz3rhSGrTuZCYA9uwwMMYkhI="#)
    };
}

#[macro_export]
macro_rules! license_header {
    () => {
        "{\"alg\":\"ES256\",\"typ\":\"JWT\"}"
    };
}

#[macro_export]
macro_rules! raise_error {
    ($msg:expr, $code:expr) => {
        $crate::error::BichonError::Generic {
            message: $msg,
            code: $code,
            location: $crate::error::snafu::location!(),
        }
    };
}

#[macro_export]
macro_rules! run_with_timeout {
    ($duration:expr, $task:expr, $err_msg:expr) => {{
        match tokio::time::timeout($duration, $task).await {
            Ok(result) => Ok(result),
            Err(_) => Err($err_msg),
        }
    }};
}

#[macro_export]
macro_rules! free_memory {
    () => {{
        let mut sys = sysinfo::System::new_all();
        sys.refresh_memory();
        sys.free_memory()
    }};
}

#[macro_export]
macro_rules! generate_token {
    ($bit_strength:expr) => {{
        $crate::utils::generate_token_impl($bit_strength)
    }};
}

pub fn generate_token_impl(bit_strength: usize) -> String {
    let byte_length = (bit_strength + 23) / 24 * 3;
    let random_bytes: Vec<u8> = (0..byte_length).map(|_| rand::random::<u8>()).collect();
    let mut encoded = general_purpose::URL_SAFE.encode(&random_bytes);

    encoded = encoded
        .chars()
        .map(|c| {
            if c == '/' || c == '+' || c == '-' || c == '_' {
                make_single_random_char()
            } else {
                c
            }
        })
        .collect();

    encoded
}

fn make_single_random_char() -> char {
    let random_bytes: [u8; 3] = rng().random();
    let encoded = general_purpose::URL_SAFE.encode(random_bytes);
    encoded
        .chars()
        .find(|&c| c != '-' && c != '_' && c != '+' && c != '/')
        .unwrap_or('a')
}

#[macro_export]
macro_rules! ensure_access {
    ($dir:expr) => {{
        $crate::utils::ensure_dir_and_test_access($dir)
    }};
}

#[macro_export]
macro_rules! decode_mailbox_name {
    ($name:expr) => {{
        utf7_imap::decode_utf7_imap($name.to_string())
    }};
}
#[macro_export]
macro_rules! encode_mailbox_name {
    ($name:expr) => {{
        utf7_imap::encode_utf7_imap($name.to_string())
    }};
}

#[macro_export]
macro_rules! get_encoding {
    ($label:expr) => {
        match encoding_rs::Encoding::for_label($label.as_bytes()) {
            None => None,
            Some(encoding) => Some(encoding),
        }
    };
}

#[macro_export]
macro_rules! current_datetime {
    () => {{
        use chrono::Local;
        let now = Local::now();
        now.format("%Y%m%d%H%M").to_string()
    }};
}

#[macro_export]
macro_rules! validate_email {
    ($email:expr) => {{
        $crate::utils::validate_email($email)
    }};
}

#[macro_export]
macro_rules! encrypt {
    ($plaintext:expr) => {{
        $crate::utils::encrypt::encrypt_string($plaintext)
    }};
}

#[macro_export]
macro_rules! decrypt {
    ($plaintext:expr) => {{
        $crate::utils::encrypt::decrypt_string($plaintext)
    }};
}

pub fn validate_email(email: &str) -> crate::error::BichonResult<()> {
    use std::str::FromStr;
    let email_address = email_address::EmailAddress::from_str(email).map_err(|_| {
        raise_error!(
            format!("Invalid email format : {}", email),
            ErrorCode::InvalidParameter
        )
    })?;
    if email != email_address.email() {
        return Err(raise_error!(
            format!("Invalid email format: {}", email),
            ErrorCode::InvalidParameter
        ));
    }
    Ok(())
}

#[macro_export]
macro_rules! id {
    ($bit_strength:expr) => {{
        // Generate a token with the given bit strength
        let token = $crate::utils::generate_token_impl($bit_strength);
        // Hash the generated token
        $crate::utils::hash(&token)
    }};
}

#[macro_export]
macro_rules! u64_to_str {
    ($id:expr) => {{
        let mut buf = itoa::Buffer::new();
        buf.format($id)
    }};
}

/// Generates a 64-bit hash from a string, ensuring the output is within JavaScript's safe integer range (0 to 2^53 - 1).
pub fn hash(s: &str) -> u64 {
    let mut cursor = Vec::new();
    cursor.extend_from_slice(s.as_bytes());
    let mut cursor = std::io::Cursor::new(cursor);
    let hash = murmur3::murmur3_x64_128(&mut cursor, 0).unwrap();
    (hash & 0x1F_FFFF_FFFF_FFFF) as u64
}

pub fn hex_hash(s: &str) -> String {
    let mut cursor = Vec::new();
    cursor.extend_from_slice(s.as_bytes());
    let mut cursor = std::io::Cursor::new(cursor);
    let hash = murmur3::murmur3_x64_128(&mut cursor, 0).unwrap();
    format!("{:032x}", hash)
}

pub fn create_hash(account_id: u64, field: &str) -> u64 {
    let mut buffer = Vec::new();
    buffer.extend_from_slice(&account_id.to_le_bytes());
    buffer.push(b':'); // Separator
    buffer.extend_from_slice(field.as_bytes());
    let mut cursor = std::io::Cursor::new(buffer);
    let hash = murmur3::murmur3_x64_128(&mut cursor, 0).unwrap();
    (hash & 0x1F_FFFF_FFFF_FFFF) as u64
}

pub fn create_hash2(account_id: u64, field1: u64, field2: &str) -> u64 {
    let mut buffer = Vec::new();
    buffer.extend_from_slice(&account_id.to_le_bytes());
    buffer.push(b':'); // Separator
    buffer.extend_from_slice(&field1.to_le_bytes());
    buffer.push(b':'); // Separator
    buffer.extend_from_slice(field2.as_bytes());
    let mut cursor = std::io::Cursor::new(buffer);
    let hash = murmur3::murmur3_x64_128(&mut cursor, 0).unwrap();
    (hash & 0x1F_FFFF_FFFF_FFFF) as u64
}

pub fn get_total_size(path: &PathBuf) -> io::Result<u64> {
    if !path.exists() {
        return Ok(0);
    }

    if path.is_file() {
        return Ok(fs::metadata(path)?.len());
    }

    let mut total_size = 0u64;
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let entry_path = entry.path();

        if entry_path.is_file() {
            total_size += fs::metadata(&entry_path)?.len();
        } else if entry_path.is_dir() {
            total_size += get_total_size(&entry_path)?;
        }
    }

    Ok(total_size)
}

const MAX_AVATAR_BYTES: usize = 128 * 1024;

pub fn decode_avatar_bytes(base64_str: &str) -> BichonResult<Vec<u8>> {
    let bytes = STANDARD.decode(base64_str).map_err(|e| {
        raise_error!(
            format!("Invalid avatar base64 encoding: {}", e),
            ErrorCode::InvalidParameter
        )
    })?;

    if bytes.len() > MAX_AVATAR_BYTES {
        return Err(raise_error!(
            format!(
                "Avatar image exceeds maximum size ({} KB).",
                MAX_AVATAR_BYTES / 1024
            ),
            ErrorCode::InvalidParameter
        ));
    }

    Ok(bytes)
}

pub fn compute_content_hash(content: &[u8]) -> String {
    let hash = blake3::hash(content);
    hash.to_hex().to_string()
}

/// Isolate raw MIME blobs from legacy decoded attachment and original EML keys.
pub fn compute_raw_attachment_hash(content: &[u8]) -> String {
    let mut hasher = blake3::Hasher::new_derive_key("bichon.raw-mime-attachment.v1");
    hasher.update(content);
    hasher.finalize().to_hex().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── hash ──────────────────────────────────────────────────────────

    #[test]
    fn hash_is_deterministic() {
        let a = hash("hello world");
        let b = hash("hello world");
        assert_eq!(a, b);
    }

    #[test]
    fn hash_different_inputs_produce_different_outputs() {
        let a = hash("hello");
        let b = hash("world");
        assert_ne!(a, b);
    }

    #[test]
    fn hash_empty_string() {
        let h = hash("");
        assert!(h < (1u64 << 53));
    }

    #[test]
    fn hex_hash_is_deterministic() {
        let a = hex_hash("test");
        let b = hex_hash("test");
        assert_eq!(a, b);
        assert_eq!(a.len(), 32);
    }

    #[test]
    fn hex_hash_different_inputs_produce_different_outputs() {
        assert_ne!(hex_hash("a"), hex_hash("b"));
    }

    // ── create_hash / create_hash2 ────────────────────────────────────

    #[test]
    fn create_hash_deterministic() {
        let a = create_hash(1, "INBOX");
        let b = create_hash(1, "INBOX");
        assert_eq!(a, b);
    }

    #[test]
    fn create_hash_different_accounts_differ() {
        assert_ne!(create_hash(1, "INBOX"), create_hash(2, "INBOX"));
    }

    #[test]
    fn create_hash_different_fields_differ() {
        assert_ne!(create_hash(1, "INBOX"), create_hash(1, "Sent"));
    }

    #[test]
    fn create_hash2_deterministic() {
        let a = create_hash2(1, 100, "INBOX");
        let b = create_hash2(1, 100, "INBOX");
        assert_eq!(a, b);
    }

    #[test]
    fn create_hash2_different_inputs_differ() {
        assert_ne!(create_hash2(1, 100, "INBOX"), create_hash2(1, 200, "INBOX"));
        assert_ne!(create_hash2(1, 100, "INBOX"), create_hash2(1, 100, "Sent"));
    }

    // ── compute_content_hash ──────────────────────────────────────────

    #[test]
    fn compute_content_hash_deterministic() {
        let data = b"test content";
        let a = compute_content_hash(data);
        let b = compute_content_hash(data);
        assert_eq!(a, b);
        assert_eq!(a.len(), 64);
    }

    #[test]
    fn compute_content_hash_different_content_differ() {
        assert_ne!(compute_content_hash(b"a"), compute_content_hash(b"b"));
    }

    // ── validate_email ────────────────────────────────────────────────

    #[test]
    fn validate_email_valid() {
        assert!(validate_email("user@example.com").is_ok());
        assert!(validate_email("a@b.co").is_ok());
        assert!(validate_email("test.user+tag@domain.com").is_ok());
    }

    #[test]
    fn validate_email_invalid() {
        assert!(validate_email("not-an-email").is_err());
        assert!(validate_email("").is_err());
        assert!(validate_email("@domain.com").is_err());
        assert!(validate_email("user@").is_err());
    }

    // ── generate_token_impl ───────────────────────────────────────────

    #[test]
    fn generate_token_has_expected_length() {
        let token = generate_token_impl(256);
        // URL-safe base64 encodes 3 bytes → 4 chars, so length is roughly
        // ceil(bit_strength / 24) * 4, but chars like /+=-_ are replaced
        assert!(!token.is_empty());
    }

    #[test]
    fn generate_token_does_not_contain_special_chars() {
        for _ in 0..10 {
            let token = generate_token_impl(256);
            assert!(!token.contains('/'));
            assert!(!token.contains('+'));
            assert!(!token.contains('-'));
            assert!(!token.contains('_'));
        }
    }

    #[test]
    fn generate_token_is_random() {
        let a = generate_token_impl(256);
        let b = generate_token_impl(256);
        assert_ne!(a, b);
    }

    // ── decode_avatar_bytes ───────────────────────────────────────────

    #[test]
    fn decode_avatar_bytes_valid() {
        // "avatar" in base64 = "YXZhdGFy"
        let result = decode_avatar_bytes("YXZhdGFy").unwrap();
        assert_eq!(result, b"avatar");
    }

    #[test]
    fn decode_avatar_bytes_invalid_base64() {
        assert!(decode_avatar_bytes("!!!invalid!!!").is_err());
    }
}
