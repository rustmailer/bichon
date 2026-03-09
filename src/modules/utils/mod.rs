//
// Copyright (c) 2025 rustmailer.com (https://rustmailer.com)
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

use crate::modules::error::BichonResult;
use base64::engine::general_purpose::STANDARD;
use base64::{engine::general_purpose, Engine};
use rand::{rng, RngExt};

use super::error::code::ErrorCode;

pub mod encrypt;
pub mod html;
pub mod net;
pub mod rate_limit;
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
        $crate::modules::error::BichonError::Generic {
            message: $msg,
            location: snafu::Location::default(),
            code: $code,
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
        $crate::modules::utils::generate_token_impl($bit_strength)
    }};
}

pub(crate) fn generate_token_impl(bit_strength: usize) -> String {
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
        $crate::modules::utils::ensure_dir_and_test_access($dir)
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
        $crate::modules::utils::validate_email($email)
    }};
}

#[macro_export]
macro_rules! encrypt {
    ($plaintext:expr) => {{
        $crate::modules::utils::encrypt::encrypt_string($plaintext)
    }};
}

#[macro_export]
macro_rules! decrypt {
    ($plaintext:expr) => {{
        $crate::modules::utils::encrypt::decrypt_string($plaintext)
    }};
}

pub fn validate_email(email: &str) -> crate::modules::error::BichonResult<()> {
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
macro_rules! calculate_hash {
    ($name:expr) => {
        $crate::modules::utils::hash($name)
    };
}

#[macro_export]
macro_rules! id {
    ($bit_strength:expr) => {{
        // Generate a token with the given bit strength
        let token = $crate::modules::utils::generate_token_impl($bit_strength);
        // Hash the generated token
        $crate::modules::utils::hash(&token)
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

pub fn validate_tag(tag: &str) -> Result<(), String> {
    if tag.is_empty() {
        return Err("Tag cannot be empty".to_string());
    }

    const INVALID: &[char] = &[
        '\'', '"', '`', ';', ',', '(', ')', '[', ']', '{', '}', '<', '>',
    ];

    let mut found = Vec::new();

    for c in tag.chars() {
        if INVALID.contains(&c) && !found.contains(&c) {
            found.push(c);
        }
    }

    if !found.is_empty() {
        let chars: String = found.iter().collect();
        return Err(format!("Tag contains invalid characters: {}", chars));
    }

    Ok(())
}
