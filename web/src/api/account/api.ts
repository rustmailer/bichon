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
import { PaginatedResponse } from "..";

export interface MinimalAccount {
    id: number;
    email: string;
    name?: string;
    /** IMAP authentication method; `null`/absent for accounts with no IMAP config (e.g. NoSync). */
    auth_type?: AuthType | null;
    /** Storage type: `View` is a read-only filtered view and holds no mail of
     *  its own; `NoSync` with a `kind` is the journal archive. */
    account_type: AccountType;
    /** Special role (Pro/Enterprise): present only on the journal archive. */
    kind?: 'journal';
}

export const minimal_account_list = async () => {
    const response = await axiosInstance.get<MinimalAccount[]>("api/v1/minimal-account-list");
    return response.data;
};


export enum DownloadStatus {
    Running = "Running",
    Success = "Success",
    Failed = "Failed",
    Cancelled = "Cancelled",
}

export enum TriggerType {
    Manual = "Manual",
    Scheduled = "Scheduled",
    SyncFull = "SyncFull",
}

export enum FolderStatus {
    Pending = "Pending",
    Downloading = "Downloading",
    Success = "Success",
    Failed = "Failed",
    Cancelled = "Cancelled",
}

export interface FolderProgress {
    folder_name: string;
    planned: number;
    current: number;
    filtered: number;
    status: FolderStatus;
    message: string | null;
}


export interface AccountError {
    at: number;
    error: string;
}

export interface GapFillFolderStats {
    downloaded: number;
    failed: number;
    filtered: number;
    candidate_count: number;
    message?: string | null;
}

export enum GapFillStatus {
    Running = "Running",
    Success = "Success",
    Failed = "Failed",
    Cancelled = "Cancelled",
}

export interface GapFillRun {
    started_at: number;
    finished_at: number | null;
    status: GapFillStatus;
    folders: Record<string, GapFillFolderStats>;
    downloaded: number;
    failed: number;
    filtered: number;
}

export interface GapFillState {
    account_id: number;
    active: GapFillRun | null;
    history: GapFillRun[];
}

export interface DownloadSession {
    start_time: number;
    end_time: number | null;
    status: DownloadStatus;
    message: string | null;
    trigger: TriggerType;
    folder_details: Record<string, FolderProgress>;
    current_folder: string | null;
    errors: AccountError[];
}

export interface DownloadState {
    account_id: number;
    active_session: DownloadSession | null;
    history: DownloadSession[];
    last_trigger_at: number;
    last_finished_at: number | null;
}



type Encryption = 'Ssl' | 'StartTls' | 'None';
type AuthType = 'Password' | 'OAuth2';
type Unit = 'Days' | 'Months' | 'Years';
type AccountType = 'IMAP' | 'NoSync' | 'View';

// Interface definitions
interface AuthConfig {
    auth_type: AuthType;
    password?: string;
}

export interface ImapConfig {
    host: string;
    port: number; // integer, 0-65535
    encryption: Encryption;
    auth: AuthConfig;
    use_proxy?: number;
}

interface RelativeDate {
    unit: Unit;
    value: number; // integer, minimum 1
}

interface DateSelection {
    fixed?: string; // format: "YYYY-MM-DD"
    relative?: RelativeDate;
}


export type QuotaWindow = 'hourly' | 'daily' | 'weekly' | 'monthly'

export interface FilterRule {
    include: string[];
    exclude: string[];
}

export interface ArchiveRules {
    enabled: boolean;
    senders: FilterRule;
    subjects: FilterRule;
    skip_larger_than?: number;
    spam_headers: string[];
}

export interface ExtractionRules {
    enabled: boolean;
    extensions: FilterRule;
    folders: FilterRule;
    attachment_names: FilterRule;
    senders: FilterRule;
}

export interface AccountModel {
    id: number;
    account_type: AccountType;
    imap?: ImapConfig;
    enabled: boolean;
    login_name?: string,
    account_name?: string,
    email: string;
    capabilities?: string[];
    date_since?: DateSelection;
    date_before?: RelativeDate;
    download_folders: string[];
    download_interval_min?: number;
    download_batch_size?: number;
    max_email_size_bytes?: number;
    created_by: number;
    created_user_name: string;
    created_user_email: string;
    created_at: number;
    updated_at: number;
    use_dangerous: boolean;
    pgp_key?: string;
    imap_quota_window?: QuotaWindow;
    imap_quota_bytes?: number;
    auto_download_new_mailboxes?: boolean;
    download_schedule?: string;
    archive_rules?: ArchiveRules;
    extraction_rules?: ExtractionRules;
    /** Ordered headers that may replace the stored `from` address. */
    override_from_headers?: string[];
    deleting?: boolean;
    /** Retention window in days (0 / undefined = keep everything). */
    retention_days?: number;
    /** Source-mailbox cleanup configuration (community). */
    remote_cleanup?: RemoteCleanupConfig | null;
    /** Legal hold flag (Enterprise). When true the retention sweep skips this account. */
    legal_hold?: boolean;
    hold_reason?: string | null;
    hold_placed_by?: number | null;
    hold_placed_at?: number | null;
    /** Special role (Pro/Enterprise). `null`/absent = ordinary account. */
    kind?: AccountKind | null;
    /** Sources and per-source filters of a `View` account. */
    view?: ViewConfig | null;
}

