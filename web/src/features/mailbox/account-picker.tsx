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

import * as React from 'react'
import { useTranslation } from 'react-i18next'
import type { MinimalAccount } from '@/api/account/api'
import { VirtualizedSelect } from '@/components/virtualized-select'
import { accountPickerOptions } from '@/lib/account-picker'

interface Props {
  accounts: MinimalAccount[]
  value?: number
  onChange: (accountId: number) => void
  className?: string
}

/**
 * Searchable account selector for the mailbox view.
 *
 * A plain Select is the wrong control once there are more than a handful of
 * accounts: finding one means scanning a list in insertion order. This wraps
 * the app's searchable combobox (the same one the audit log and OAuth2 dialogs
 * use) with the account-specific mapping — sorted A-Z by the visible label,
 * with the email kept as a second line and as a search key when a display name
 * hides it.
 *
 * The list height is capped by the popover's available space in the viewport,
 * so a long account list scrolls inside the dropdown instead of running off the
 * bottom of the screen.
 */
export function AccountPicker({ accounts, value, onChange, className }: Props) {
  const { t, i18n } = useTranslation()
  const options = React.useMemo(
    () => accountPickerOptions(accounts, i18n.language),
    [accounts, i18n.language]
  )
  // Named accounts get the email as a second line, which needs a taller row.
  const itemHeight = options.some((option) => option.description) ? 48 : 35
  // Fit the list to its content, but never past the popover's available space:
  // a long account list scrolls inside the dropdown instead of running off the
  // bottom of the screen. The 8px is CommandGroup's `p-1` (4px top and bottom)
  // and the 1px guards against sub-pixel rounding; without them the content is
  // a little taller than the box and the scrollbar appears even for a
  // three-account list.
  const contentHeight = Math.max(options.length, 1) * itemHeight + 9
  const height = `min(${contentHeight}px, clamp(8rem, calc(var(--radix-popover-content-available-height, 320px) - 3rem), 20rem))`

  return (
    <VirtualizedSelect
      options={options}
      value={value !== undefined ? String(value) : undefined}
      onSelectOption={(selected) => {
        const next = selected[0]
        if (next !== undefined) onChange(Number(next))
      }}
      placeholder={t('mailboxView.selectAccount')}
      searchPlaceholder={t('mailboxView.searchAccounts', 'Search accounts...')}
      isLoading={false}
      className={className}
      itemHeight={itemHeight}
      height={height}
      scrollArea
      inputClassName='text-xs'
      itemClassName='text-xs'
      noItemsComponent={
        <div className='py-2 text-center text-xs text-muted-foreground'>
          {t('mailboxView.noAccountsFound', 'No matching accounts.')}
        </div>
      }
    />
  )
}
