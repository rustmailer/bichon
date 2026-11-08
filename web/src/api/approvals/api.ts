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
//
// Dual control (two-person control) API client (Enterprise). A gated operation
// is not executed when the requester asks for it: it is queued, and a second
// person with `approval:decide` must approve it before anything happens.
import axiosInstance from '@/api/axiosInstance'

/** The operations that can be gated. */
export const OP_LEGAL_HOLD_RELEASE = 'legal_hold.release'
export const OP_USER_DELETE = 'user.delete'
export const OP_ACCOUNT_DELETE = 'account.delete'
export const OP_MAILBOX_DELETE = 'mailbox.delete'
export const OP_MESSAGE_DELETE = 'message.delete'
export const OP_TOKEN_REVOKE = 'token.revoke'
export const OP_EXPORT_BATCH = 'export.batch'
export const OP_GDPR_ERASURE = 'gdpr.erasure'

/**
 * A request's lifecycle.
 *
 * `executing` is only observable while the process that started the operation
 * is still running it — a background delete can take minutes. `executed` is
 * the terminal success state. `interrupted` is the terminal *unknown* state: a
 * restart killed the operation part-way through and the target may be
 * partially modified. It is deliberately not folded into a failure state,
 * because "it failed" reads as "nothing happened", which would be false.
 */
export type ApprovalStatus =
  | 'pending'
  | 'approved'
  | 'executing'
  | 'executed'
  | 'rejected'
  | 'expired'
  | 'interrupted'

/** What a request acts on. Discriminated by `kind`.
 *
 *  `account_ids` alone could not express a user, a mailbox, or a set of
 *  messages, and is empty for three of the five operations — so the queue
 *  renders *this*, not that. */
export type ApprovalTarget =
  | { kind: 'accounts'; account_ids: number[] }
  | { kind: 'users'; user_ids: number[] }
  | {
      kind: 'mailbox'
      account_id: number
      mailbox_id: number
      name: string
      /** Approximate, including subfolders; captured when the request was made. */
      messages: number
    }
  | {
      kind: 'messages'
      /** Inline set; empty when the ids were externalized to a detail file. */
      by_account: Record<string, string[]>
      messages: number
      /** Accounts covered, present even when `by_account` is empty. */
      account_ids?: number[]
      /** The filter a `batch-delete` request was selected by. */
      filter?: Record<string, unknown>
      /** Detail-file reference; the ids themselves are not in the ledger. */
      detail_file?: string
      detail_sha256?: string
      detail_bytes?: number
    }
  | {
      kind: 'token'
      user_id: number
      username: string
      /** A truncated SHA-256 of the token, never the token itself — the ledger
       *  is read by approvers, exported as evidence and forwarded to a SIEM, and
       *  a live credential must not reach any of those. */
      fingerprint: string
    }
  | {
      kind: 'export'
      saved_search_id: string
      saved_search_name: string
      /** `mbox`. A string, not an enum: the ledger stays readable to whoever
       *  opens the raw row. */
      format: string
      /** The accounts the requester could export from when the request was
       *  made. `null` means all accounts (the unscoped permission). Frozen at
       *  request time, so a role change while it waits cannot widen what the
       *  approval covers. */
      account_ids: number[] | null
      /** Approximate, from the export preview the requester saw. */
      messages: number
      size_bytes: number
    }
  | {
      kind: 'erasure'
      /** The data subject the request is about, as the operator recorded it. */
      subject: string
      /** How the affected messages were identified. */
      criteria: string
      /** The operator's written reason for believing no statutory retention
       *  obligation applies. Required at submission — the approver is being
       *  asked to sign a deletion, and this is what they are signing. */
      assessment: string
      /** The frozen message set. The erasure deletes exactly what was
       *  assessed, not whatever matches the criteria later. */
      by_account: Record<string, string[]>
      messages: number
    }

