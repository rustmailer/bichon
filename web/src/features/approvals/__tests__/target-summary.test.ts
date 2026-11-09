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
import {
  accountMessageCounts,
  filterClauses,
  formatFilterTimestamp,
} from '../target-summary'
import type { MessageTarget } from '../target-summary'

function target(overrides: Partial<MessageTarget>): MessageTarget {
  return {
    kind: 'messages',
    by_account: {},
    messages: 0,
    ...overrides,
  }
}

describe('accountMessageCounts', () => {
  it('uses the frozen per-account counts and sorts by account', () => {
    const counts = accountMessageCounts(
      target({
        account_counts: [
          { account_id: 4, count: 30_000 },
          { account_id: 3, count: 70_000 },
        ],
      }),
    )
    expect(counts).toEqual([
      { account_id: 3, count: 70_000 },
      { account_id: 4, count: 30_000 },
    ])
  })

  it('falls back to the inline id set for rows written before the counts existed', () => {
    const counts = accountMessageCounts(
      target({
        by_account: { '2': ['a', 'b'], '1': ['c'] },
      }),
    )
    expect(counts).toEqual([
      { account_id: 1, count: 1 },
      { account_id: 2, count: 2 },
    ])
  })

  it('prefers externalized counts over an empty inline set', () => {
    // The detail-file path never carries ids; it must still show the split.
    const counts = accountMessageCounts(
      target({
        by_account: {},
        account_counts: [{ account_id: 9, count: 12 }],
      }),
    )
    expect(counts).toEqual([{ account_id: 9, count: 12 }])
  })

  it('returns nothing when neither shape is present', () => {
    expect(accountMessageCounts(target({}))).toEqual([])
  })
})

describe('formatFilterTimestamp', () => {
  it('formats milliseconds as a date', () => {
    expect(formatFilterTimestamp(Date.UTC(2026, 0, 15))).toBe('2026-01-15')
  })

  it('accepts seconds without rendering 1970', () => {
    expect(formatFilterTimestamp(Date.UTC(2026, 0, 15) / 1000)).toBe(
      '2026-01-15',
    )
  })
})

describe('filterClauses', () => {
  it('keeps the fields that narrow the selection, in reading order', () => {
    const clauses = filterClauses({
      text: 'newsletter',
      from: 'news@example.com',
      tags: ['finance', 'q4'],
      has_attachment: true,
      since: Date.UTC(2026, 0, 15),
    })
    expect(clauses).toEqual([
      { field: 'text', value: 'newsletter' },
      { field: 'from', value: 'news@example.com' },
      { field: 'has_attachment', value: 'true' },
      { field: 'tags', value: 'finance, q4' },
      { field: 'since', value: '2026-01-15' },
    ])
  })

  it('drops empty values, nulls and fields it does not know', () => {
    expect(
      filterClauses({
        text: '',
        subject: null,
        from: 'a@b',
        account_ids: [],
        match_all: true,
        something_else: 'ignored',
      }),
    ).toEqual([{ field: 'from', value: 'a@b' }])
  })

  it('treats a missing or non-object filter as empty', () => {
    expect(filterClauses(undefined)).toEqual([])
    expect(filterClauses(null)).toEqual([])
    expect(filterClauses('text: x')).toEqual([])
  })
})
