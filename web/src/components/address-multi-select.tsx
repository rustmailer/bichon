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
// A multi-select for e-mail addresses, sourced from the archive's contacts.
//
// One shared implementation rather than one per feature: the contact source
// (`/all-contacts`, cached by `useContacts`), the selected-on-top ordering, the
// chip overflow (`first two + n`) and the free-text add are behaviour every
// caller should get identically. The only per-caller input is the placeholder.
//
// Selected addresses are shown **first** in the list — including ones that are
// not in the contact list (free text) or that the current search would filter
// out — so removing a choice never requires clearing the search box first.
import { useMemo, useState } from 'react'
import { Check, Plus, X } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { useContacts } from '@/hooks/use-contacts'
import type { Contact } from '@/api/search/api'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandSeparator,
} from '@/components/ui/command'
import { ScrollArea } from '@/components/ui/scroll-area'

/** How many chips the trigger shows before collapsing into `+n`. */
const VISIBLE_CHIPS = 2
/** How many unselected contacts the popover lists at most. */
const MAX_CANDIDATES = 100

export interface AddressMultiSelectProps {
  value: string[]
  onChange: (next: string[]) => void
  placeholder: string
  /** Read-only: the caller may see the selection but not change it. */
  disabled?: boolean
}

export function AddressMultiSelect({
  value,
  onChange,
  placeholder,
  disabled = false,
}: AddressMultiSelectProps) {
  const { t } = useTranslation()
  const [search, setSearch] = useState('')
  // The full list once; filtering happens locally so that addresses selected
  // but not matching the current search still render (they stay at the top).
  const { contacts, isLoading, isError } = useContacts('')

  // Display names are only known for senders, and only in the contact list —
  // a free-text address has none.
  const nameByEmail = useMemo(() => {
    const map = new Map<string, string>()
    for (const contact of contacts) {
      if (contact.name) map.set(contact.email.toLowerCase(), contact.name)
    }
    return map
  }, [contacts])
  const displayName = (email: string) =>
    nameByEmail.get(email.toLowerCase()) ?? email.split('@')[0]

  const toggle = (email: string) => {
    const address = email.trim()
    if (!address) return
    if (value.some((v) => v.toLowerCase() === address.toLowerCase())) {
      onChange(value.filter((v) => v.toLowerCase() !== address.toLowerCase()))
    } else {
      onChange([...value, address])
    }
  }

  const typed = search.trim()
  const lower = typed.toLowerCase()
  // Address or display name: users look for "Alice", not always the address.
  const matchesSearch = (contact: Contact) =>
    !lower ||
    contact.email.toLowerCase().includes(lower) ||
    (contact.name ?? '').toLowerCase().includes(lower)

  const isSelected = (email: string) =>
    value.some((v) => v.toLowerCase() === email.toLowerCase())

  // Selected first — including free-text addresses that are not in the
  // contact list — then the unselected contacts matching the search.
  const selectedRows = value
  const candidateRows = contacts
    .filter((contact) => !isSelected(contact.email) && matchesSearch(contact))
    .slice(0, MAX_CANDIDATES)

  const canAddTyped =
    typed.includes('@') &&
    !isSelected(typed) &&
    !contacts.some((c) => c.email.toLowerCase() === lower)

  // A field with ten addresses would otherwise grow its row without bound.
  const visibleChips = value.slice(0, VISIBLE_CHIPS)
  const hiddenCount = value.length - visibleChips.length

  return (
    <Popover>
      <PopoverTrigger asChild>
        <button
          type='button'
          disabled={disabled}
          className='flex min-h-9 w-full flex-wrap items-center gap-1 rounded-md border border-input bg-transparent px-2 py-1 text-left text-xs shadow-sm focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50'
        >
          {value.length === 0 ? (
            <span className='text-xs text-muted-foreground'>{placeholder}</span>
          ) : (
            visibleChips.map((address) => (
              <span
                key={address}
                className='inline-flex items-center gap-1 rounded bg-muted px-1.5 py-0.5 text-xs'
              >
                {address}
                <span
                  role='button'
                  tabIndex={-1}
                  aria-label={t('common.remove', 'Remove')}
                  onClick={(e) => {
                    e.stopPropagation()
                    if (!disabled) toggle(address)
                  }}
                  className='cursor-pointer text-muted-foreground hover:text-destructive'
                >
                  <X className='h-3 w-3' />
                </span>
              </span>
            ))
          )}
          {hiddenCount > 0 && (
            <span className='inline-flex items-center rounded bg-primary/10 px-1.5 py-0.5 text-xs font-medium text-primary'>
              +{hiddenCount}
            </span>
          )}
        </button>
      </PopoverTrigger>
      <PopoverContent align='start' className='w-[320px] p-0'>
        <Command shouldFilter={false}>
          <CommandInput
            value={search}
            onValueChange={setSearch}
            placeholder={t(
              'address_select.searchPlaceholder',
              'Search contacts or type an address'
            )}
            className='h-9'
          />
          <ScrollArea className='h-[280px]'>
            {isLoading && (
              <div className='p-4 text-center text-xs opacity-50'>
                {t('search_contacts.loading')}
              </div>
            )}
            {isError && (
              <div className='p-4 text-center text-xs text-destructive'>
                {t('address_select.loadFailed', 'Could not load contacts.')}
              </div>
            )}
            {!isError && (
              <CommandEmpty>{t('search_contacts.no_contact_found')}</CommandEmpty>
            )}
            <CommandGroup>
              {canAddTyped && (
                <CommandItem
                  value={`__add_${typed}`}
                  onSelect={() => {
                    toggle(typed)
                    setSearch('')
                  }}
                  className='text-xs'
                >
                  <Plus className='mr-2 h-3.5 w-3.5' />
                  {t('address_select.add', 'Add "{{address}}"', {
                    address: typed,
                  })}
                </CommandItem>
              )}
              {selectedRows.map((email) => (
                <CommandItem
                  key={`selected-${email}`}
                  value={`selected-${email}`}
                  onSelect={() => toggle(email)}
                  className='flex items-center justify-between gap-3 py-2 text-xs'
                >
                  <div className='flex min-w-0 flex-col'>
                    <span className='font-medium'>{displayName(email)}</span>
                    <span className='max-w-[220px] truncate text-xs text-muted-foreground'>
                      {email}
                    </span>
                  </div>
                  <Check className='h-4 w-4 shrink-0 text-primary' />
                </CommandItem>
              ))}
              {selectedRows.length > 0 && candidateRows.length > 0 && (
                <CommandSeparator />
              )}
              {candidateRows.map((contact) => (
                <CommandItem
                  key={contact.email}
                  value={contact.email}
                  onSelect={() => toggle(contact.email)}
                  className='flex items-center justify-between gap-3 py-2 text-xs'
                >
                  <div className='flex min-w-0 flex-col'>
                    <span className='font-medium'>{displayName(contact.email)}</span>
                    <span className='max-w-[220px] truncate text-xs text-muted-foreground'>
                      {contact.email}
                    </span>
                  </div>
                </CommandItem>
              ))}
              {contacts.length > MAX_CANDIDATES && (
                <div className='border-t border-border/50 px-3 py-2 text-center text-xs text-muted-foreground'>
                  {t('search_contacts.showing_limit', { total: contacts.length })}
                </div>
              )}
            </CommandGroup>
          </ScrollArea>
        </Command>
      </PopoverContent>
    </Popover>
  )
}
