//
// Copyright (c) 2025 rustmailer.com (https://rustmailer.com)
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
import { EmailEnvelope } from '@/api'

export type SearchDialogType = 'mailbox' | 'display' | 'delete' | 'filters' | 'tags' | 'edit-tags' | 'search-form' | 'restore'

interface SearchContextType {
  open: SearchDialogType | null
  setOpen: (str: SearchDialogType | null) => void
  currentEnvelope: EmailEnvelope | undefined
  setCurrentEnvelope: React.Dispatch<React.SetStateAction<EmailEnvelope | undefined>>
  toDelete: Map<number, Set<number>>
  setToDelete: React.Dispatch<React.SetStateAction<Map<number, Set<number>>>>
  selected: Map<number, Set<number>>
  setSelected: React.Dispatch<React.SetStateAction<Map<number, Set<number>>>>
  selectedTags: string[]
}

const SearchContext = React.createContext<SearchContextType | null>(null)

interface Props {
  children: React.ReactNode
  value: SearchContextType
}

export default function SearchProvider({ children, value }: Props) {
  return <SearchContext.Provider value={value}>{children}</SearchContext.Provider>
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
