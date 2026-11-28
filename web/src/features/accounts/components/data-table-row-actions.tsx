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


import { useState } from 'react'
import { DotsHorizontalIcon } from '@radix-ui/react-icons'
import { Row } from '@tanstack/react-table'
import { IconClock, IconEdit, IconPlayerPlay, IconPlayerStop, IconShieldLock, IconTrash } from '@tabler/icons-react'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuShortcut,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { useAccountContext } from '../context'
import { Eraser, History, Mailbox, MessageSquareMore, Play, Settings } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { useCurrentUser } from '@/hooks/use-current-user'
import { AccountModel, cancel_account_download } from '@/api/account/api'
import { toast } from '@/hooks/use-toast'
import { errorMessage } from '@/lib/error'
import { useNavigate } from '@tanstack/react-router'
import { StartDownloadDialog } from './start-download-dialog'
import { CleanupConfigDialog } from './cleanup-config-dialog'
import { CleanupReportsDialog } from './cleanup-reports-dialog'
import { CleanupRunDialog } from './cleanup-run-dialog'
import { OP_ACCOUNT_DELETE } from '@/api/approvals/api'
import { usePendingApprovals } from '@/features/approvals/use-pending-approvals'

interface DataTableRowActionsProps {
  row: Row<AccountModel>
}