/** A view's restriction on one source account; every condition is ANDed with
 *  the viewer's own search. */
export interface ViewSource {
    account_id: number;
    /** OR within the field: at least one of these addresses must appear. */
    any_participant?: string[] | null;
    /** OR within the field: at least one of these addresses must appear. */
    any_recipient?: string[] | null;
}

export interface AccountKind {
    /** The only special role: the SMTP journaling archive. */
    role: 'journal';
    target_address?: string | null;
}

export interface ViewConfig {
    sources: ViewSource[];
}

/** Configuration for moving archived mail into bichon_auto_created_trash. */
export interface RemoteCleanupConfig {
    enabled?: boolean;
    schedule?: string | null;
    folders?: string[] | null;
    eligible_after_archive_days?: number;
    min_message_age_days?: number | null;
    max_messages_per_run?: number;
    skip_flagged?: boolean;
}

export enum RemoteCleanupStatus {
    Running = 'Running',
    Success = 'Success',
    Cancelled = 'Cancelled',
    Blocked = 'Blocked',
    Failed = 'Failed',
}

export enum RemoteCleanupTrigger {
    Scheduled = 'Scheduled',
    Manual = 'Manual',
}

/** One message a cleanup run did not move, named so it can be identified. */
export interface RemoteCleanupSkippedMessage {
    folder: string;
    uid: number;
    message_id: string;
    subject?: string | null;
    /** Stable reason key, the same one counted in skipped_reasons. */
    reason: string;
    /** Counted as a failure rather than a policy skip. */
    failed: boolean;
    size: number;
}

export interface RemoteCleanupFolderStats {
    candidate_count: number;
    verified: number;
    moved: number;
    skipped: number;
    failed: number;
    bytes_moved: number;
    message?: string | null;
    /** Why messages were skipped/stopped in this folder. */
    skipped_reasons?: Record<string, number>;
    /**
     * Which messages those counters refer to, capped. `null`/absent means the
     * report predates this field — not that nothing was skipped.
     */
    unprocessed?: RemoteCleanupSkippedMessage[] | null;
}

export interface RemoteCleanupRun {
    run_id: string;
    started_at: number;
    finished_at: number | null;
    trigger: RemoteCleanupTrigger;
    user?: string | null;
    status: RemoteCleanupStatus;
    destination_folder: string;
    folders: Record<string, RemoteCleanupFolderStats>;
    moved: number;
    skipped: number;
    failed: number;
    bytes_moved: number;
    message?: string | null;
}

export interface RemoteCleanupState {
    account_id: number;
    active: RemoteCleanupRun | null;
    history: RemoteCleanupRun[];
    last_finished_at?: number | null;
}

export interface CleanupSkipCount {
    reason: string;
    count: number;
}

export interface CleanupFolderPreview {
    folder: string;
    candidates: number;
    verified: number;
    bytes: number;
    skipped: CleanupSkipCount[];
}

export interface CleanupPreview {
    account_id: number;
    kind: 'move';
    generated_at: number;
    expires_at: number;
    destination_folder: string;
    destination_exists: boolean;
    /** Whether the server advertises MOVE (RFC 6851); the only move path. */
    supports_move: boolean;
    candidates: number;
    verified: number;
    bytes: number;
    folders: CleanupFolderPreview[];
    skipped: CleanupSkipCount[];
    warnings: string[];
    /** Bichon backup state at preview time. */
    backup_configured: boolean;
    backup_last_success_at: number | null;
    /** A successful backup finished within the last seven days. */
    backup_is_recent: boolean;
    /** A move execution will stop at the first identity mismatch. */
    will_abort: boolean;
    token: string;
    /**
     * Set by the server when the job is read: the confirmation token is past
     * `expires_at`, so Execute will be refused and a new preview is required.
     * Always false in the response that just made the preview.
     */
    token_expired?: boolean;
}

export const download_state = async (account_id: number) => {
    const response = await axiosInstance.get<DownloadState>(`api/v1/accounts/${account_id}/download-stats`);
    return response.data;
};

