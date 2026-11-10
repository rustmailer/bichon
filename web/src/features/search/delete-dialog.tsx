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

import { useMemo } from 'react'
import { IconAlertTriangle } from '@tabler/icons-react'
import { toast } from '@/hooks/use-toast'
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert'
import { ConfirmDialog } from '@/components/confirm-dialog'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { delete_messages } from '@/api/mailbox/envelope/api'
import {
  create_batch_delete,
  preview_batch_delete,
} from '@/api/batch-delete/api'
import { OP_MESSAGE_DELETE } from '@/api/approvals/api'
import { useGatedOp } from '@/features/approvals/use-gated-op'
import { useSearchContext } from './context'
import { mapToRecordOfArrays } from '@/lib/utils'
import { useTranslation } from 'react-i18next'

interface Props {
  open: boolean
  onOpenChange: (open: boolean) => void
}

export function EnvelopeDeleteDialog({ open, onOpenChange }: Props) {
  const queryClient = useQueryClient()
  const {
    toDelete,
    setToDelete,
    setSelected,
    selectAllScope,
    setSelectAllScope,
  } = useSearchContext()
  const { t, i18n } = useTranslation()
  const { gated, announceIfQueued } = useGatedOp(OP_MESSAGE_DELETE)

  const scopeMode = !!selectAllScope
  const scopeFilter = useMemo(() => {
    try {
      return JSON.parse(selectAllScope?.filterJson ?? '{}')
    } catch {
      return {}
    }
  }, [selectAllScope?.filterJson])

  // The authoritative count and the legal-hold check come from the server,
  // not from the search page's cached total: the page can be a few seconds
  // stale, and a job that touches a held account must be refused before the
  // operator confirms it.
  const preview = useQuery({
    queryKey: ['batch-delete-preview', scopeFilter],
    queryFn: () => preview_batch_delete({ filter: scopeFilter }),
    enabled: open && scopeMode,
    retry: false,
  })

  const deleteMutation = useMutation({
    mutationFn: ({ payload }: { payload: Record<number, string[]> }) =>
      delete_messages(payload),
    retry: false,
    onSuccess: (data) => {
      queryClient.invalidateQueries({ queryKey: ['search-messages'], exact: false })
      queryClient.invalidateQueries({ queryKey: ['all-tags'] })
      queryClient.invalidateQueries({ queryKey: ['approvals'] })
      onOpenChange(false)
      setToDelete(new Map())
      setSelected(new Map())

      // A queued deletion must not report that the messages are gone. They are
      // still there — the list was cleared because the operator asked for it,
      // not because anything happened.
      if (announceIfQueued(data)) return

      toast({
        title: t('search.delete.successTitle'),
        description: t('search.delete.successDesc'),
      })
    },
    onError: (error: Error) => {
      toast({
        title: t('search.delete.errorTitle'),
        description: `${error.message}`,
        variant: 'destructive',
      })
    },
  })

  const jobMutation = useMutation({
    mutationFn: () => create_batch_delete({ filter: scopeFilter }),
    retry: false,
    onSuccess: (data) => {
      queryClient.invalidateQueries({ queryKey: ['search-messages'], exact: false })
      queryClient.invalidateQueries({ queryKey: ['all-tags'] })
      queryClient.invalidateQueries({ queryKey: ['batch-delete-jobs'] })
      onOpenChange(false)
      setToDelete(new Map())
      setSelected(new Map())
      setSelectAllScope(null)

      if (announceIfQueued(data)) return

      toast({
        title: t('search.batchDelete.startedTitle', 'Batch deletion started'),
        description: t(
          'search.batchDelete.startedDesc',
          'The job runs in the background. You can close this page; progress stays visible in the deletion history.'
        ),
      })
    },
    onError: (error: Error) => {
      toast({
        title: t('search.delete.errorTitle'),
        description: `${error.message}`,
        variant: 'destructive',
      })
    },
  })

  const handleDelete = () => {
    if (scopeMode) {
      jobMutation.mutate()
      return
    }
    const payload = mapToRecordOfArrays(toDelete)
    deleteMutation.mutate({ payload })
  }

  const isLoading =
    deleteMutation.isPending ||
    jobMutation.isPending ||
    (scopeMode && preview.isLoading)

  const emailCount = scopeMode
    ? (preview.data?.total ?? selectAllScope?.total ?? 0)
    : Array.from(toDelete.values()).reduce((sum, set) => sum + set.size, 0)
  const heldAccounts = preview.data?.held_account_ids ?? []
  const skippedNoPermission = preview.data?.skipped_no_permission ?? 0
  const retentionLockedAccounts =
    preview.data?.retention_locked_account_ids?.length ?? 0
  // The preview is authoritative for the scope mode: only messages on
  // accounts the caller may delete from are part of the job.
  const nothingDeletable =
    scopeMode &&
    !preview.isLoading &&
    !preview.isError &&
    !!preview.data &&
    preview.data.total === 0

  return (
    <ConfirmDialog
      open={open}
      onOpenChange={onOpenChange}
      handleConfirm={handleDelete}
      className="max-w-xl"
      isLoading={isLoading}
      destructive
      disabled={nothingDeletable || (scopeMode && preview.isError)}
      title={
        <span className="text-destructive">
          <IconAlertTriangle
            className="mr-1 inline-block stroke-destructive"
            size={18}
          />{' '}
          {t('search.delete.title')}
        </span>
      }
      desc={
        <div className="space-y-4 text-xs">
          <p className="mb-2">
            {t('search.delete.confirmPrefix')}{' '}
            <span className="font-bold">
              {t('search.delete.countLabel', { count: emailCount })}
            </span>
            ?
            <br />
            {scopeMode
              ? t(
                  'search.batchDelete.confirmDetail',
                  'Every message matching the current search is permanently deleted. The job runs in the background and cannot be undone.'
                )
              : t('search.delete.confirmDetail')}
          </p>

          {scopeMode && preview.isError && (
            <Alert variant="destructive" className="text-xs">
              <AlertTitle>{t('search.delete.errorTitle')}</AlertTitle>
              <AlertDescription className="text-xs">
                {preview.error instanceof Error
                  ? preview.error.message
                  : String(preview.error)}
              </AlertDescription>
            </Alert>
          )}

          {scopeMode && skippedNoPermission > 0 && (
            <Alert className="text-xs">
              <AlertTitle>
                {t(
                  'search.batchDelete.skippedTitle',
                  'Some messages will be skipped'
                )}
              </AlertTitle>
              <AlertDescription className="text-xs">
                {t('search.batchDelete.skippedNoPermission', {
                  formatted: new Intl.NumberFormat(i18n.language).format(
                    skippedNoPermission
                  ),
                  defaultValue:
                    '{{formatted}} matching messages are on accounts you cannot delete; they will be left untouched.',
                })}
              </AlertDescription>
            </Alert>
          )}

          {nothingDeletable && (
            <Alert variant="destructive" className="text-xs">
              <AlertTitle>{t('search.delete.errorTitle')}</AlertTitle>
              <AlertDescription className="text-xs">
                {t(
                  'search.batchDelete.noneDeletable',
                  'You do not have delete permission on any matching message.'
                )}
              </AlertDescription>
            </Alert>
          )}

          {scopeMode && retentionLockedAccounts > 0 && (
            <Alert className='text-xs'>
              <AlertTitle>
                {t(
                  'search.batchDelete.retentionLockTitle',
                  'Minimum retention is active'
                )}
              </AlertTitle>
              <AlertDescription className='text-xs'>
                {t('search.batchDelete.retentionLockDesc', {
                  count: retentionLockedAccounts,
                  defaultValue:
                    '{{count}} account(s) in scope have a minimum-retention lock. Messages younger than the lock window will be skipped, not deleted.',
                })}
              </AlertDescription>
            </Alert>
          )}

          {scopeMode && heldAccounts.length > 0 && (
            <Alert variant="destructive" className="text-xs">
              <AlertTitle>{t('search.delete.warningTitle')}</AlertTitle>
              <AlertDescription className="text-xs">
                {t(
                  'search.batchDelete.heldWarning',
                  'Accounts under legal hold are in scope; the server will refuse this deletion.'
                )}
              </AlertDescription>
            </Alert>
          )}

          {gated ? (
            <Alert className="text-xs">
              <AlertTitle>
                {t('approvals.gatedMessagesTitle', 'Dual control is on for this operation')}
              </AlertTitle>
              <AlertDescription className="text-xs">
                {t(
                  'approvals.gatedMessagesDesc',
                  'This will NOT delete these messages now. It submits a request that a second person must approve, and only then are they deleted. They stay in the archive until that happens.'
                )}
              </AlertDescription>
            </Alert>
          ) : (
            <Alert variant="destructive" className="text-xs">
              <AlertTitle>{t('search.delete.warningTitle')}</AlertTitle>
              <AlertDescription className="text-xs">
                {t('search.delete.warningDesc')}
              </AlertDescription>
            </Alert>
          )}
        </div>
      }
      confirmText={
        scopeMode
          ? gated
            ? t('approvals.submitForApproval', 'Submit for approval')
            : t('search.batchDelete.submit', 'Delete in background')
          : gated
            ? t('approvals.submitForApproval', 'Submit for approval')
            : t('search.delete.confirmButton')
      }
    />
  )
}
