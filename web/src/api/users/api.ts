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

import axiosInstance from "@/api/axiosInstance";

export type RoleType = 'Global' | 'Account';

export interface UserRole {
    id: number;
    name: string;
    description?: string | null;
    permissions: string[];
    is_builtin: boolean;
    role_type: RoleType;
    created_at: number;
    updated_at: number;
}

export function getPermissions(
    t: (key: string) => string,
    opts?: { isEnterprise?: boolean }
) {
    const list = [
        // 1. Global Management
        { label: t('permission.system.access'), value: 'system:access' },
        { label: t('permission.system.root'), value: 'system:root' },
        { label: t('permission.system.metrics'), value: 'system:metrics' },
        { label: t('permission.user.manage'), value: 'user:manage' },
        { label: t('permission.user.view'), value: 'user:view' },
        { label: t('permission.token.manage'), value: 'token:manage' },
        { label: t('permission.account.create'), value: 'account:create' },

        // 2. Global "ALL" Scoped (Admin)
        { label: t('permission.account.manage_all'), value: 'account:manage:all' },
        { label: t('permission.data.read_all'), value: 'data:read:all' },
        { label: t('permission.data.manage_all'), value: 'data:manage:all' },
        { label: t('permission.data.raw_download_all'), value: 'data:raw:download:all' },
        { label: t('permission.data.delete_all'), value: 'data:delete:all' },
        { label: t('permission.data.export_batch_all'), value: 'data:export:batch:all' },

        // 3. Scoped / Limited
        { label: t('permission.account.manage'), value: 'account:manage' },
        { label: t('permission.account.read_details'), value: 'account:read_details' },
        { label: t('permission.data.read'), value: 'data:read' },
        { label: t('permission.data.manage'), value: 'data:manage' },
        { label: t('permission.data.raw_download'), value: 'data:raw:download' },
        { label: t('permission.data.delete'), value: 'data:delete' },
        { label: t('permission.data.export_batch'), value: 'data:export:batch' },
        { label: t('permission.data.import_batch'), value: 'data:import:batch' },
        { label: t('permission.data.smtp_ingest'), value: 'data:smtp:ingest' },
    ]

    // 4. Compliance suite (Enterprise only — inert permissions otherwise)
    if (opts?.isEnterprise) {
        list.push(
            { label: t('permission.legal.hold'), value: 'legal:hold' },
            { label: t('permission.timestamp.manage'), value: 'timestamp:manage' },
            { label: t('permission.compliance.audit'), value: 'compliance:audit' },
            { label: t('permission.approval.decide'), value: 'approval:decide' },
        )
    }

    return list
}

export interface RateLimit {
    quota: number;
    interval: number;
}

export interface AccessControl {
    ip_whitelist?: string[];
    rate_limit?: RateLimit;
}

export type TokenType = "WebUI" | "Api";

export interface AccessToken {
    user_id: number;
    user_name: string,
    user_email: string,
    /** Public, non-secret identifier used for revocation. */
    token_id: string;
    /** Leading characters of the token, for display only. */
    token_prefix: string;
    created_at: number;
    updated_at: number;
    name?: string;
    last_access_at: number;
    expire_at?: number | null;
    token_type: TokenType;
    /** When true, the token may only call GET /api/v1/metrics. */
    metrics_only?: boolean;
}

export interface User {
    id: number;
    username: string;
    email: string;
    password?: string | null;
    description?: string | null;
    global_roles: number[];
    global_roles_names: string[];
    /** Deadline (unix ms) per role id for time-limited delegations. A role
     *  absent from this map is permanent. The server omits the key entirely
     *  when there are no delegations, so it may be undefined. */
    global_role_expiries?: Record<number, number>;
    /** Deadline (unix ms) per account id, for scoped delegations. */
    account_role_expiries?: Record<number, number>;
    avatar?: string;
    acl?: AccessControl;
    account_access_map: Record<number, number>;
    account_roles_summary: Record<number, string>;
    global_permissions: string[]
    account_permissions: Record<number, string[]>
    created_at: number;
    updated_at: number;
    sso_id?: string | null;
    sso_provider?: string | null;
}

export interface LoginResult {
    success: boolean;
    error_message?: string | null;
    access_token?: string | null;
    /** The profile's saved theme; check it with `isTheme` before applying. */
    theme?: string,
    language?: string,
    mfa_required?: boolean,
    mfa_challenge?: string | null,
}


export interface MinimalUser {
    id: number;
    username: string;
    email: string;
}


export const login = async (data: Record<string, any>) => {
    const response = await axiosInstance.post<LoginResult>(`api/login`, data);
    return response.data;
};

// Pro/Enterprise: synchronous LDAP/AD bind. Returns the same LoginResult shape as
// `login`, so the form reuses its existing token + MFA handling unchanged.
export const ldapLogin = async (data: { username: string; password: string }) => {
    const response = await axiosInstance.post<LoginResult>(`api/auth/ldap/login`, data);
    return response.data;
};

export interface MfaStatus {
    enabled: boolean;
    has_secret: boolean;
}

export interface MfaEnrollResult {
    secret: string;
    otpauth_uri: string;
}

export interface MfaConfirmResult {
    recovery_codes: string[];
}

