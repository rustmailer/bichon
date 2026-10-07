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
import axiosInstance from '@/api/axiosInstance'

export interface Release {
  tag_name: string
  published_at: string
  body: string
  html_url: string
}

export interface ReleaseNotification {
  latest: Release | null // `latest` can be null if the release information is not available
  is_newer: boolean
  error_message: string | null // New field to store error message when the request fails
}

export interface SystemAlert {
  kind: string // discriminator for label/icon, e.g. "backup"
  severity: string // "warning" | "error"
  title: string
  message: string
}

interface Notifications {
  release: ReleaseNotification
  alerts: SystemAlert[]
}

export const get_notifications = async () => {
  const response =
    await axiosInstance.get<Notifications>(`api/v1/notifications`)
  return response.data
}

export interface DashboardStats {
  account_count: number // Number of accounts
  email_count: number // Total number of emails
  attachment_count: number // Total number of attachments
  total_size_bytes: number // Total size of all emails (in bytes)
  storage_usage_bytes: number // Actual storage used (in bytes)
  index_usage_bytes: number // Index storage size (in bytes)
  recent_activity: TimeBucket[] // Email activity over recent days
  top_senders: Group[] // Top 10 senders
  top_accounts: Group[] // Top 10 senders
  with_attachment_count: number // Emails with attachments
  without_attachment_count: number // Emails without attachments
  top_largest_emails: LargestEmail[] // Top 10 largest emails
  top_largest_attachments: LargestAttachment[] // Top 10 largest attachments
  system_version: string //The semantic version string of the currently running backend service
}

export const INITIAL_DASHBOARD_STATS: DashboardStats = {
  account_count: 0,
  email_count: 0,
  attachment_count: 0,
  total_size_bytes: 0,
  storage_usage_bytes: 0,
  index_usage_bytes: 0,
  recent_activity: [],
  top_senders: [],
  top_accounts: [],
  with_attachment_count: 0,
  without_attachment_count: 0,
  top_largest_emails: [],
  top_largest_attachments: [],
  system_version: '0.0.0',
}

export interface TimeBucket {
  timestamp_ms: number // Timestamp in milliseconds
  count: number // Number of emails in this time bucket
}

export interface Group {
  key: string // Sender email or name
  count: number // Number of emails from this sender
  // Set for top_accounts only: the account the bucket was grouped by. The
  // account email (key) is not unique across accounts, so resolve names and
  // search targets through this id when present.
  account_id?: number
}

export interface LargestEmail {
  subject: string // Email subject
  size_bytes: number // Email size in bytes
  id: string
}

export interface LargestAttachment {
  name: string // Attachment name
  size_bytes: number // Email size in bytes
  id: string // attachment id
}

export interface Proxy {
  id: number
  url: string
  created_at: number
  updated_at: number
}

export interface ProxyTestResult {
  ip?: string | null
  country?: string | null
  region?: string | null
  city?: string | null
  isp?: string | null
}

export type ServerConfigurations = {
  bichon_log_level: string
  bichon_http_port: number
  bichon_bind_ip?: string | null
  bichon_base_url: string
  bichon_public_url: string
  bichon_enable_rest_https: boolean
  bichon_http_compression_enabled: boolean

  bichon_cors_origins?: string[] | null
  bichon_cors_max_age: number

  bichon_ansi_logs: boolean
  bichon_log_to_file: boolean
  bichon_json_logs: boolean
  bichon_max_server_log_files: number

  bichon_encrypt_password_set: boolean
  bichon_webui_token_expiration_hours: number

  bichon_root_dir: string
  bichon_index_dir?: string | null
  bichon_data_dir?: string | null
  bichon_metadata_cache_size: number
  bichon_envelope_cache_size: number

  bichon_sync_concurrency?: number | null
  bichon_enable_smtp: boolean
  bichon_smtp_port: number
  bichon_smtp_encryption: 'none' | 'starttls' | 'tls'
  bichon_smtp_auth_required: boolean
  bichon_smtp_tls_key_path?: string | null
  bichon_smtp_tls_cert_path?: string | null

  bichon_upload_body_limit_mb: number
  bichon_web_mbox_upload_limit_mb: number
  bichon_web_pst_upload_limit_mb: number
}

export const get_dashboard_stats = async () => {
  const response = await axiosInstance.get<DashboardStats>(
    `api/v1/dashboard-stats`
  )
  return response.data
}

export const list_proxy = async () => {
  const response = await axiosInstance.get<Proxy[]>(`api/v1/list-proxy`)
  return response.data
}

export const delete_proxy = async (id: number) => {
  const response = await axiosInstance.delete(`api/v1/proxy/${id}`)
  return response.data
}

export const update_proxy = async (id: number, url: string) => {
  const response = await axiosInstance.post(`api/v1/proxy/${id}`, url, {
    headers: {
      'Content-Type': 'text/plain',
    },
  })
  return response.data
}

export const test_proxy = async (id: number) => {
  const response = await axiosInstance.post<ProxyTestResult>(
    `api/v1/proxy/${id}/test`
  )
  return response.data
}

export const add_proxy = async (url: string) => {
  const response = await axiosInstance.post(`api/v1/proxy`, url, {
    headers: {
      'Content-Type': 'text/plain',
    },
  })
  return response.data
}

export const get_system_configurations = async () => {
  const response = await axiosInstance.get<ServerConfigurations>(
    `api/v1/system-configurations`
  )
  return response.data
}
