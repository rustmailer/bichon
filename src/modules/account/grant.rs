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

use poem_openapi::Object;
use serde::{Deserialize, Serialize};

use crate::{
    modules::{
        account::migration::AccountModel,
        common::auth::ClientContext,
        database::{manager::DB_MANAGER, with_transaction},
        error::{code::ErrorCode, BichonResult},
        users::{
            permissions::Permission,
            role::{RoleType, UserRole},
            UserModel,
        },
    },
    raise_error, utc_now,
};

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize, Object)]
pub struct BatchAccountRoleRequest {
    pub account_ids: Vec<u64>,
    pub user_ids: Vec<u64>,
    pub role_id: u64,
}

impl BatchAccountRoleRequest {
    pub async fn validate_existence(&self) -> BichonResult<()> {
        let role = UserRole::find(self.role_id).await?.ok_or_else(|| {
            raise_error!(
                format!("Role ID {} not found", self.role_id),
                ErrorCode::ResourceNotFound
            )
        })?;

        if !matches!(role.role_type, RoleType::Account) {
            return Err(raise_error!(
                "Only Account roles can be assigned to individual account".into(),
                ErrorCode::InvalidParameter
            ));
        }

        for id in &self.account_ids {
            let exists = AccountModel::find(*id).await?; // Assuming an exists helper
            if exists.is_none() {
                return Err(raise_error!(
                    format!("Account ID {} not found", id),
                    ErrorCode::ResourceNotFound
                ));
            }
        }

        for id in &self.user_ids {
            let exists = UserModel::find(*id).await?; // Assuming an exists helper
            if exists.is_none() {
                return Err(raise_error!(
                    format!("User ID {} not found", id),
                    ErrorCode::ResourceNotFound
                ));
            }
        }

        Ok(())
    }

    async fn grant_batch_account_access(
        account_ids: Vec<u64>,
        user_ids: Vec<u64>,
        role_id: u64,
    ) -> BichonResult<()> {
        with_transaction(DB_MANAGER.meta_db(), move |rw| {
            for &uid in &user_ids {
                // Fetch the current user record from the database
                let user = rw
                    .get()
                    .primary::<UserModel>(uid)
                    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
                    .ok_or_else(|| {
                        raise_error!(
                            format!("User with id={} not found.", uid),
                            ErrorCode::ResourceNotFound
                        )
                    })?;

                let mut updated_user = user.clone();

                // Apply the role to each specified account_id
                for &aid in &account_ids {
                    updated_user.account_access_map.insert(aid, role_id);
                }

                updated_user.updated_at = utc_now!();

                // Save the updated user back to the database within the transaction
                rw.update(user, updated_user)
                    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
            }
            Ok(())
        })
        .await
    }

    pub async fn do_assign(self, context: &ClientContext) -> BichonResult<()> {
        for account_id in &self.account_ids {
            // Get the user's specific access for this account
            let assigned_role_id =
                context
                    .user
                    .account_access_map
                    .get(account_id)
                    .ok_or_else(|| {
                        raise_error!(
                            format!("No access to account {}", account_id),
                            ErrorCode::Forbidden
                        )
                    })?;

            // Fetch the role definition from the database
            let user_scoped_role = UserRole::find(*assigned_role_id).await?.ok_or_else(|| {
                raise_error!(
                    "Assigned account role no longer exists".into(),
                    ErrorCode::InternalError
                )
            })?;

            // Critical Check: Does this role grant management/sharing rights?
            if !user_scoped_role
                .permissions
                .contains(Permission::ACCOUNT_MANAGE)
            {
                return Err(raise_error!(
                    format!("Your role on account {} does not allow sharing", account_id),
                    ErrorCode::Forbidden
                ));
            }

            // Optional: Ensure manager isn't giving away perms they don't have
            // This is where you'd compare target_role.permissions vs manager's perms
        }

        Self::grant_batch_account_access(self.account_ids, self.user_ids, self.role_id).await
    }
}