/** One approval request. `result` is only present once an approval executed. */
export interface ApprovalRequest {
  id: string
  op: string
  account_ids: number[]
  target: ApprovalTarget
  reason?: string | null
  requested_by: number
  requested_by_name: string
  requested_at: number
  status: ApprovalStatus
  decided_by?: number | null
  decided_by_name?: string | null
  decided_at?: number | null
  decision_note?: string | null
  expires_at?: number | null
  executed_at?: number | null
  /** The per-item outcome. `ReleaseItem[]` for a hold release, otherwise
   *  `ApprovalOutcomeItem[]`; both are discriminated unions, so callers should
   *  narrow on the element shape rather than assume one. */
  result?: ApprovalResultItem[] | null
  /** `Some(hours)` when this decision was taken through the break-glass
   *  override — one person, after the request had waited `hours`. `null` for
   *  the normal path.
   *
   *  The queue must show this rather than letting the request look ordinary:
   *  an override means the second signature was waived, and a reader cannot
   *  infer that from the user fields alone. */
  break_glass_pending_hours?: number | null
}

/** Per-account outcome of a hold release. Has no `kind` discriminator — it is
 *  the wire format the first slice established and is compared byte for byte
 *  by a server-side test. */
export interface BatchReleaseItem {
  account_id: number
  email: string
  ok: boolean
  error?: string | null
}

/** Per-item outcome of the operations added after legal-hold release. */
export type ApprovalOutcomeItem =
  | { kind: 'user'; user_id: number; username: string; ok: boolean; error?: string | null }
  | { kind: 'account'; account_id: number; email: string; ok: boolean; error?: string | null }
  | { kind: 'mailbox'; account_id: number; mailbox_id: number; ok: boolean; error?: string | null }
  | {
      kind: 'messages'
      account_id: number
      requested: number
      deleted: number
      ok: boolean
      error?: string | null
    }
  | { kind: 'token'; user_id: number; username: string; ok: boolean; error?: string | null }
  | {
      kind: 'export'
      /** Empty when the export never started — there is no job to name. */
      export_id: string
      saved_search_name: string
      ok: boolean
      error?: string | null
    }
  | {
      kind: 'erasure'
      /** The data subject, so the exported evidence reads as a person rather
       *  than as an account number. */
      subject: string
      account_id: number
      requested: number
      deleted: number
      ok: boolean
      error?: string | null
    }

/** Either shape, since a request's op decides which one it produced. */
export type ApprovalResultItem = BatchReleaseItem | ApprovalOutcomeItem

/** True when an item is from the general (tagged) result shape. */
export function is_outcome_item(
  item: ApprovalResultItem
): item is ApprovalOutcomeItem {
  return typeof (item as ApprovalOutcomeItem).kind === 'string'
}

/** A request that is still waiting on a decision. */
export const is_pending = (s: ApprovalStatus): boolean => s === 'pending'

/** A request whose execution has not finished — and whose outcome is
 *  therefore unknown. The queue must show these as in-progress rather than
 *  letting an approver believe their approval completed the work. */
export const is_in_flight = (s: ApprovalStatus): boolean =>
  s === 'executing' || s === 'approved'

/** The sentinel for "break-glass is switched off entirely". */
export const BREAK_GLASS_DISABLED = -1

/** The policy. `available_ops` lists the operations the server can execute;
 *  `ops` is the subset an administrator has chosen to gate. */
export interface ApprovalConfig {
  enabled: boolean
  expires_after_hours: number
  retention_days: number
  ops: string[]
  available_ops: string[]
  /** Minimum hours a request must sit pending before break-glass applies, or
   *  `-1` when the override is disabled. Already resolved by the server — this
   *  is the wait that is actually enforced, not the raw stored value. */
  break_glass_min_hours: number
  updated_at: number
}

export interface ApprovalUpdate {
  enabled?: boolean
  expires_after_hours?: number
  retention_days?: number
  ops?: string[]
  /** Minimum wait before break-glass, or `-1` to disable it. `0` means the
   *  server default. */
  break_glass_min_hours?: number
}

