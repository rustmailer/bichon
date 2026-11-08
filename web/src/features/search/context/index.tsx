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
import React from 'react'
import { SortingState } from '@tanstack/react-table'
import { EmailEnvelope } from '@/api'
import type { DateDisplayMode } from '@/hooks/use-date-display'

export type SearchDialogType =
  | 'mailbox'
  | 'display'
  | 'delete'
  | 'filters'
  | 'tags'
  | 'edit-tags'
  | 'update-tags'
  | 'restore'
  | 'repair'
  | 'delete-mailbox'
  /// The GDPR Art. 17 erasure request form. Its own dialog rather than a mode
  /// of the delete dialog: the two ask for different things (a typed
  /// confirmation versus a recorded assessment) and one of them leaves a
  /// record a supervisory authority can read.
  | 'gdpr-erasure'

export type { DateDisplayMode }

/// "Select all results matching this filter", rather than only the current
/// page. `filterJson` is the snapshot that produced `total`; if the live filter
/// changes while the scope is active the scope is dropped, so a stale count can
/// never become a deletion of a different result set.
export interface SelectAllScope {
  filterJson: string
  total: number
}

interface SearchContextType {
  open: SearchDialogType | null
  setOpen: (str: SearchDialogType | null) => void
  currentEnvelope: EmailEnvelope | undefined
  setCurrentEnvelope: React.Dispatch<
    React.SetStateAction<EmailEnvelope | undefined>
  >
  toDelete: Map<number, Set<string>>
  setToDelete: React.Dispatch<React.SetStateAction<Map<number, Set<string>>>>
  selected: Map<number, Set<string>>
  setSelected: React.Dispatch<React.SetStateAction<Map<number, Set<string>>>>
  selectAllScope: SelectAllScope | null
  setSelectAllScope: React.Dispatch<React.SetStateAction<SelectAllScope | null>>
  deleteMailboxId: string | undefined
  setDeleteMailboxId: React.Dispatch<React.SetStateAction<string | undefined>>
  selectedAccountId: number | undefined
  setSelectedAccountId: React.Dispatch<React.SetStateAction<number | undefined>>
  selectedTags: string[]
  sorting: SortingState
  setSorting: React.Dispatch<React.SetStateAction<SortingState>>
  filter: Record<string, any>
  setFilter: React.Dispatch<React.SetStateAction<Record<string, any>>>
  refetch: () => Promise<unknown>
  isFetching: boolean
  /// Effective sort order actually applied by the backend (RELEVANCE when a
  /// text term is present and no explicit sort was chosen).
  effectiveSort: 'DATE' | 'SIZE' | 'RELEVANCE'
  handleTagToggle: (tag: string) => void
  editTagsOpen: boolean
  setEditTagsOpen: (open: boolean) => void
  /// Kept out of `SearchDialogType` on purpose: that union is driven by
  /// `useDialogState`, whose setter toggles, so the history drawer would
  /// become mutually exclusive with the dialogs instead of layering over them.
  dateDisplay: DateDisplayMode
  setDateDisplay: (mode: DateDisplayMode) => void
}

const SearchContext = React.createContext<SearchContextType | null>(null)

interface Props {
  children: React.ReactNode
  value: SearchContextType
}

export default function SearchProvider({ children, value }: Props) {
  return (
    <SearchContext.Provider value={value}>{children}</SearchContext.Provider>
  )
}

export const useSearchContext = () => {
  const searchContext = React.useContext(SearchContext)

  if (!searchContext) {
    throw new Error(
      'useSearchContext has to be used within <SearchContext.Provider>'
    )
  }

  return searchContext
}
