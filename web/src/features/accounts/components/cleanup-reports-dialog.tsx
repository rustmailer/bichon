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
import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { AlertTriangle, RefreshCw } from 'lucide-react'
import type {
  AccountModel,
  RemoteCleanupRun,
  RemoteCleanupState,
} from '@/api/account/api'
import { remote_cleanup_state } from '@/api/account/api'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { useToast } from '@/hooks/use-toast'
import { errorMessage } from './use-cleanup-job'
import { CleanupUnprocessedList } from './cleanup-unprocessed-list'

interface CleanupReportsDialogProps {
  account: AccountModel
  open: boolean
  onOpenChange: (open: boolean) => void
}

function statusClass(status: string): string {
  switch (status) {
    case 'Success':
      return 'bg-green-500/10 text-green-600'
    case 'Failed':
      return 'bg-red-500/10 text-red-600'
    case 'Blocked':
      return 'bg-amber-500/10 text-amber-600'
    case 'Running':
      return 'bg-blue-500/10 text-blue-600'
    default:
      return 'bg-muted text-muted-foreground'
  }
}

function when(timestamp: number): string {
  try {
    return new Date(timestamp).toLocaleString()
  } catch {
    return String(timestamp)
  }
}

function bytes(value: number): string {
  if (value >= 1024 * 1024 * 1024) return `${(value / (1024 * 1024 * 1024)).toFixed(2)} GB`
  if (value >= 1024 * 1024) return `${(value / (1024 * 1024)).toFixed(1)} MB`
  if (value >= 1024) return `${(value / 1024).toFixed(1)} KB`
  return `${value} B`
}

/**
 * Read-only archive of the last cleanup runs. Move runs are stored in the
 * account's RemoteCleanupState (30 most recent). This is where failure reasons
 * and per-folder outcomes are visible after a dialog has been closed.
 */
