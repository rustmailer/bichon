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

use base64::{engine::general_purpose, Engine as _};
use ring::aead::{Aad, BoundKey, Nonce, NonceSequence, OpeningKey, SealingKey, AES_256_GCM};
use ring::pbkdf2::{self, derive};
use ring::rand::{SecureRandom, SystemRandom};
use std::fs;
use std::num::NonZeroU32;
use std::sync::LazyLock;

use crate::modules::error::code::ErrorCode;
use crate::modules::error::BichonResult;
use crate::modules::settings::cli::SETTINGS;
use crate::raise_error;

static ENCRYPT_PASSWORD: LazyLock<String> =
    LazyLock::new(|| match &SETTINGS.bichon_encrypt_password {
        Some(p) => p.clone(),
        None => {
            // unwrap() is safe here, because SETTINGS validates that at least one of the encrypt_password
            // fields is set.
            fs::read_to_string(SETTINGS.bichon_encrypt_password_file.as_ref().unwrap())
                .expect("failed to read the file with the encrypt password")
        }
    });

struct SingleNonceSequence([u8; 12]);

impl SingleNonceSequence {
    fn new(nonce: [u8; 12]) -> Self {
        SingleNonceSequence(nonce)
    }
}

impl NonceSequence for SingleNonceSequence {
    fn advance(&mut self) -> Result<Nonce, ring::error::Unspecified> {
        Ok(Nonce::assume_unique_for_key(self.0))
    }
}

pub fn encrypt_string(plaintext: &str) -> BichonResult<String> {
    internal_encrypt_string(&ENCRYPT_PASSWORD, plaintext)
        .map_err(|_| raise_error!("Failed to encrypt string.".into(), ErrorCode::InternalError))
}

pub fn decrypt_string(data: &str) -> BichonResult<String> {
    internal_decrypt_string(&ENCRYPT_PASSWORD, data).map_err(|_| {
        raise_error!(
            "Decryption failed, likely due to incorrect encryption key or corrupted data".into(),
            ErrorCode::InternalError
        )
    })
}

fn internal_encrypt_string(
    password: &str,
    plaintext: &str,
) -> Result<String, ring::error::Unspecified> {
    let rng = SystemRandom::new();
    let mut salt = [0u8; 32];
    rng.fill(&mut salt)?;
    let mut key = [0u8; 32];
    derive(
        pbkdf2::PBKDF2_HMAC_SHA256,
        NonZeroU32::new(100_000).unwrap(),
        &salt,
        password.as_bytes(),
        &mut key,
    );
    let mut nonce_bytes = [0u8; 12];
    rng.fill(&mut nonce_bytes)?;
    let unbound_key = ring::aead::UnboundKey::new(&AES_256_GCM, &key)?;
    let nonce_sequence = SingleNonceSequence::new(nonce_bytes);
    let mut sealing_key = SealingKey::new(unbound_key, nonce_sequence);
    let mut in_out = plaintext.as_bytes().to_vec();
    let aad = Aad::empty();
    sealing_key.seal_in_place_append_tag(aad, &mut in_out)?;
    let mut result = Vec::with_capacity(32 + 12 + in_out.len());
    result.extend_from_slice(&salt);
    result.extend_from_slice(&nonce_bytes);
    result.extend_from_slice(&in_out);
    Ok(general_purpose::URL_SAFE.encode(&result))
}

fn internal_decrypt_string(password: &str, data: &str) -> Result<String, ring::error::Unspecified> {
    let data = general_purpose::URL_SAFE
        .decode(data)
        .map_err(|_| ring::error::Unspecified)?;
    if data.len() < 32 + 12 {
        return Err(ring::error::Unspecified);
    }
    let salt = &data[0..32];
    let nonce_bytes: [u8; 12] = data[32..44]
        .try_into()
        .map_err(|_| ring::error::Unspecified)?;
    let ciphertext = &data[44..];
    let mut key = [0u8; 32];
    derive(
        pbkdf2::PBKDF2_HMAC_SHA256,
        NonZeroU32::new(100_000).unwrap(),
        salt,
        password.as_bytes(),
        &mut key,
    );
    let unbound_key = ring::aead::UnboundKey::new(&AES_256_GCM, &key)?;
    let nonce_sequence = SingleNonceSequence::new(nonce_bytes);
    let mut opening_key = OpeningKey::new(unbound_key, nonce_sequence);
    let mut in_out = ciphertext.to_vec();
    let aad = Aad::empty();
    let decrypted_bytes = opening_key.open_in_place(aad, &mut in_out)?;
    String::from_utf8(decrypted_bytes.to_vec()).map_err(|_| ring::error::Unspecified)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encrypt_decrypt() {
        let password = "my_secure_passwasdasdasdasdasord";
        let plaintext = "Helloasdasdasdasdasd, World!";
        let encrypted = internal_encrypt_string(password, plaintext).unwrap();
        println!("{}", &encrypted);
        let decrypted = internal_decrypt_string(password, &encrypted).unwrap();
        assert_eq!(decrypted, plaintext);
    }
}
