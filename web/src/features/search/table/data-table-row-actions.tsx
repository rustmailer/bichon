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


import { Row } from '@tanstack/react-table'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuShortcut,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { useTranslation } from 'react-i18next'
import { Clock, MoreVertical, TagIcon, Trash2, Upload, Wrench } from 'lucide-react'
import { EmailEnvelope } from '@/api'
import { useSearchContext } from '../context'
import { OP_MESSAGE_DELETE } from '@/api/approvals/api'
import { usePendingApprovals } from '@/features/approvals/use-pending-approvals'
import { useCurrentUser } from '@/hooks/use-current-user'

interface DataTableRowActionsProps {
  row: Row<EmailEnvelope>
}

export function DataTableRowActions({ row }: DataTableRowActionsProps) {
  const { setOpen, setCurrentEnvelope, setSelected, setToDelete, setEditTagsOpen } = useSearchContext()
  const { t } = useTranslation()
  const { require_any_permission } = useCurrentUser()
  // Gate each action by the permission the server demands, on this message's
  // own account: `data:manage` for tags/repair, `data:export:batch` for
  // restore, `data:delete` for deletion.
  const accountId = row.original.account_id
  const canEditTags = require_any_permission(
    ['system:root', 'data:manage', 'data:manage:all'],
    accountId
  )
  const canRestore = require_any_permission(
    ['system:root', 'data:export:batch', 'data:export:batch:all'],
    accountId
  )
  const canDelete = require_any_permission(
    ['system:root', 'data:delete', 'data:delete:all'],
    accountId
  )
  const noPermission = t('common.noPermission', 'No permission')

  // Repair is non-destructive — it only re-fetches and re-attaches missing
  // bytes, hash-verified before the write — so unlike deletion it does not go
  // through the approval flow.
  const canRepair = require_any_permission(
    ['system:root', 'data:manage', 'data:manage:all'],
    row.original.account_id
  )

  // A queued message deletion leaves the message in the list, so the row still
  // offers to delete it. Blocking only *this* message keeps unrelated cleanup
  // possible while an approval is pending.
  const { hasPendingMessage } = usePendingApprovals()
  const deleteQueued = hasPendingMessage(
    OP_MESSAGE_DELETE,
    row.original.account_id,
    row.original.id
  )

  const toggleToDelete = (accountId: number, mailId: string) => {
    setToDelete(prev => {
      const next = new Map(prev)
      const set = new Set(next.get(accountId) || [])

      if (set.has(mailId)) {
        set.delete(mailId)
        if (set.size === 0) next.delete(accountId)
        else next.set(accountId, set)
      } else {
        set.add(mailId)
        next.set(accountId, set)
      }
      return next
    })
  }

  const handleDelete = (envelope: EmailEnvelope) => {
    setToDelete(new Map())
    toggleToDelete(envelope.account_id, envelope.id)
    setOpen("delete")
  }

  return (
    <>
      <DropdownMenu modal={false}>
        <DropdownMenuTrigger asChild>
          <Button
            variant='ghost'
            className='flex h-8 w-8 p-0 data-[state=open]:bg-muted'
          >
            <MoreVertical size={10} />
            <span className='sr-only'>Open menu</span>
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align='end' className='w-[160px]'>
          <DropdownMenuItem
            className='text-xs data-[disabled]:pointer-events-auto'
            disabled={!canEditTags}
            title={!canEditTags ? noPermission : undefined}
            onClick={(e) => {
              e.stopPropagation()
              setCurrentEnvelope(row.original)
              setEditTagsOpen(true)
            }}
          >
            {t('search.editTag')}
            <DropdownMenuShortcut>
              <TagIcon size={16} />
            </DropdownMenuShortcut>
          </DropdownMenuItem>
          <DropdownMenuSeparator />
          <DropdownMenuItem
            className='text-xs data-[disabled]:pointer-events-auto'
            disabled={!canRestore}
            title={!canRestore ? noPermission : undefined}
            onClick={(e) => {
              e.stopPropagation()
              setCurrentEnvelope(row.original)
              setSelected(new Map())
              setOpen("restore")
            }}
          >
            {t('restore_message.restore_to_imap', 'Restore Mail')}
            <DropdownMenuShortcut>
              <Upload size={16} />
            </DropdownMenuShortcut>
          </DropdownMenuItem>
          <>
            <DropdownMenuSeparator />
            <DropdownMenuItem
              className='text-xs'
              disabled={!canRepair}
              title={!canRepair ? noPermission : undefined}
              onClick={(e) => {
                  e.stopPropagation()
                  setCurrentEnvelope(row.original)
                  setSelected(new Map())
                setOpen('repair')
              }}
            >
              {t('repair_message.repairAction', 'Repair message')}
              <DropdownMenuShortcut>
                <Wrench size={16} />
              </DropdownMenuShortcut>
            </DropdownMenuItem>
          </>
          <DropdownMenuSeparator />
          <DropdownMenuItem
            disabled={deleteQueued || !canDelete}
            title={!canDelete ? noPermission : undefined}
            onClick={(e) => {
              e.stopPropagation()
              handleDelete(row.original)
            }}
            className='!text-red-500 text-xs data-[disabled]:pointer-events-auto'
          >
            {t('common.delete')}
            <DropdownMenuShortcut>
              {deleteQueued ? (
                <span className='flex items-center gap-1 text-amber-600 dark:text-amber-500'>
                  <Clock size={16} />
                  {t('approvals.pendingBadge', 'Awaiting approval')}
                </span>
              ) : (
                <Trash2 size={16} />
              )}
            </DropdownMenuShortcut>
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </>
  )
}
