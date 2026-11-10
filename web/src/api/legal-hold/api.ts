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
// Legal Hold API client (Enterprise). Accounts under a legal hold are frozen:
// the retention sweep skips them and bulk deletion refuses to purge them.
import axiosInstance from '@/api/axiosInstance'

/** Account summary as returned by the legal-hold console endpoints. */
export interface HoldAccount {
  /** Case id. A legacy account-level hold also has one. */
  hold_id?: string
  id: number
  email: string
  name?: string | null
  /** Operator case/matter reference, e.g. "2026-014". */
  case_ref?: string | null
  reason?: string | null
  placed_by?: number | null
  placed_by_name?: string | null
  placed_at?: number | null
}

export async function list_legal_holds(): Promise<HoldAccount[]> {
  const { data } = await axiosInstance.get<HoldAccount[]>('api/v1/legal-hold')
  return data
}

export async function place_legal_hold(
  account_id: number,
  reason: string,
  case_ref?: string
): Promise<HoldAccount> {
  const { data } = await axiosInstance.post<HoldAccount>(
    `api/v1/legal-hold/${account_id}`,
    { reason, case_ref: case_ref || null }
  )
  return data
}

/**
 * Result of releasing one hold.
 *
 * When dual control is on the release is not executed: the server answers
 * `202` with `status: 'pending'` and a `request_id`, and the account stays on
 * hold until someone else approves it. `ok` stays `false` in that case so a
 * caller cannot read a queued request as a lifted hold.
 */
export interface ReleaseHoldResult {
  ok: boolean
  account_id: number
  /** Present on a case-level release. */
  hold_id?: string
  status?: 'released' | 'pending'
  pending?: boolean
  request_id?: string
  expires_at?: number | null
}

export async function release_legal_hold(
  account_id: number,
  reason?: string
): Promise<ReleaseHoldResult> {
  // Always send a JSON body ({ reason: null } when none) — a bodyless DELETE
  // used to hit the backend's required Json extractor and come back 415
  // Unsupported Media Type without a readable message.
  const { data } = await axiosInstance.delete<ReleaseHoldResult>(
    `api/v1/legal-hold/${account_id}`,
    {
      data: { reason: reason ?? null },
    }
  )
  return data
}

/**
 * Release exactly one hold case, leaving any other active case on the same
 * account in place.
 *
 * The per-row Release action uses this; the batch action stays account-level
 * and closes every active case for the selected accounts.
 */
export async function release_legal_hold_case(
  hold_id: string,
  reason?: string
): Promise<ReleaseHoldResult> {
  const { data } = await axiosInstance.delete<ReleaseHoldResult>(
    `api/v1/legal-hold/cases/${encodeURIComponent(hold_id)}`,
    {
      data: { reason: reason ?? null },
    }
  )
  return data
}

/**
 * Per-account outcome of a batch hold operation.
 *
 * `pending` and `request_id` are only present on a release that was queued for
 * approval; with dual control off the server omits them entirely.
 */
export interface BatchHoldResult {
  account_id: number
  email: string
  ok: boolean
  error?: string | null
  pending?: boolean
  request_id?: string
}

/** Place a hold on multiple accounts at once with a shared reason. */
export async function place_legal_holds_batch(
  account_ids: number[],
  reason: string,
  case_ref?: string
): Promise<BatchHoldResult[]> {
  const { data } = await axiosInstance.post<BatchHoldResult[]>(
    'api/v1/legal-hold/batch',
    { account_ids, reason, case_ref: case_ref || null }
  )
  return data
}

/** Release holds on multiple accounts at once (reason optional). */
export async function release_legal_holds_batch(
  account_ids: number[],
  reason?: string
): Promise<BatchHoldResult[]> {
  const { data } = await axiosInstance.delete<BatchHoldResult[]>(
    'api/v1/legal-hold/batch',
    { data: { account_ids, reason: reason ?? null } }
  )
  return data
}
