use std::{
    collections::{BTreeSet, HashSet},
    net::IpAddr,
};

use crate::{
    error::{code::ErrorCode, BichonResult},
    raise_error,
    users::{permissions::Permission, role::UserRole, UserModel},
};

#[derive(Clone, Debug)]
pub struct ClientContext {
    pub ip_addr: Option<IpAddr>,
    pub user: UserModel,
}

impl ClientContext {
    pub fn require_any_permission(
        &self,
        requirements: Vec<(Option<u64>, &str)>,
    ) -> BichonResult<()> {
        for (account_id, permission) in requirements {
            if self.has_permission(account_id, permission) {
                return Ok(());
            }
        }
        Err(raise_error!(
            "Access denied: Insufficient permissions to perform this action.".into(),
            ErrorCode::Forbidden
        ))
    }

    /// Resolve a user's effective permissions and test one against them.
    ///
    /// Expiry is applied here, at resolution time, and nowhere else. There is
    /// deliberately no background job that sweeps expired assignments out of
    /// `global_roles` / `account_access_map`: a sweeper that fails to run, or
    /// that runs on one node of several, would leave an expired delegation
    /// live — and "the grant is still in the list" is not something an auditor
    /// can tell apart from a valid one. Deriving from the stored expiry on
    /// every check makes expiry authoritative rather than eventual, and works
    /// identically on IMAP, SMTP and REST because they all come through here.
    ///
    /// The cost is one map lookup per assignment per check, which is noise
    /// beside the `UserRole::find` DB reads already in this loop.
    fn permissions_allow(user: &UserModel, account_id: Option<u64>, permission: &str) -> bool {
        let now = crate::utc_now!();
        let mut global_perms = HashSet::new();
        for rid in &user.global_roles {
            if user.global_role_expired(*rid, now) {
                continue;
            }
            if let Some(role) = UserRole::find(*rid).ok().flatten() {
                global_perms.extend(role.permissions);
            }
        }

        if Self::check_global_logic(&global_perms, permission) {
            return true;
        }

        if let Some(aid) = account_id {
            if user.account_role_expired(aid, now) {
                return false;
            }
            if let Some(role_id) = user.account_access_map.get(&aid) {
                if let Some(role) = UserRole::find(*role_id).ok().flatten() {
                    if role.permissions.contains(&permission.to_string())
                        || Self::check_account_logic(&role.permissions, permission)
                    {
                        return true;
                    }
                }
            }
        }

        false
    }

    pub fn check_has_permission(
        user: &UserModel,
        account_id: Option<u64>,
        permission: &str,
    ) -> bool {
        if user.is_admin() {
            return true;
        }

        Self::permissions_allow(user, account_id, permission)
    }

    pub fn has_permission(&self, account_id: Option<u64>, permission: &str) -> bool {
        if self.user.is_admin() {
            return true;
        }

        Self::permissions_allow(&self.user, account_id, permission)
    }

    fn check_global_logic(global: &HashSet<String>, perm: &str) -> bool {
        if global.contains(perm) {
            return true;
        }

        match perm {
            Permission::DATA_READ => global.contains(Permission::DATA_READ_ALL),
            // `data:manage:all` is the global counterpart of `data:manage`
            // (repair, tag edits). Without this arm a custom role holding only
            // the `:all` form is refused per-account even though the CLI and
            // the role editor treat the two as equivalent.
            Permission::DATA_MANAGE => global.contains(Permission::DATA_MANAGE_ALL),
            Permission::DATA_DELETE => global.contains(Permission::DATA_DELETE_ALL),
            Permission::DATA_RAW_DOWNLOAD => global.contains(Permission::DATA_RAW_DOWNLOAD_ALL),
            Permission::DATA_EXPORT_BATCH => global.contains(Permission::DATA_EXPORT_BATCH_ALL),
            Permission::ACCOUNT_MANAGE | Permission::ACCOUNT_READ_DETAILS => {
                global.contains(Permission::ACCOUNT_MANAGE_ALL)
            }
            _ => false,
        }
    }

    fn check_account_logic(scoped_perms: &BTreeSet<String>, perm: &str) -> bool {
        if scoped_perms.contains(perm) {
            return true;
        }
        match perm {
            Permission::DATA_READ | Permission::ACCOUNT_READ_DETAILS => {
                scoped_perms.contains(Permission::ACCOUNT_MANAGE)
            }
            _ => false,
        }
    }

    pub fn require_permission(
        &self,
        account_id: Option<u64>,
        permission: &str,
    ) -> BichonResult<()> {
        if self.has_permission(account_id, permission) {
            Ok(())
        } else {
            Err(raise_error!(
                format!("Access Denied: Missing permission '{}'", permission),
                ErrorCode::Forbidden
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn globals(perms: &[&str]) -> HashSet<String> {
        perms.iter().map(|p| p.to_string()).collect()
    }

    /// Every global `X:all` permission must satisfy its scoped form `X`.
    ///
    /// This is written as a convention over the permission table, not as a
    /// list of pairs, so adding a new `X:all` and forgetting the mapping in
    /// `check_global_logic` fails here instead of silently producing a role
    /// that the CLI treats as powerful but every endpoint refuses.
    #[test]
    fn every_global_all_permission_implies_its_scoped_form() {
        for (global, _) in Permission::global_permissions() {
            let Some(scoped) = global.strip_suffix(":all") else {
                continue;
            };
            assert!(
                ClientContext::check_global_logic(&globals(&[global]), scoped),
                "global permission '{global}' must satisfy its scoped form '{scoped}'"
            );
        }
    }

    /// `repair-message` and the tag-edit endpoints check `data:manage` with an
    /// account id. A custom role holding only the global `data:manage:all`
    /// must pass that check, exactly as the CLI and the role editor assume.
    #[test]
    fn data_manage_all_satisfies_the_per_account_permission() {
        assert!(ClientContext::check_global_logic(
            &globals(&[Permission::DATA_MANAGE_ALL]),
            Permission::DATA_MANAGE,
        ));
    }

    /// An unrelated `:all` permission must not leak into `data:manage`.
    #[test]
    fn data_manage_is_not_satisfied_by_other_global_permissions() {
        assert!(!ClientContext::check_global_logic(
            &globals(&[Permission::DATA_DELETE_ALL]),
            Permission::DATA_MANAGE,
        ));
        assert!(!ClientContext::check_global_logic(
            &globals(&[]),
            Permission::DATA_MANAGE,
        ));
    }
}