export function DataTableRowActions({ row }: DataTableRowActionsProps) {
  const { t } = useTranslation()
  const { setOpen, setCurrentRow } = useAccountContext()
  const navigate = useNavigate()
  const [startDialogOpen, setStartDialogOpen] = useState(false)
  const [cleanupOpen, setCleanupOpen] = useState(false)
  const [cleanupRunOpen, setCleanupRunOpen] = useState(false)
  const [cleanupReportsOpen, setCleanupReportsOpen] = useState(false)

  const account_type = row.original.account_type;
  const { require_any_permission } = useCurrentUser()

  // A queued deletion leaves the account exactly as it is, so without this the
  // row looks untouched and the delete is offered again. Asking twice would
  // queue two requests for one account.
  const { hasPendingAccount } = usePendingApprovals()
  const deleteQueued = hasPendingAccount(OP_ACCOUNT_DELETE, row.original.id)

  const hasPermission = require_any_permission(['system:root', 'account:manage'], row.original.id);
  const hasReadPermission = require_any_permission(['system:root', 'account:read_details'], row.original.id);

  const isDeleting = row.original.deleting === true;

  // Editing a view is gated by management of the *view account* itself, which
  // `hasPermission` already checks. Individual sources the operator does not
  // manage are grayed out inside the dialog instead of blocking the entry.

  // Keep the menu reachable for IMAP rows even without permission so the
  // cleanup actions can be shown disabled instead of disappearing.
  const canShowAnyAction =
    !isDeleting &&
    (hasPermission || hasReadPermission || account_type === 'IMAP');

  const showDownload = !isDeleting && account_type === 'IMAP' && hasPermission;

  const handleStartDownload = async () => {
    setStartDialogOpen(true)
  }


  const handleCancelDownload = async () => {
    try {
      await cancel_account_download(row.original.id);
      toast({ title: t('accounts.downloadCancelled') });
    } catch (error) {
      toast({
        variant: "destructive",
        title: t('accounts.cancelFailed'),
        description: errorMessage(error)
      });
    }
  }

  return (
    <>
      <DropdownMenu modal={false}>
        <DropdownMenuTrigger asChild disabled={!canShowAnyAction}>
          <Button
            variant='ghost'
            className='flex h-8 w-8 p-0 data-[state=open]:bg-muted'
          >
            <DotsHorizontalIcon className='h-4 w-4' />
            <span className='sr-only'>{t('accounts.openMenu')}</span>
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align='end' className='w-[220px]'>
          {hasPermission && account_type !== 'Graph' && account_type !== 'Gmail' && <DropdownMenuItem
            onClick={() => {
              if (account_type === "IMAP") {
                navigate({ to: '/accounts/$id/settings', params: { id: String(row.original.id) } });
              } else if (account_type === "View") {
                setCurrentRow(row.original)
                setOpen("edit-view");
              } else {
                setCurrentRow(row.original)
                setOpen("edit-nosync");
              }
            }}
            className='text-xs'
          >
            {t('accounts.edit')}
            <DropdownMenuShortcut>
              <IconEdit size={16} />
            </DropdownMenuShortcut>
          </DropdownMenuItem>}
          {account_type === "IMAP" && hasPermission && <DropdownMenuItem className='text-xs'
            onClick={() => {
              navigate({ to: '/accounts/$id/settings', params: { id: String(row.original.id) } });
            }}
          >
            {t('accounts.settings.settings')}
            <DropdownMenuShortcut>
              <Settings size={16} />
            </DropdownMenuShortcut>
          </DropdownMenuItem>}
          {account_type === "IMAP" && hasPermission && <DropdownMenuItem
            className='text-xs'
            onClick={() => {
              setCurrentRow(row.original)
              setOpen('sync-folders')
            }}
          >
            {t('accounts.selectMailboxes')}
            <DropdownMenuShortcut>
              <Mailbox size={16} />
            </DropdownMenuShortcut>
          </DropdownMenuItem>}
          {account_type === "IMAP" && hasReadPermission && <DropdownMenuItem
            className='text-xs'
            onClick={() => {
              setCurrentRow(row.original)
              setOpen('detail')
            }}
          >
            {t('accounts.detail')}
            <DropdownMenuShortcut>
              <MessageSquareMore size={16} />
            </DropdownMenuShortcut>
          </DropdownMenuItem>}
          {hasPermission && <DropdownMenuSeparator />}
          {hasPermission && <DropdownMenuItem
            className='text-xs'
            onClick={() => {
              setCurrentRow(row.original)
              setOpen('access-assign')
            }}
          >
            <span>{t('accounts.accessControl')}</span>
            <DropdownMenuShortcut>
              <IconShieldLock size={16} />
            </DropdownMenuShortcut>
          </DropdownMenuItem>}
          {hasPermission && <DropdownMenuSeparator />}

          {showDownload && (
            <DropdownMenuItem onClick={handleStartDownload} className='text-xs'>
              {t('accounts.startDownload')}
              <DropdownMenuShortcut>
                <IconPlayerPlay size={16} />
              </DropdownMenuShortcut>
            </DropdownMenuItem>
          )}


          {showDownload && (
            <DropdownMenuItem onClick={handleCancelDownload} className='text-xs'>
              {t('accounts.cancelDownload')}
              <DropdownMenuShortcut>
                <IconPlayerStop size={16} />
              </DropdownMenuShortcut>
            </DropdownMenuItem>
          )}
          {showDownload && <DropdownMenuSeparator />}

          {account_type === 'IMAP' && (
            <DropdownMenuItem
              className='text-xs'
              disabled={!hasPermission}
              title={!hasPermission ? t('common.noPermission', 'No permission') : undefined}
              onClick={() => setCleanupOpen(true)}
            >
              {t('accounts.cleanupConfigAction', 'Source cleanup')}
              <DropdownMenuShortcut>
                <Eraser size={16} />
              </DropdownMenuShortcut>
            </DropdownMenuItem>
          )}

          {account_type === 'IMAP' && (
            <DropdownMenuItem
              className='text-xs'
              disabled={!hasPermission || !row.original.remote_cleanup}
              title={
                !hasPermission
                  ? t('common.noPermission', 'No permission')
                  : !row.original.remote_cleanup
                    ? t(
                        'accounts.cleanup.runNotConfigured',
                        'Configure source cleanup first; manual runs use the saved configuration.'
                      )
                    : undefined
              }
              onClick={() => setCleanupRunOpen(true)}
            >
              {t('accounts.cleanupRunAction', 'Run cleanup now')}
              <DropdownMenuShortcut>
                <Play size={16} />
              </DropdownMenuShortcut>
            </DropdownMenuItem>
          )}

          {account_type === 'IMAP' && (
            <DropdownMenuItem
              className='text-xs'
              disabled={!hasReadPermission && !hasPermission}
              title={
                !hasReadPermission && !hasPermission
                  ? t('common.noPermission', 'No permission')
                  : undefined
              }
              onClick={() => setCleanupReportsOpen(true)}
            >
              {t('accounts.cleanupReportsAction', 'Cleanup reports')}
              <DropdownMenuShortcut>
                <History size={16} />
              </DropdownMenuShortcut>
            </DropdownMenuItem>
          )}

          {hasPermission && <DropdownMenuSeparator />}

          {hasPermission && <DropdownMenuItem
            disabled={deleteQueued}
            onClick={() => {
              setCurrentRow(row.original)
              setOpen('delete')
            }}
            className='!text-red-500 text-xs'
          >
            {t('accounts.delete')}
            <DropdownMenuShortcut>
              {deleteQueued ? (
                <span className='flex items-center gap-1 text-amber-600 dark:text-amber-500'>
                  <IconClock size={16} />
                  {t('approvals.pendingBadge', 'Awaiting approval')}
                </span>
              ) : (
                <IconTrash size={16} />
              )}
            </DropdownMenuShortcut>
          </DropdownMenuItem>}
        </DropdownMenuContent>
      </DropdownMenu>
      <StartDownloadDialog
        row={row.original}
        open={startDialogOpen}
        onOpenChange={setStartDialogOpen}
      />
      <CleanupConfigDialog
        account={row.original}
        open={cleanupOpen}
        onOpenChange={setCleanupOpen}
      />
      <CleanupReportsDialog
        account={row.original}
        open={cleanupReportsOpen}
        onOpenChange={setCleanupReportsOpen}
      />
      <CleanupRunDialog
        account={row.original}
        open={cleanupRunOpen}
        onOpenChange={setCleanupRunOpen}
      />
    </>
  )
}
