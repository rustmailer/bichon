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

import { useEffect, useRef } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Loader2, X } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import {
  BatchDeleteJob,
  cancel_batch_delete,
  list_batch_deletes,
} from '@/api/batch-delete/api'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { ScrollArea } from '@/components/ui/scroll-area'
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from '@/components/ui/sheet'
import { toast } from '@/hooks/use-toast'
import { RepairHistoryPanel } from '@/features/repair/repair-history-drawer'
import { isActive, jobLabel, statusVariant } from '@/features/search/batch-delete-status'

interface HistoryDrawerProps {
  open: boolean
  onOpenChange: (open: boolean) => void
}

/** Batch deletion jobs, newest first. Separate from repair history: the two
 *  are different operators' workflows and were deliberately un-merged. */
export function DeletionHistoryDrawer({
  open,
  onOpenChange,
}: HistoryDrawerProps) {
  const { t } = useTranslation()
  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <SheetContent
        side='right'
        className='flex w-full flex-col gap-0 overflow-hidden sm:max-w-4xl'
      >
        <SheetHeader className='pb-2'>
          <SheetTitle>
            {t('search.batchDelete.history', 'Deletion history')}
          </SheetTitle>
          <SheetDescription>
            {t(
              'search.batchDelete.historyDesc',
              'The most recent batch deletions. Only the last 30 are kept.'
            )}
          </SheetDescription>
        </SheetHeader>
        <div className='flex-1 overflow-y-auto px-4 pb-6'>
          <DeletionHistoryPanel enabled={open} />
        </div>
      </SheetContent>
    </Sheet>
  )
}

/** Blob repair reports, newest first. Read-only: repairs are triggered from
 *  the row actions, not from here. */
export function RepairHistoryDrawer({
  open,
  onOpenChange,
}: HistoryDrawerProps) {
  const { t } = useTranslation()
  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <SheetContent
        side='right'
        className='flex w-full flex-col gap-0 overflow-hidden sm:max-w-4xl'
      >
        <SheetHeader className='pb-2'>
          <SheetTitle>{t('repair_message.history', 'Repair history')}</SheetTitle>
        </SheetHeader>
        <div className='flex-1 overflow-y-auto px-4 pb-6'>
          <RepairHistoryPanel enabled={open} />
        </div>
      </SheetContent>
    </Sheet>
  )
}

/**
 * The filter type has a long tail of optional fields, so serializing it writes
 * `null` for almost everything. Strip null-ish entries recursively: the JSON
 * is there to show the conditions the operator set, not the shape of the type.
 */
function compactFilter(value: unknown): unknown {
  if (Array.isArray(value)) {
    return value
      .map(compactFilter)
      .filter((item) => item !== null && item !== undefined)
  }
  if (value && typeof value === 'object') {
    const out: Record<string, unknown> = {}
    for (const [key, entry] of Object.entries(value)) {
      if (entry === null || entry === undefined) continue
      out[key] = compactFilter(entry)
    }
    return out
  }
  return value
}

