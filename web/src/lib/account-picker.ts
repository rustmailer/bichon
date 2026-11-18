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

import type { MinimalAccount } from '@/api/account/api'

export interface AccountPickerOption {
  /** Account id as a string, the value the combobox reports back. */
  value: string
  /** The label the dropdown shows: a custom display name, else the email. */
  label: string
  /** The email, shown as a second line only when a display name hides it. */
  description?: string
}

/** What the user sees, and therefore what the list is sorted by. */
export function accountPickerLabel(account: MinimalAccount): string {
  const name = account.name?.trim()
  return name ? name : account.email
}

/**
 * Builds the options for the mailbox account picker.
 *
 * Sorted A-Z by the visible label, not by email: a named account shown as
 * "Work" belongs under W even though its address starts with something else.
 * The email stays searchable — it is the `description` whenever the label
 * hides it, and the combobox filters on both. Ties fall back to email then id
 * so the order is stable when two accounts share a display name.
 */
export function accountPickerOptions(
  accounts: MinimalAccount[],
  locale?: string
): AccountPickerOption[] {
  const compare = (a: string, b: string) =>
    a.localeCompare(b, locale, { sensitivity: 'base', numeric: true })

  return [...accounts]
    .sort((a, b) => {
      const byLabel = compare(accountPickerLabel(a), accountPickerLabel(b))
      if (byLabel !== 0) return byLabel
      const byEmail = compare(a.email, b.email)
      if (byEmail !== 0) return byEmail
      return a.id - b.id
    })
    .map((account) => {
      const label = accountPickerLabel(account)
      return {
        value: String(account.id),
        label,
        description: label === account.email ? undefined : account.email,
      }
    })
}
