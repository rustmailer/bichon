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

import { useEffect, useRef, useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useTranslation } from 'react-i18next'
import { History, Loader2, X } from 'lucide-react'
import {
  BatchDeleteJob,
  cancel_batch_delete,
  list_batch_deletes,
} from '@/api/batch-delete/api'
import { Button } from '@/components/ui/button'
import { Card, CardContent } from '@/components/ui/card'
import { Progress } from '@/components/ui/progress'
import { toast } from '@/hooks/use-toast'
import { DeletionHistoryDrawer } from '@/features/history/history-drawers'
import { isActive, jobLabel } from './batch-delete-status'

/**
 * A running batch deletion, as a floating status card.
 *
 * It used to sit at the top of the result list, pushing the table down for the
 * entire (possibly hour-long) job. A fixed card in the bottom-left corner keeps
 * the progress, cancel and history reachable without occupying list space; on
 * narrow screens it sits above the bulk-action bar. When the job ends the card
 * disappears and a toast reports the outcome.
 */
export function BatchDeletePanel() {
  const { t, i18n } = useTranslation()
  const queryClient = useQueryClient()
  const [historyOpen, setHistoryOpen] = useState(false)

  const jobsQuery = useQuery({
    queryKey: ['batch-delete-jobs'],
    queryFn: list_batch_deletes,
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
  const active = jobs.find(isActive)
  const watchedJobId = useRef<string | null>(null)
  const wasActive = useRef(false)

  // Transition detection does two things the card cannot: refresh the stale
  // result list, and tell the user the job finished after the card vanished.
  useEffect(() => {
    const data = jobsQuery.data ?? []
    const running = data.find(isActive)
    if (running) {
      watchedJobId.current = running.job_id
    } else if (wasActive.current) {
      queryClient.invalidateQueries({ queryKey: ['search-messages'], exact: false })
      queryClient.invalidateQueries({ queryKey: ['all-tags'] })
      const finished = data.find((job) => job.job_id === watchedJobId.current)
      if (finished) {
        toast({
          title: String(
            t(`search.batchDelete.status.${finished.status}`, finished.status)
          ),
          description:
            t('search.batchDelete.progress', {
              processed: new Intl.NumberFormat(i18n.language).format(
                finished.deleted
              ),
              total: new Intl.NumberFormat(i18n.language).format(finished.total),
              defaultValue: '{{processed}} of {{total}} deleted',
            }) +
              ((finished.skipped_locked ?? 0) > 0
                ? ` · ${t('search.batchDelete.skippedLocked', {
                    count: finished.skipped_locked,
                    defaultValue: '{{count}} skipped (retention lock)',
                  })}`
                : '') +
              (finished.error ? ` · ${finished.error}` : ''),
          variant: finished.status === 'success' ? 'default' : 'destructive',
        })
      }
      watchedJobId.current = null
    }
    wasActive.current = !!running
  }, [jobsQuery.data, queryClient, t, i18n.language])

  const percent =
    active && active.total > 0
      ? Math.min(100, Math.round((active.processed / active.total) * 100))
      : 0
  const format = (value: number) =>
    new Intl.NumberFormat(i18n.language).format(value)

  return (
    <>
      {active && (
        <div className='fixed bottom-4 left-4 z-40 w-80 max-w-[calc(100vw-2rem)] max-lg:bottom-24'>
          <Card className='border-primary/30 bg-background/95 shadow-xl backdrop-blur'>
            <CardContent className='flex flex-col gap-2 p-3'>
              <div className='flex items-center justify-between gap-2 text-xs'>
                <span className='flex items-center gap-1.5 font-medium'>
                  <Loader2 className='h-3.5 w-3.5 animate-spin text-primary' />
                  {String(
                    t(`search.batchDelete.status.${active.status}`, active.status)
                  )}
                </span>
                <span className='tabular-nums text-muted-foreground'>
                  {format(active.deleted)} / {format(active.total)}
                </span>
              </div>
              <Progress value={percent} />
              <div className='truncate text-xs text-muted-foreground'>
                {jobLabel(active)}
              </div>
              <div className='flex items-center justify-between gap-1'>
                <Button
                  variant='ghost'
                  size='sm'
                  className='h-7 gap-1 px-2 text-xs'
                  onClick={() => setHistoryOpen(true)}
                >
                  <History className='h-3.5 w-3.5' />
                  {t('search.batchDelete.history', 'Deletion history')}
                </Button>
                <Button
                  variant='outline'
                  size='sm'
                  className='h-7 gap-1 px-2 text-xs'
                  disabled={
                    active.status === 'cancelling' || cancelMutation.isPending
                  }
                  onClick={() => cancelMutation.mutate(active.job_id)}
                >
                  <X className='h-3.5 w-3.5' />
                  {t('search.batchDelete.cancel', 'Cancel job')}
                </Button>
              </div>
            </CardContent>
          </Card>
        </div>
      )}
      <DeletionHistoryDrawer open={historyOpen} onOpenChange={setHistoryOpen} />
    </>
  )
}
