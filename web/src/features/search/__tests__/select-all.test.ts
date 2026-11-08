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
import { nextSelectAllAction, type SelectAllAction } from '../select-all'

const action = (input: {
  scopeActive?: boolean
  pageAllSelected?: boolean
}): SelectAllAction =>
  nextSelectAllAction({
    scopeActive: false,
    pageAllSelected: false,
    ...input,
  })

describe('nextSelectAllAction', () => {
  it('selects the page when it is not fully selected', () => {
    expect(action({})).toBe('select-page')
  })

  it('deselects a fully selected page', () => {
    expect(action({ pageAllSelected: true })).toBe('deselect-page')
  })

  it('clears an active scope regardless of page state', () => {
    expect(action({ scopeActive: true, pageAllSelected: true })).toBe('clear')
    expect(action({ scopeActive: true, pageAllSelected: false })).toBe('clear')
  })
})