export function CleanupReportsDialog({
  account,
  open,
  onOpenChange,
}: CleanupReportsDialogProps) {
  const { t } = useTranslation()
  const { toast } = useToast()
  const [state, setState] = useState<RemoteCleanupState | null>(null)
  const [loading, setLoading] = useState(false)

  const load = async () => {
    setLoading(true)
    try {
      setState(await remote_cleanup_state(account.id))
    } catch (error) {
      toast({
        variant: 'destructive',
        title: t('accounts.cleanup.failed', 'Cleanup request failed'),
        description: errorMessage(error),
      })
    } finally {
      setLoading(false)
    }
  }

  useEffect(() => {
    if (!open) return
    setState(null)
    load()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, account.id])

  // Newest first: the running run (if any), then history ordered by start
  // time descending, independent of the order the server stored it in.
  const moveRuns: RemoteCleanupRun[] = [
    ...(state?.active ? [state.active] : []),
    ...[...(state?.history ?? [])].sort((a, b) => b.started_at - a.started_at),
  ]

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className='max-w-3xl max-h-[90vh] [&_button]:text-xs [&_label]:text-xs'>
        <DialogHeader>
          <DialogTitle>
            {t('accounts.cleanup.reportsTitle', 'Cleanup reports')}
          </DialogTitle>
          <DialogDescription>
            {t(
              'accounts.cleanup.reportsDesc',
              'The last 30 cleanup runs for this account, with per-folder outcomes and failure reasons.'
            )}
          </DialogDescription>
        </DialogHeader>

        <Tabs defaultValue='moves' className='w-full'>
          <div className='flex items-center justify-between gap-2'>
            <TabsList>
              <TabsTrigger value='moves'>
                {t('accounts.cleanup.moveRuns', 'Move runs')}
              </TabsTrigger>
            </TabsList>
            <Button
              type='button'
              variant='ghost'
              size='sm'
              disabled={loading}
              onClick={load}
            >
              <RefreshCw className='h-3.5 w-3.5' />
              {t('accounts.cleanup.refresh', 'Refresh')}
            </Button>
          </div>

          <TabsContent value='moves'>
            {/* A native scroll container, not <ScrollArea>: Radix's viewport
                sizes itself with height:100%, which needs a definite height on
                the root. With only max-height the viewport grows with the
                content and the root clips it, so the list could not scroll. */}
            <div className='max-h-[60vh] overflow-y-auto pr-4'>
              <div className='pt-2 text-xs'>
                {moveRuns.length === 0 && (
                  <p className='text-muted-foreground'>
                    {t('accounts.cleanup.noRuns', 'No cleanup runs recorded yet.')}
                  </p>
                )}
                {moveRuns.length > 0 && (
                  <div className='relative'>
                    {/* Timeline spine with the arrowhead at the top, pointing
                        at the newest run: time runs from the top (latest)
                        downwards to older runs. Mirrors the account status
                        history. */}
                    {moveRuns.length > 1 && (
                      <>
                        <div
                          className='absolute left-[7px] top-2 bottom-1 w-0.5 bg-border'
                          aria-hidden
                        />
                        <div
                          className='absolute left-[3px] top-0 h-0 w-0 border-l-[5px] border-r-[5px] border-b-[7px] border-l-transparent border-r-transparent border-b-border'
                          aria-hidden
                        />
                      </>
                    )}
                    <div className='space-y-3'>
                      {moveRuns.map((run, index) => (
                        <div key={run.run_id} className='relative pl-6'>
                          {index === 0 ? (
                            <span
                              className='absolute left-[3px] top-4 flex h-2.5 w-2.5'
                              aria-hidden
                            >
                              <span className='animate-ping absolute inline-flex h-full w-full rounded-full bg-primary opacity-75' />
                              <span className='relative inline-flex h-2.5 w-2.5 rounded-full bg-primary' />
                            </span>
                          ) : (
                            <span
                              className='absolute left-[4px] top-[17px] h-2 w-2 rounded-full bg-muted-foreground/30'
                              aria-hidden
                            />
                          )}
                          <RunCard run={run} isLatest={index === 0} />
                        </div>
                      ))}
                    </div>
                  </div>
                )}
              </div>
            </div>
          </TabsContent>

        </Tabs>

        <DialogFooter>
          <Button type='button' variant='outline' onClick={() => onOpenChange(false)}>
            {t('common.close', 'Close')}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

function Metric({ label, value }: { label: string; value: number }) {
  return (
    <div>
      <div className='text-[10px] uppercase tracking-wide text-muted-foreground'>
        {label}
      </div>
      <div className='font-semibold'>{value}</div>
    </div>
  )
}

function RunCard({ run, isLatest }: { run: RemoteCleanupRun; isLatest: boolean }) {
  const { t } = useTranslation()
  return (
    <div className={`rounded-md border p-3 ${isLatest ? 'border-primary/40 bg-primary/[0.03]' : ''}`}>
      <div className='flex flex-wrap items-center justify-between gap-2'>
        <div className='flex items-center gap-2'>
          <span className='font-semibold'>{when(run.started_at)}</span>
          <Badge
            variant='outline'
            className={`${statusClass(run.status)} border-none font-medium`}
          >
            {run.status}
          </Badge>
          {run.status === 'Running' && (
            <Badge variant='outline' className='border-none bg-blue-500/10 text-blue-600'>
              {t('accounts.cleanup.activeRun', 'Active')}
            </Badge>
          )}
          {isLatest && (
            <Badge variant='outline' className='border-none bg-primary/10 text-primary'>
              {t('accounts.cleanup.latest', 'Latest')}
            </Badge>
          )}
        </div>
        <span className='text-muted-foreground'>
          {run.trigger}
          {run.user ? ` · ${run.user}` : ''}
        </span>
      </div>

      <div className='mt-2 grid grid-cols-2 gap-x-4 gap-y-1 sm:grid-cols-4'>
        <Metric
          label={t('accounts.cleanup.reportMoved', 'Moved')}
          value={run.moved}
        />
        <Metric
          label={t('accounts.cleanup.reportFailed', 'Failed')}
          value={run.failed}
        />
        <Metric
          label={t('accounts.cleanup.reportBytes', 'Bytes')}
          value={run.bytes_moved}
        />
        <Metric
          label={t('accounts.cleanup.reportFolders', 'Folders')}
          value={Object.keys(run.folders).length}
        />
      </div>

      {run.message && (
        <div className='mt-2 flex items-start gap-1.5 rounded bg-amber-500/10 p-2 text-amber-700'>
          <AlertTriangle className='mt-0.5 h-3 w-3 shrink-0' />
          <span>{run.message}</span>
        </div>
      )}

      <div className='mt-2 space-y-1.5'>
        {Object.entries(run.folders).map(([name, stats]) => (
          <div key={name} className='rounded bg-muted/40 p-2'>
            <div className='flex flex-wrap items-center justify-between gap-2'>
              <span className='font-semibold'>{name}</span>
              {/* Successes and failures only: routine skips (too recent, gone
                  from the source, flagged) are not actionable here. */}
              <span className='text-muted-foreground'>
                {t('accounts.cleanup.reportMoved', 'Moved')}: {stats.moved} ·{' '}
                {t('accounts.cleanup.reportFailed', 'Failed')}: {stats.failed} ·{' '}
                {t('accounts.cleanup.reportBytes', 'Bytes')}: {bytes(stats.bytes_moved)}
              </span>
            </div>
            {stats.message && (
              <div className='mt-1 text-amber-700'>{stats.message}</div>
            )}
            <div className='mt-1.5'>
              <CleanupUnprocessedList
                messages={stats.unprocessed}
                failed={stats.failed}
              />
            </div>
          </div>
        ))}
      </div>
    </div>
  )
}
