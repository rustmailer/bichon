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

/** What a click on the results-table header checkbox should do. */
export type SelectAllAction =
  /** Drop the "all matching results" scope and clear the selection. */
  | 'clear'
  /** Every row on this page is selected: deselect the page. */
  | 'deselect-page'
  /** Select every row on the current page. */
  | 'select-page'

/**
 * Pure decision for the header checkbox, so the off-by-one cases (a single
 * page of results, selections left over from other pages, an active scope)
 * are testable without rendering the table.
 *
 * The checkbox only ever acts on the page. "Select all matching results" is a
 * separate, visible action in the banner shown once the page is selected —
 * making the second click do something different was a hidden state machine
 * nobody could discover.
 */
export function nextSelectAllAction(input: {
  scopeActive: boolean
  pageAllSelected: boolean
}): SelectAllAction {
  if (input.scopeActive) return 'clear'
  if (input.pageAllSelected) return 'deselect-page'
  return 'select-page'
}
