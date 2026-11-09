//
// Copyright (c) 2025-2026 rustmailer.com (https://rustmailer.com)
//
// Audit log API client (Pro edition).
//

import axiosInstance from '@/api/axiosInstance'

export interface AuditRecord {
  id: string
  seq: number
  ts_ms: number
  event_type: string
  user: string
  account_id: number | null
  mailbox_id: number | null
  email_id: string | null
  content_hash: string | null
  ip: string | null
  payload: Record<string, unknown>
  prev_hash: string | null
}

export interface AuditPageResponse {
  items: AuditRecord[]
  total: number
  page: number
  page_size: number
}

export interface AuditQueryParams {
  page?: number
  page_size?: number
  start_ms?: number
  end_ms?: number
  user?: string
  event_type?: string
  account_id?: number
  email_id?: string
  /** Restricts to the events a single deletion job produced. */
  job_id?: string
}

export async function list_audit_log(
  params: AuditQueryParams,
): Promise<AuditPageResponse> {
  const { data } = await axiosInstance.get<AuditPageResponse>('api/v1/audit-log', {
    params,
  })
  return data
}

export async function list_audit_log_by_email(
  envelopeId: string,
  page: number,
  page_size: number,
): Promise<AuditPageResponse> {
  const { data } = await axiosInstance.get<AuditPageResponse>(
    `api/v1/audit-log/email/${envelopeId}`,
    { params: { page, page_size } },
  )
  return data
}
