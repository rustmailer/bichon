//
// Copyright (c) 2025-2026 rustmailer.com (https://rustmailer.com)
//
// This file is part of the Bichon Email Archiving Project
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as
// published by the Free Software Foundation, either version 3 of the
// License, or (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <http://www.gnu.org/licenses/>.
//
// Cloud provider connections (Pro/Enterprise). A connection holds the
// tenant-level credentials shared by the mailboxes imported from it; the
// server never returns the secret, only `credential_set`.
import axiosInstance from '@/api/axiosInstance'

export type ProviderKind = 'Graph' | 'Gmail'

/** Connection health as of the last probe (server vocabulary). */
export type ConnectionStatus =
  | 'untested'
  | 'ok'
  | 'auth_required'
  | 'misconfigured'
  | 'throttled'
  | 'disabled'

/** Derived account status (server vocabulary). */
export type ProviderAccountStatus =
  | 'idle'
  | 'syncing'
  | 'gap_fill'
  | 'paused'
  | 'needs_reauth'
  | 'license_blocked'
  | 'disabled'
  | 'error'
  | 'interrupted'

export interface ProviderEndpoints {
  authority: string
  token_url: string
  graph_base: string
  scope: string
}

export interface ProviderConnectionView {
  id: number
  provider: ProviderKind
  cloud: string
  name: string
  credential_set: boolean
  credential_version: number
  tenant_id?: string | null
  admin_email?: string | null
  use_proxy?: boolean | null
  endpoints?: ProviderEndpoints | null
  status: ConnectionStatus | string
  last_ok_at?: number | null
  last_error_kind?: string | null
  last_error_at?: number | null
  created_at: number
  updated_at: number
  account_count: number
}

export interface ProviderPreset {
  id: string
  provider: ProviderKind
  display_name: string
  authority_host: string
  graph_base: string
  default_scope: string
  admin_consent_path: string
  docs_url?: string | null
  /** Built in and hand-checked; `false` means "not verified against a live tenant". */
  verified: boolean
  /** Added by the operator in `provider-presets.json`. */
  custom: boolean
}

export interface ProviderCredentialsInput {
  client_id?: string | null
  client_secret?: string | null
  service_account_json?: string | null
}

export interface ProviderConnectionCreate {
  provider: ProviderKind
  cloud: string
  name: string
  credentials: ProviderCredentialsInput
  tenant_id?: string | null
  admin_email?: string | null
}

export interface ProviderConnectionUpdate {
  cloud?: string
  name?: string
  credentials?: ProviderCredentialsInput
  tenant_id?: string
  admin_email?: string
}

export interface ProviderErrorInfo {
  provider: ProviderKind
  kind: string
  disposition: unknown
  status?: number | null
  code?: string | null
  message?: string | null
  request_id?: string | null
  hint?: string | null
}

export interface ConnectionTestOutcome {
  provider: ProviderKind
  ok: boolean
  token_obtained: boolean
  probe_ok: boolean
  latency_ms: number
  error?: ProviderErrorInfo | null
}

export interface DiscoveryCandidate {
  email: string
  display_name?: string | null
  aliases: string[]
  mailbox_type: string
  enabled: boolean
  suspended: boolean
  archived: boolean
  archivable: boolean
  reason?: string | null
  already_imported: boolean
}

export interface DiscoveryView {
  provider: ProviderKind
  fetched_at: number
  stale: boolean
  total_seen: number
  filtered_out: number
  candidates: DiscoveryCandidate[]
}

export type ImportStatus =
  | 'created'
  | 'already_exists'
  | 'not_archivable'
  | 'not_discovered'
  | 'license_refused'
  | 'failed'

export interface ImportResultItem {
  email: string
  status: ImportStatus
  account_id?: number | null
  detail?: string | null
}

export interface ImportOutcome {
  connection_id: number
  provider: ProviderKind
  requested: number
  created: number
  skipped: number
  failed: number
  discovery_stale: boolean
  results: ImportResultItem[]
}

