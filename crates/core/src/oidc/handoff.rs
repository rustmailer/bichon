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

//! One-shot handoff of a freshly minted WebUI access token from the OIDC
//! callback handler to the SPA.
//!
//! Passing the access token as a URL query parameter to the SPA would leak it
//! into browser history, Referer headers (to images, third-party analytics,
//! CDNs) and server access logs. Instead we store the token server-side
//! under a short-lived, single-use handoff id, redirect the browser to
//! `/sso-callback?handoff=<id>`, and let the SPA POST that id back to
//! `/api/auth/oidc/handoff` to receive the token in the JSON response body.
//!
//! Handoff entries:
//!   * are single-use (deleted on the first successful read),
//!   * expire after 60 seconds,
//!   * are keyed by 256 bits of entropy from a CSPRNG.

use crate::{
    database::{
        batch_delete_impl, delete_impl, find_impl, insert_impl, list_all_impl, manager::DB_MANAGER,
        MemDbModel,
    },
    error::BichonResult,
    generate_token, utc_now,
};
use serde::{Deserialize, Serialize};

const EXPIRATION_DURATION_MS: i64 = 60 * 1000;

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct OidcHandoffEntity {
    pub id: String,
    pub access_token: String,
    pub redirect_after_login: Option<String>,
    pub created_at: i64,
}

impl MemDbModel for OidcHandoffEntity {
    fn collection() -> &'static str {
        "oidc_handoff"
    }
    fn key(&self) -> String {
        self.id.clone()
    }
}

impl OidcHandoffEntity {
    pub fn create(access_token: String, redirect_after_login: Option<String>) -> BichonResult<Self> {
        let entity = Self {
            id: generate_token!(256),
            access_token,
            redirect_after_login,
            created_at: utc_now!(),
        };
        insert_impl(DB_MANAGER.db(), entity.clone())?;
        Ok(entity)
    }

    /// Consume a handoff entry: return it if it exists, is not expired, and
    /// delete it in the same call so it cannot be replayed.
    pub fn consume(id: &str) -> BichonResult<Option<Self>> {
        let entity = find_impl::<OidcHandoffEntity>(DB_MANAGER.db(), id)?;
        match entity {
            Some(entity) => {
                delete_impl::<OidcHandoffEntity>(DB_MANAGER.db(), id)?;
                if utc_now!() - entity.created_at > EXPIRATION_DURATION_MS {
                    return Ok(None);
                }
                Ok(Some(entity))
            }
            None => Ok(None),
        }
    }

    pub fn clean() -> BichonResult<()> {
        let all = list_all_impl::<OidcHandoffEntity>(DB_MANAGER.db())?;
        let now = utc_now!();
        let to_delete: Vec<String> = all
            .into_iter()
            .filter(|e| now - e.created_at > EXPIRATION_DURATION_MS)
            .map(|e| e.id)
            .collect();
        if !to_delete.is_empty() {
            batch_delete_impl::<OidcHandoffEntity>(DB_MANAGER.db(), to_delete)?;
        }
        Ok(())
    }
}


