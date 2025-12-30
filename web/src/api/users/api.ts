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

export function getPermissions(t: (key: string) => string) {
    return [
        // 1. Global Management
        { label: t('permission.system.access'), value: 'system:access' },
        { label: t('permission.system.root'), value: 'system:root' },
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
    ]
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
    token: string;
    created_at: number;
    updated_at: number;
    name?: string;
    last_access_at: number;
    expire_at?: number | null;
    token_type: TokenType;
}

export interface User {
    id: number;
    username: string;
    email: string;
    password?: string | null;
    description?: string | null;
    global_roles: number[];
    global_roles_names: string[];
    avatar?: string;
    acl?: AccessControl;
    account_access_map: Record<number, number>;
    account_roles_summary: Record<number, string>;
    global_permissions: string[]
    account_permissions: Record<number, string[]>
    created_at: number;
    updated_at: number;
}

type Theme = 'dark' | 'light'


export interface LoginResult {
    success: boolean;
    error_message?: string | null;
    access_token?: string | null;
    theme?: Theme,
    language?: string,
}


export interface MinimalUser {
    id: number;
    username: string;
    email: string;
}


export const login = async (data: Record<string, any>) => {
    const response = await axiosInstance.post<LoginResult>(`/api/login`, data);
    return response.data;
};

export const reset_admin_token = async () => {
    const response = await axiosInstance.post("/api/v1/reset-admin-token");
    return response.data;
};

export const reset_admin_password = async (password: string) => {
    const response = await axiosInstance.post("/api/v1/reset-admin-password", password, {
        headers: {
            "Content-Type": "text/plain",
        },
    });
    return response.data;
};

export const list_access_tokens = async () => {
    const response = await axiosInstance.get<AccessToken[]>("/api/v1/access-token-list");
    return response.data;
};

export const create_access_token = async (data: Record<string, any>) => {
    const response = await axiosInstance.post("/api/v1/access-token", data);
    return response.data;
}

export const update_access_token = async (token: string, data: Record<string, any>) => {
    const response = await axiosInstance.post(`/api/v1/access-token/${token}`, data);
    return response.data;
}

export const remove_access_token = async (token: string) => {
    const response = await axiosInstance.delete(`/api/v1/access-token/${token}`);
    return response.data;
}


export const list_roles = async () => {
    const response = await axiosInstance.get<UserRole[]>("/api/v1/list-roles");
    return response.data;
};


export const remove_role = async (id: number) => {
    const response = await axiosInstance.delete(`/api/v1/roles/${id}`);
    return response.data;
};


export const create_role = async (data: Record<string, any>) => {
    const response = await axiosInstance.post("/api/v1/roles", data);
    return response.data;
};


export const update_role = async (id: number, data: Record<string, any>) => {
    const response = await axiosInstance.post(`/api/v1/roles/${id}`, data);
    return response.data;
};


export const list_users = async () => {
    const response = await axiosInstance.get<User[]>("/api/v1/list-users");
    return response.data;
};


export const list_minimal_users = async () => {
    const response = await axiosInstance.get<MinimalUser[]>("/api/v1/minimal-user-list");
    return response.data;
};

export const remove_user = async (id: number) => {
    const response = await axiosInstance.delete(`/api/v1/users/${id}`);
    return response.data;
};


export const create_user = async (data: Record<string, any>) => {
    const response = await axiosInstance.post("/api/v1/users", data);
    return response.data;
};


export const update_user = async (id: number, data: Record<string, any>) => {
    const response = await axiosInstance.post(`/api/v1/users/${id}`, data);
    return response.data;
};

export const get_user_tokens = async (id: number) => {
    const response = await axiosInstance.get<AccessToken[]>(`/api/v1/user-tokens/${id}`);
    return response.data;
};

export const get_current_user = async () => {
    const response = await axiosInstance.get<User>("/api/v1/current-user");
    return response.data;
};
