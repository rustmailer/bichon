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

import { describe, expect, it } from 'vitest'
import { describeEvent } from '../describe-event'
import type { AuditRecord } from '@/api/audit/api'

function record(
  event_type: string,
  payload: Record<string, unknown> = {},
  overrides: Partial<AuditRecord> = {},
): AuditRecord {
  return {
    id: 'id',
    seq: 1,
    ts_ms: 0,
    event_type,
    user: 'admin',
    account_id: null,
    mailbox_id: null,
    email_id: null,
    content_hash: null,
    ip: null,
    payload,
    prev_hash: null,
    ...overrides,
  }
}

describe('describeEvent', () => {
  it('summarizes a bulk deletion by its message count', () => {
    expect(describeEvent(record('email.batch_deleted', { count: 500 }))).toBe(
      '500 email(s)',
    )
  })

  it('leaves a bulk deletion without a count blank rather than inventing one', () => {
    expect(describeEvent(record('email.batch_deleted', {}))).toBe('')
  })

  it('keeps the single-deletion summary on the subject', () => {
    expect(
      describeEvent(record('email.deleted', { subject: 'Quarterly report' })),
    ).toBe('Quarterly report')
  })

  it('falls back to the email id when a deletion has no subject', () => {
    expect(
      describeEvent(record('email.deleted', {}, { email_id: 'env-1' })),
    ).toBe('env-1')
  })

  it('returns nothing for event types it has no summary for', () => {
    expect(describeEvent(record('user.login', { ip: '10.0.0.1' }))).toBe('')
  })
})