/** The queue, newest first. `status` filters server-side. */
export async function list_approvals(
  status?: ApprovalStatus
): Promise<ApprovalRequest[]> {
  const { data } = await axiosInstance.get<ApprovalRequest[]>(
    'api/v1/approvals',
    { params: status ? { status } : undefined }
  )
  return data
}

export async function get_approval(id: string): Promise<ApprovalRequest> {
  const { data } = await axiosInstance.get<ApprovalRequest>(
    `api/v1/approvals/${encodeURIComponent(id)}`
  )
  return data
}

/** The outcome of submitting a GDPR erasure request.
 *
 *  `pending` is the discriminator, exactly as on the gated community
 *  endpoints: dual control queued the request and nothing has been erased.
 *  With the policy off the server runs the erasure on this one signature and
 *  answers `ok: true` with the per-account result. */
export interface ErasureSubmitResult {
  ok: boolean
  pending: boolean
  request_id?: string
  expires_at?: number | null
  result?: ApprovalOutcomeItem[] | null
}

/** Record a GDPR Art. 17 erasure request.
 *
 *  `messages` maps account id to the message ids the operator selected. The
 *  server freezes that set: the erasure covers exactly what was assessed, not
 *  whatever matches the criteria later. */
export async function create_erasure_request(payload: {
  subject: string
  criteria: string
  assessment: string
  messages: Record<number, string[]>
}): Promise<ErasureSubmitResult> {
  const { data } = await axiosInstance.post<ErasureSubmitResult>(
    'api/v1/approvals/erasure-requests',
    payload
  )
  return data
}

/** Approve a request.
 *
 *  For an inline operation the server has already executed it, so the returned
 *  request carries `executed_at` and a `result`. For a background one the
 *  request comes back as `executing` with **no** `result` — the caller must not
 *  read the absence as "nothing was affected", only as "not known yet". */
export async function approve_request(
  id: string,
  note?: string
): Promise<ApprovalRequest> {
  const { data } = await axiosInstance.post<ApprovalRequest>(
    `api/v1/approvals/${encodeURIComponent(id)}/approve`,
    { note: note ?? null }
  )
  return data
}

/** Reject a request. A note is required by the server. */
export async function reject_request(
  id: string,
  note: string
): Promise<ApprovalRequest> {
  const { data } = await axiosInstance.post<ApprovalRequest>(
    `api/v1/approvals/${encodeURIComponent(id)}/reject`,
    { note }
  )
  return data
}

/** Approve a request alone, through the break-glass override.
 *
 *  Only reachable by a `system:root` administrator, and only once the request
 *  has been pending longer than `break_glass_min_hours` — the server measures
 *  the wait itself and answers 403 if it has not elapsed, so a caller cannot
 *  shortcut it by passing a duration. There is deliberately no reject
 *  counterpart: a lone rejection is not a blocked operation.
 *
 *  The returned request carries `break_glass_pending_hours`, which the queue
 *  should surface — the second signature was waived and the record should say
 *  so. */
export async function break_glass_request(
  id: string,
  note?: string
): Promise<ApprovalRequest> {
  const { data } = await axiosInstance.post<ApprovalRequest>(
    `api/v1/approvals/${encodeURIComponent(id)}/break-glass`,
    { note: note ?? null }
  )
  return data
}

/** Account ids that already have a pending *hold release* (the server filters
 *  by op), so the legal-hold console can badge those rows instead of letting an
 *  operator queue them twice. */
export async function list_pending_account_ids(): Promise<number[]> {
  const { data } = await axiosInstance.get<{ account_ids: number[] }>(
    'api/v1/approvals/pending-accounts'
  )
  return data.account_ids
}

export async function get_approval_config(): Promise<ApprovalConfig> {
  const { data } = await axiosInstance.get<ApprovalConfig>(
    'api/v1/approvals/config'
  )
  return data
}

export async function update_approval_config(
  payload: ApprovalUpdate
): Promise<ApprovalConfig> {
  const { data } = await axiosInstance.put<ApprovalConfig>(
    'api/v1/approvals/config',
    payload
  )
  return data
}