export const mfaVerify = async (challenge: string, code: string) => {
    const response = await axiosInstance.post<LoginResult>(`api/auth/mfa/verify`, { challenge, code });
    return response.data;
};

export const mfaStatus = async () => {
    const response = await axiosInstance.get<MfaStatus>(`api/v1/mfa/status`);
    return response.data;
};

export const mfaEnroll = async () => {
    const response = await axiosInstance.post<MfaEnrollResult>(`api/v1/mfa/enroll`);
    return response.data;
};

export const mfaConfirm = async (code: string) => {
    const response = await axiosInstance.post<MfaConfirmResult>(`api/v1/mfa/confirm`, { code });
    return response.data;
};

export const mfaDisable = async (code: string) => {
    await axiosInstance.post(`api/v1/mfa/disable`, { code });
};
export const mfaAdminReset = async (userId: number) => {
    await axiosInstance.post(`api/v1/mfa/admin-reset/${userId}`);
};

export const reset_admin_token = async () => {
    const response = await axiosInstance.post("api/v1/reset-admin-token");
    return response.data;
};

export const reset_admin_password = async (password: string) => {
    const response = await axiosInstance.post("api/v1/reset-admin-password", password, {
        headers: {
            "Content-Type": "text/plain",
        },
    });
    return response.data;
};

export const list_access_tokens = async () => {
    const response = await axiosInstance.get<AccessToken[]>("api/v1/access-token-list");
    return response.data;
};

export const create_access_token = async (data: Record<string, any>) => {
    const response = await axiosInstance.post("api/v1/access-token", data);
    return response.data;
}

export const update_access_token = async (token_id: string, data: Record<string, any>) => {
    const response = await axiosInstance.post(`api/v1/access-token/${token_id}`, data);
    return response.data;
}

export const remove_access_token = async (token_id: string) => {
    const response = await axiosInstance.delete(`api/v1/access-token/${token_id}`);
    return response.data;
}


export const list_roles = async () => {
    const response = await axiosInstance.get<UserRole[]>("api/v1/list-roles");
    return response.data;
};


export const remove_role = async (id: number) => {
    const response = await axiosInstance.delete(`api/v1/roles/${id}`);
    return response.data;
};


export const create_role = async (data: Record<string, any>) => {
    const response = await axiosInstance.post("api/v1/roles", data);
    return response.data;
};


export const update_role = async (id: number, data: Record<string, any>) => {
    const response = await axiosInstance.post(`api/v1/roles/${id}`, data);
    return response.data;
};


export const list_users = async () => {
    const response = await axiosInstance.get<User[]>("api/v1/list-users");
    return response.data;
};


export const list_minimal_users = async () => {
    const response = await axiosInstance.get<MinimalUser[]>("api/v1/minimal-user-list");
    return response.data;
};

export const list_account_roles = async () => {
    const response = await axiosInstance.get<UserRole[]>("api/v1/list-account-roles");
    return response.data;
};

export const remove_user = async (id: number) => {
    const response = await axiosInstance.delete(`api/v1/users/${id}`);
    return response.data;
};


export const create_user = async (data: Record<string, any>) => {
    const response = await axiosInstance.post("api/v1/users", data);
    return response.data;
};


export const update_user = async (id: number, data: Record<string, any>) => {
    const response = await axiosInstance.post(`api/v1/users/${id}`, data);
    return response.data;
};

export const get_user_tokens = async (id: number) => {
    const response = await axiosInstance.get<AccessToken[]>(`api/v1/user-tokens/${id}`);
    return response.data;
};

export const get_current_user = async () => {
    const response = await axiosInstance.get<User>("api/v1/current-user");
    return response.data;
};

// ── Time-limited role delegation (Enterprise) ──────────────────────
//
// The deadline lives on the *assignment*, not the role, so these calls take a
// role id that the user must already hold. Assigning a role stays on
// `update_user`; this only puts an end date on an assignment that exists.

/** One delegation. `expired` is computed server-side so the UI never has to
 *  re-derive the comparison and disagree with the enforcement path. */
export interface Delegation {
    scope: 'global' | 'account';
    role_id?: number | null;
    account_id?: number | null;
    role_name?: string | null;
    /** Unix ms. */
    expires_at: number;
    /** Negative once lapsed. */
    remaining_ms: number;
    expired: boolean;
}

export const list_delegations = async (userId: number): Promise<Delegation[]> => {
    const response = await axiosInstance.get<{ delegations: Delegation[] }>(
        `api/v1/users/${userId}/delegations`
    );
    return response.data.delegations;
};

/**
 * Put an end date on a role assignment the user already holds.
 *
 * Sends `expires_at` (unix ms) so the deadline is the calendar instant that
 * was approved, not "N days from whenever this request landed". `days` is the
 * convenience path and is converted server-side.
 */
export const delegate_role = async (
    userId: number,
    roleId: number,
    deadline: { expires_at: number } | { days: number }
): Promise<Delegation> => {
    const response = await axiosInstance.put<{ delegation: Delegation }>(
        `api/v1/users/${userId}/delegations`,
        { role_id: roleId, ...deadline }
    );
    return response.data.delegation;
};

/** Remove the end date, leaving the role assigned permanently. */
export const undelegate_role = async (
    userId: number,
    roleId: number
): Promise<void> => {
    await axiosInstance.delete(`api/v1/users/${userId}/delegations/${roleId}`);
};
