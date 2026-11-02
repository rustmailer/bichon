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
import { AttachmentModel } from '@/api/attachment/api'
import type { DateDisplayMode } from '@/hooks/use-date-display'

export type { DateDisplayMode }

export type AttachmentDialogType =
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
  | 'nested-eml'

interface AttachmentContextType {
  open: AttachmentDialogType | null
  setOpen: (str: AttachmentDialogType | null) => void
  currentAttachment: AttachmentModel | undefined
  setCurrentAttachment: React.Dispatch<
    React.SetStateAction<AttachmentModel | undefined>
  >
  toDelete: Map<number, Set<string>>
  setToDelete: React.Dispatch<React.SetStateAction<Map<number, Set<string>>>>
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
  dateDisplay: DateDisplayMode
  setDateDisplay: (mode: DateDisplayMode) => void
}

const AttachmentContext = React.createContext<AttachmentContextType | null>(
  null
)

interface Props {
  children: React.ReactNode
  value: AttachmentContextType
}

export default function AttachmentProvider({ children, value }: Props) {
  return (
    <AttachmentContext.Provider value={value}>
      {children}
    </AttachmentContext.Provider>
  )
}

export const useAttachmentContext = () => {
  const attachmentContext = React.useContext(AttachmentContext)

  if (!attachmentContext) {
    throw new Error(
      'useAttachmentContext has to be used within <AttachmentContext.Provider>'
    )
  }

  return attachmentContext
}