export const gap_fill_state = async (account_id: number) => {
    const response = await axiosInstance.get<GapFillState>(`api/v1/accounts/${account_id}/gap-fill-stats`);
    return response.data;
};

export interface CleanupJob {
    job_id: string;
    account_id: number;
    kind: CleanupJobKind;
    status: CleanupJobStatus;
    user?: string | null;
    created_at: number;
    finished_at?: number | null;
    /** Set when a preview job succeeded. */
    preview?: CleanupPreview | null;
    /** Set when a move job succeeded (the run itself may still have failed). */
    run?: RemoteCleanupRun | null;
    error?: string | null;
}

export type CleanupJobKind = 'move_preview' | 'move_run';

export enum CleanupJobStatus {
    Running = 'Running',
    Success = 'Success',
    Failed = 'Failed',
    Cancelled = 'Cancelled',
}

export const remote_cleanup_state = async (account_id: number) => {
    const response = await axiosInstance.get<RemoteCleanupState>(`api/v1/accounts/${account_id}/remote-cleanup/state`);
    return response.data;
};

/**
 * Cleanup previews and moves run in the background: the request
 * only starts the job and returns it immediately, so no HTTP deadline can kill
 * a run (strict verification re-downloads every candidate). Poll
 * `remote_cleanup_job` until `status` leaves `Running`.
 */
export const preview_remote_cleanup = async (account_id: number) => {
    const response = await axiosInstance.post<CleanupJob>(
        `api/v1/accounts/${account_id}/remote-cleanup/preview`,
        undefined
    );
    return response.data;
};

export const execute_remote_cleanup = async (account_id: number, preview_token: string) => {
    const response = await axiosInstance.post<CleanupJob>(`api/v1/accounts/${account_id}/remote-cleanup/execute`, {
        preview_token,
        confirm: true,
        acknowledge_risk: true,
    });
    return response.data;
};

/**
 * Jobs the server still knows for this account, newest first with running ones
 * ahead of finished ones. Jobs are in memory and expire 30 minutes after they
 * finish; an empty list therefore does not mean nothing ever ran.
 *
 * Used to reattach after the client lost its job id (page reload, dialog
 * reopened): the server refuses a second job for the same account while one
 * is running.
 */
export const list_remote_cleanup_jobs = async (account_id: number) => {
    const response = await axiosInstance.get<CleanupJob[]>(
        `api/v1/accounts/${account_id}/remote-cleanup/jobs`
    );
    return response.data;
};

export const remote_cleanup_job = async (account_id: number, job_id: string) => {
    const response = await axiosInstance.get<CleanupJob>(
        `api/v1/accounts/${account_id}/remote-cleanup/jobs/${job_id}`
    );
    return response.data;
};

export const cancel_remote_cleanup_job = async (account_id: number, job_id: string) => {
    const response = await axiosInstance.post<CleanupJob>(
        `api/v1/accounts/${account_id}/remote-cleanup/jobs/${job_id}/cancel`,
        undefined
    );
    return response.data;
};

export const create_account = async (data: Record<string, any>) => {
    const response = await axiosInstance.post("api/v1/account", data);
    return response.data;
};

export const list_accounts = async () => {
    const response = await axiosInstance.get<PaginatedResponse<AccountModel>>("api/v1/accounts?desc=true");
    return response.data;
};

export const update_account = async (account_id: number, data: Record<string, any>) => {
    const response = await axiosInstance.post(`api/v1/account/${account_id}`, data);
    return response.data;
};

export const remove_account = async (account_id: number) => {
    const response = await axiosInstance.delete(`api/v1/account/${account_id}`);
    return response.data;
};


export const start_account_download = async (account_id: number, run_gap_fill = false) => {
    const response = await axiosInstance.post(`api/v1/accounts/${account_id}/start-download`, { run_gap_fill });
    return response.data;
};

export const cancel_account_download = async (account_id: number) => {
    const response = await axiosInstance.post(`api/v1/accounts/${account_id}/cancel-download`);
    return response.data;
};

export interface AutoConfigResult {
    imap: ServerConfig;
    oauth2?: OAuth2Config;
}

export interface ServerConfig {
    host: string;
    port: number;
    encryption: 'None' | 'Ssl' | 'StartTls';
}

export interface OAuth2Config {
    issuer: string;
    scope: string;
    auth_url: string;
    token_url: string;
}

export const autoconfig = async (email: string) => {
    const response = await axiosInstance.get<AutoConfigResult>(`api/v1/autoconfig/${email}`);
    return response.data;
};

export const access_assign = async (data: Record<string, any>) => {
    const response = await axiosInstance.post("api/v1/accounts/access/assignments", data);
    return response.data;
};