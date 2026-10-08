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

use crate::{
    database::{
        batch_delete_impl, delete_impl, find_impl, insert_impl, list_all_impl, manager::DB_MANAGER,
        MemDbModel,
    },
    error::BichonResult,
    utc_now,
};
use serde::{Deserialize, Serialize};

const EXPIRATION_DURATION_MS: i64 = 10 * 60 * 1000;

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct OidcPendingEntity {
    pub state: String,
    pub code_verifier: String,
    pub nonce: String,
    pub redirect_after_login: Option<String>,
    pub created_at: i64,
}

impl MemDbModel for OidcPendingEntity {
    fn collection() -> &'static str {
        "oidc_pending"
    }
    fn key(&self) -> String {
        self.state.clone()
    }
}

impl OidcPendingEntity {
    pub fn new(
        state: String,
        code_verifier: String,
        nonce: String,
        redirect_after_login: Option<String>,
    ) -> Self {
        Self {
            state,
            code_verifier,
            nonce,
            redirect_after_login,
            created_at: utc_now!(),
        }
    }

    pub fn save(&self) -> BichonResult<()> {
        insert_impl(DB_MANAGER.db(), self.to_owned())
    }

    pub fn delete(state: &str) -> BichonResult<()> {
        delete_impl::<OidcPendingEntity>(DB_MANAGER.db(), state)
    }

    pub fn clean() -> BichonResult<()> {
        let all = list_all_impl::<OidcPendingEntity>(DB_MANAGER.db())?;
        let now = utc_now!();
        let to_delete: Vec<String> = all
            .into_iter()
            .filter(|e| now - e.created_at > EXPIRATION_DURATION_MS)
            .map(|e| e.state)
            .collect();
        if !to_delete.is_empty() {
            batch_delete_impl::<OidcPendingEntity>(DB_MANAGER.db(), to_delete)?;
        }
        Ok(())
    }

    pub fn get(state: &str) -> BichonResult<Option<OidcPendingEntity>> {
        let entity = find_impl::<OidcPendingEntity>(DB_MANAGER.db(), state)?;
        match entity {
            Some(entity) => {
                if utc_now!() - entity.created_at > EXPIRATION_DURATION_MS {
                    delete_impl::<OidcPendingEntity>(DB_MANAGER.db(), state)?;
                    return Ok(None);
                }
                Ok(Some(entity))
            }
            None => Ok(None),
        }
    }
}