export interface ProviderAccountStatusView {
  account_id: number
  email: string
  account_enabled: boolean
  connection_id?: number | null
  status: ProviderAccountStatus | string
  /** English fallback; prefer the translated `reason_code` when present. */
  reason?: string | null
  /** Stable code (`account_disabled`, `paused`, ...) for `providers.reasons.*`. */
  reason_code?: string | null
  paused: boolean
  paused_at?: number | null
  paused_by?: string | null
  phase?: string | null
  folders_total?: number | null
  folders_done: number
  last_pass_started_at?: number | null
  last_pass_finished_at?: number | null
  last_ok_at?: number | null
  last_error_kind?: string | null
  last_error_at?: number | null
  last_pass_archived: number
  last_pass_failed: number
  last_pass_suppressed: number
  connection_status?: string | null
}

export const list_connections = async (): Promise<ProviderConnectionView[]> => {
  const { data } = await axiosInstance.get<ProviderConnectionView[]>(
    'api/v1/provider-connections'
  )
  return data
}

export const list_presets = async (): Promise<ProviderPreset[]> => {
  const { data } = await axiosInstance.get<ProviderPreset[]>(
    'api/v1/provider-presets'
  )
  return data
}

export const create_connection = async (
  request: ProviderConnectionCreate
): Promise<ProviderConnectionView> => {
  const { data } = await axiosInstance.post<ProviderConnectionView>(
    'api/v1/provider-connections',
    request
  )
  return data
}

export const update_connection = async (
  id: number,
  request: ProviderConnectionUpdate
): Promise<ProviderConnectionView> => {
  const { data } = await axiosInstance.patch<ProviderConnectionView>(
    `api/v1/provider-connections/${id}`,
    request
  )
  return data
}

export const delete_connection = async (id: number): Promise<void> => {
  await axiosInstance.delete(`api/v1/provider-connections/${id}`)
}

export const test_connection = async (
  id: number
): Promise<ConnectionTestOutcome> => {
  const { data } = await axiosInstance.post<ConnectionTestOutcome>(
    `api/v1/provider-connections/${id}/test`
  )
  return data
}

export const discover_mailboxes = async (id: number): Promise<DiscoveryView> => {
  const { data } = await axiosInstance.post<DiscoveryView>(
    `api/v1/provider-connections/${id}/discover`
  )
  return data
}

/** `undefined` when discovery has not run since the connection last changed. */
export const get_candidates = async (
  id: number
): Promise<DiscoveryView | undefined> => {
  try {
    const { data } = await axiosInstance.get<DiscoveryView>(
      `api/v1/provider-connections/${id}/candidates`
    )
    return data
  } catch (error) {
    const status = (error as { response?: { status?: number } })?.response
      ?.status
    if (status === 404) return undefined
    throw error
  }
}

export const import_accounts = async (
  connection_id: number,
  emails: string[]
): Promise<ImportOutcome> => {
  const { data } = await axiosInstance.post<ImportOutcome>(
    `api/v1/provider-connections/${connection_id}/accounts`,
    { emails }
  )
  return data
}

export const list_account_status = async (
  connection_id: number
): Promise<ProviderAccountStatusView[]> => {
  const { data } = await axiosInstance.get<ProviderAccountStatusView[]>(
    `api/v1/provider-connections/${connection_id}/account-status`
  )
  return data
}

export const pause_account = async (
  account_id: number
): Promise<ProviderAccountStatusView> => {
  const { data } = await axiosInstance.post<ProviderAccountStatusView>(
    `api/v1/provider-accounts/${account_id}/pause`
  )
  return data
}

export const resume_account = async (
  account_id: number
): Promise<ProviderAccountStatusView> => {
  const { data } = await axiosInstance.post<ProviderAccountStatusView>(
    `api/v1/provider-accounts/${account_id}/resume`
  )
  return data
}

export const start_gap_fill = async (
  account_id: number
): Promise<{ account_id: number; started: boolean }> => {
  const { data } = await axiosInstance.post<{
    account_id: number
    started: boolean
  }>(`api/v1/provider-accounts/${account_id}/gap-fill`)
  return data
}