function DeletionHistoryPanel({ enabled }: { enabled: boolean }) {
  const { t, i18n } = useTranslation()
  const queryClient = useQueryClient()

  const jobsQuery = useQuery({
    queryKey: ['batch-delete-jobs'],
    queryFn: list_batch_deletes,
    enabled,
    // Poll only while a job is in flight; a finished history does not need to
    // keep asking the server.
    refetchInterval: (query) => {
      const data = query.state.data as BatchDeleteJob[] | undefined
      return data?.some(isActive) ? 1500 : false
    },
  })

  const cancelMutation = useMutation({
    mutationFn: (jobId: string) => cancel_batch_delete(jobId),
    retry: false,
    onSuccess: () => {
      toast({
        title: t(
          'search.batchDelete.cancelRequested',
          'Cancellation requested — the job stops after the current batch.'
        ),
      })
      queryClient.invalidateQueries({ queryKey: ['batch-delete-jobs'] })
    },
    onError: (error: Error) => {
      toast({
        title: t('search.delete.errorTitle'),
        description: `${error.message}`,
        variant: 'destructive',
      })
    },
  })

  const jobs = jobsQuery.data ?? []

  // When the last running job ends, the result list is stale by definition.
  // Refresh it once instead of waiting for the user to navigate.
  const wasActive = useRef(false)
  useEffect(() => {
    const anyActive = (jobsQuery.data ?? []).some(isActive)
    if (wasActive.current && !anyActive) {
      queryClient.invalidateQueries({ queryKey: ['search-messages'], exact: false })
      queryClient.invalidateQueries({ queryKey: ['all-tags'] })
    }
    wasActive.current = anyActive
  }, [jobsQuery.data, queryClient])

  if (jobsQuery.isLoading) {
    return (
      <div className='py-8 text-center'>
        <Loader2 className='mx-auto h-4 w-4 animate-spin text-muted-foreground' />
      </div>
    )
  }

  if (jobs.length === 0) {
    return (
      <p className='py-8 text-center text-xs text-muted-foreground'>
        {t('history.deletionsEmpty', 'No deletion jobs yet.')}
      </p>
    )
  }

  return (
    <div className='space-y-2'>
      {jobs.map((job) => {
        const filterView = compactFilter(job.filter ?? {}) as Record<
          string,
          unknown
        >
        return (
        <div key={job.job_id} className='rounded-lg border p-3 text-xs'>
          <div className='flex items-center justify-between gap-2'>
            <div className='flex min-w-0 items-center gap-2'>
              <Badge variant={statusVariant(job.status)}>
                {String(
                  t(`search.batchDelete.status.${job.status}`, job.status)
                )}
              </Badge>
              <span className='truncate font-medium'>{jobLabel(job)}</span>
            </div>
            <div className='flex shrink-0 items-center gap-2'>
              <span className='text-xs text-muted-foreground'>
                {new Date(job.created_at).toLocaleString()}
              </span>
              {isActive(job) && (
                <Button
                  variant='outline'
                  size='sm'
                  className='h-7 text-xs'
                  disabled={
                    job.status === 'cancelling' || cancelMutation.isPending
                  }
                  onClick={() => cancelMutation.mutate(job.job_id)}
                >
                  <X className='h-3.5 w-3.5' />
                  {t('search.batchDelete.cancel', 'Cancel job')}
                </Button>
              )}
            </div>
          </div>
          <div className='mt-1 text-xs text-muted-foreground'>
            {t('search.batchDelete.progress', {
              processed: new Intl.NumberFormat(i18n.language).format(
                job.deleted
              ),
              total: new Intl.NumberFormat(i18n.language).format(job.total),
              defaultValue: '{{processed}} of {{total}} deleted',
            })}
            {job.failed > 0 &&
              ` · ${t('search.batchDelete.failedCount', {
                count: job.failed,
                defaultValue: '{{count}} failed',
              })}`}
            {(job.skipped_locked ?? 0) > 0 &&
              ` · ${t('search.batchDelete.skippedLocked', {
                count: job.skipped_locked,
                defaultValue: '{{count}} skipped (retention lock)',
              })}`}
            {` · ${job.username}`}
            {job.executed_by && (
              <>
                {' · '}
                {t('search.batchDelete.executedBy', {
                  name: job.executed_by,
                  defaultValue: 'approved by {{name}}',
                })}
              </>
            )}
            {job.approval_request_id && (
              <>
                {' · '}
                {t('search.batchDelete.approvalRequest', {
                  id: job.approval_request_id,
                  defaultValue: 'request {{id}}',
                })}
              </>
            )}
          </div>
          {job.error && (
            <div className='mt-1 text-xs text-destructive'>{job.error}</div>
          )}
          {Object.keys(filterView).length > 0 && (
            <details className='mt-1'>
              <summary className='cursor-pointer text-xs text-muted-foreground'>
                {t('history.filter', 'Filter')}
              </summary>
              <ScrollArea
                className='mt-1 rounded bg-muted/40'
                viewportClassName='max-h-32'
              >
                <pre className='whitespace-pre-wrap break-all p-2 text-[10px]'>
                  {JSON.stringify(filterView, null, 2)}
                </pre>
              </ScrollArea>
            </details>
          )}
        </div>
        )
      })}
    </div>
  )
}
