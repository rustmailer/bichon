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

import type { ApprovalTarget } from '@/api/approvals/api'

/** The `messages` variant of an approval target. */
export type MessageTarget = Extract<ApprovalTarget, { kind: 'messages' }>

export interface AccountCount {
  account_id: number
  count: number
}

/**
 * Per-account frozen counts for the approver's card.
 *
 * New rows carry `account_counts` directly; rows written before that field
 * existed (and the inline path) only have the ids, which answer the same
 * question. Sorted by account so two renders of one request never disagree.
 */
export function accountMessageCounts(target: MessageTarget): AccountCount[] {
  const fromInline = Object.entries(target.by_account ?? {}).map(
    ([account_id, ids]) => ({
      account_id: Number(account_id),
      count: ids.length,
    }),
  )
  const source =
    target.account_counts && target.account_counts.length > 0
      ? target.account_counts
      : fromInline
  return [...source].sort((a, b) => a.account_id - b.account_id)
}

export interface FilterClause {
  field: string
  value: string
}

/** Fields shown in the approver's filter summary, in reading order. */
const FILTER_FIELDS = [
  'text',
  'subject',
  'body',
  'from',
  'to',
  'cc',
  'bcc',
  'any_participant',
  'any_recipient',
  'has_attachment',
  'attachment_name',
  'attachment_extension',
  'attachment_category',
  'attachment_content_type',
  'tags',
  'has_tags',
  'min_size',
  'max_size',
  'since',
  'before',
  'ingest_since',
  'ingest_before',
  'internal_date_since',
  'internal_date_before',
  'account_ids',
  'mailbox_ids',
  'message_id',
  'id',
] as const

const DATE_FIELDS = new Set([
  'since',
  'before',
  'ingest_since',
  'ingest_before',
  'internal_date_since',
  'internal_date_before',
])

/**
 * Formats a timestamp as a date. The API sends milliseconds; a value small
 * enough to be seconds is accepted too, so an older or hand-built request
 * cannot render as 1970.
 */
export function formatFilterTimestamp(value: number): string {
  const ms = Math.abs(value) < 100_000_000_000 ? value * 1000 : value
  const date = new Date(ms)
  if (Number.isNaN(date.getTime())) return String(value)
  return date.toISOString().slice(0, 10)
}

function formatFilterValue(field: string, value: unknown): string | null {
  if (value === null || value === undefined) return null
  if (DATE_FIELDS.has(field) && typeof value === 'number') {
    return formatFilterTimestamp(value)
  }
  if (Array.isArray(value)) {
    if (value.length === 0) return null
    return value.join(', ')
  }
  if (typeof value === 'string') {
    return value === '' ? null : value
  }
  if (typeof value === 'boolean' || typeof value === 'number') {
    return String(value)
  }
  return null
}

/**
 * The filter a batch-delete request was selected by, as `field: value`
 * clauses. Empty and unknown fields are dropped — the approver should see
 * what actually narrowed the selection, not a block of `null`s.
 */
export function filterClauses(filter: unknown): FilterClause[] {
  if (!filter || typeof filter !== 'object') return []
  const record = filter as Record<string, unknown>
  const clauses: FilterClause[] = []
  for (const field of FILTER_FIELDS) {
    const value = formatFilterValue(field, record[field])
    if (value !== null) clauses.push({ field, value })
  }
  return clauses
}
