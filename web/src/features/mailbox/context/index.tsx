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
import { MailboxData } from '@/api/mailbox/api'
import { EmailEnvelope } from '@/api'

export type MailboxDialogType = 'mailbox' | 'display' | 'move-to-trash' | 'filters' | 'restore'

interface MailboxContextType {
  open: MailboxDialogType | null
  setOpen: (str: MailboxDialogType | null) => void
  selectedAccountId: number | undefined
  currentMailbox: MailboxData | undefined
  currentEnvelope: EmailEnvelope | undefined
  setCurrentMailbox: React.Dispatch<React.SetStateAction<MailboxData | undefined>>
  setCurrentEnvelope: React.Dispatch<React.SetStateAction<EmailEnvelope | undefined>>
  deleteIds: Set<number>
  setDeleteIds: React.Dispatch<React.SetStateAction<Set<number>>>
  selected: Set<number>
  setSelected: React.Dispatch<React.SetStateAction<Set<number>>>
}

const MailboxContext = React.createContext<MailboxContextType | null>(null)

interface Props {
  children: React.ReactNode
  value: MailboxContextType
}

export default function MailboxProvider({ children, value }: Props) {
  return <MailboxContext.Provider value={value}>{children}</MailboxContext.Provider>
}

export const useMailboxContext = () => {
  const mailboxContext = React.useContext(MailboxContext)

  if (!mailboxContext) {
    throw new Error(
      'useMailboxContext has to be used within <MailboxContext.Provider>'
    )
  }

  return mailboxContext
}
