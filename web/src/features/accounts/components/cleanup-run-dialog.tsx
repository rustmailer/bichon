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
import { useQuery } from '@tanstack/react-query'
import { useTranslation } from 'react-i18next'
import { AlertTriangle, Loader2 } from 'lucide-react'
import type {
  AccountModel,
  CleanupPreview,
  RemoteCleanupRun,
} from '@/api/account/api'
import {
  CleanupJobStatus,
  execute_remote_cleanup,
  list_remote_cleanup_jobs,
  preview_remote_cleanup,
  remote_cleanup_state,
} from '@/api/account/api'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { useToast } from '@/hooks/use-toast'
import {
  errorMessage,
  useCleanupJob,
  useElapsedSeconds,
} from './use-cleanup-job'
import { CleanupUnprocessedList } from './cleanup-unprocessed-list'

interface CleanupRunDialogProps {
  account: AccountModel
  open: boolean
  onOpenChange: (open: boolean) => void
}

/**
 * Manual cleanup run, kept separate from the configuration dialog so editing
 * settings and acting on them are never confused. Preview is read-only;
 * Execute performs the reversible move into `bichon_auto_created_trash`.
 *
 * Both steps run as background jobs on the server, so closing this dialog (or
 * a slow verification) no longer risks losing a run: the work continues and
 * reopening the dialog resumes polling it.
 *
 * The client's job id can still be lost — a page reload, or the row component
 * remounting — while the server keeps the job. On open the dialog therefore
 * asks the server for the account's recent jobs and adopts the newest one
 * (running first): a still-running verification is picked back up and a
 * finished one shows its report again. Starting a second job meanwhile would
 * only be refused with "A cleanup job is already running for this account".
 */
export function CleanupRunDialog({
  account,
  open,
  onOpenChange,
}: CleanupRunDialogProps) {
  const { t } = useTranslation()
  const { toast } = useToast()
  const [confirmMove, setConfirmMove] = useState(false)

  const previewJob = useCleanupJob(account.id, open)
  const runJob = useCleanupJob(account.id, open)

  const config = account.remote_cleanup
  const configured = !!config

  const previewing = previewJob.busy
  const running = runJob.busy
  const preview: CleanupPreview | null =
    previewJob.job?.status === CleanupJobStatus.Success
      ? previewJob.job.preview ?? null
      : null
  // A preview reattached from an earlier visit can be past its token TTL. Its
  // report is still worth showing, but Execute would be refused, so gate it.
  // The server computes this against its own clock (`token_expired`), so a
  // skewed browser clock cannot hide a valid token or offer a dead one.
  const previewExpired = preview?.token_expired === true
  const result: RemoteCleanupRun | null =
    runJob.job?.run ?? null
  const previewError =
    previewJob.job && previewJob.job.status !== CleanupJobStatus.Success
      ? previewJob.job.error ?? null
      : previewJob.pollError
  const runError =
    runJob.job && runJob.job.status === CleanupJobStatus.Failed
      ? runJob.job.error ?? null
      : runJob.pollError

  // Elapsed counters keep the dialog alive-looking while verification walks
  // every candidate; the live run state adds message counters.
  // Counting from the job's `created_at` keeps them honest after a reattach.
  const previewSeconds = useElapsedSeconds(previewing, previewJob.job?.created_at)
  const runSeconds = useElapsedSeconds(running, runJob.job?.created_at)
  const { data: liveState } = useQuery({
    queryKey: ['remote-cleanup-state', account.id],
    queryFn: () => remote_cleanup_state(account.id),
    enabled: open && running,
    refetchInterval: running ? 2000 : false,
  })
  const liveRun = running ? liveState?.active ?? null : null

  // Jobs this account still has on the server, newest first. Jobs are
  // in-memory, so this is the only way back to a job started before the
  // dialog was reopened.
  const { data: knownJobs } = useQuery({
    queryKey: ['remote-cleanup-jobs', account.id],
    queryFn: () => list_remote_cleanup_jobs(account.id),
    enabled: open,
    staleTime: 0,
    refetchOnWindowFocus: false,
  })

  useEffect(() => {
    if (!open) return
    setConfirmMove(false)
    // A job left running by a previous visit keeps running; only a finished
    // one is cleared so a fresh preview/run starts from a clean slate.
    if (previewJob.job && previewJob.job.status !== CleanupJobStatus.Running) {
      previewJob.reset()
    }
    if (runJob.job && runJob.job.status !== CleanupJobStatus.Running) {
      runJob.reset()
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, account.id])

  // Reattach to the account's newest job (the server returns running jobs
  // ahead of finished ones). A running job resumes its verification/progress;
  // a finished one is shown again so its report is not lost with the dialog.
  useEffect(() => {
    if (!open || !knownJobs || knownJobs.length === 0) return
    const newest = knownJobs[0]
    if (newest.kind === 'move_preview') {
      if (previewJob.job?.job_id !== newest.job_id) previewJob.adopt(newest)
    } else if (runJob.job?.job_id !== newest.job_id) {
      runJob.adopt(newest)
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, knownJobs])

  /** Adopt the job that blocked a start request, if the server reports one. */
  const reclaimRunningJob = async (): Promise<boolean> => {
    try {
      const jobs = await list_remote_cleanup_jobs(account.id)
      const running = jobs.find(
        (job) => job.status === CleanupJobStatus.Running
      )
      if (!running) return false
      if (running.kind === 'move_preview') previewJob.adopt(running)
      else runJob.adopt(running)
      return true
    } catch {
      // Fall through so the caller still reports the original failure.
      return false
    }
  }

  const runPreview = async () => {
    try {
      await previewJob.start(() => preview_remote_cleanup(account.id))
      setConfirmMove(false)
    } catch (error) {
      // A lost job id is the common cause here: the server refuses a second
      // job while the first still runs. Reattach instead of only complaining.
      if (await reclaimRunningJob()) return
      toast({
        variant: 'destructive',
        title: t('accounts.cleanup.failed', 'Cleanup request failed'),
        description: errorMessage(error),
      })
    }
  }

  const runMove = async () => {
    if (!preview) return
    try {
      const finished = await runJob.start(() =>
        execute_remote_cleanup(account.id, preview.token)
      )
      if (finished.run) {
        toast({
          title: t('accounts.cleanup.moveDone', 'Move run finished'),
          description: `${finished.run.moved} moved, ${finished.run.skipped} skipped, ${finished.run.failed} failed`,
        })
      }
    } catch (error) {
      if (await reclaimRunningJob()) return
      toast({
        variant: 'destructive',
        title: t('accounts.cleanup.failed', 'Cleanup request failed'),
        description: errorMessage(error),
      })
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className='max-w-xl max-h-[90vh] [&_button]:text-xs [&_label]:text-xs'>
        <DialogHeader>
          <DialogTitle>
            {t('accounts.cleanup.runDialogTitle', 'Run cleanup now')}
          </DialogTitle>
          <DialogDescription>
            {t(
              'accounts.cleanup.runDialogDesc',
              'Manual and reversible: messages are moved into bichon_auto_created_trash on the IMAP server. Preview is read-only; Execute performs the move.'
            )}
          </DialogDescription>
        </DialogHeader>

        {/* Native scroll container: Radix's ScrollArea needs a definite height,
            a max-height alone leaves it unable to scroll (see reports dialog). */}
        <div className='max-h-[60vh] overflow-y-auto pr-4'>
          <div className='space-y-4 py-2 text-xs'>
            {!configured ? (
              <p className='rounded-md bg-muted p-3 text-muted-foreground'>
                {t(
                  'accounts.cleanup.runNotConfigured',
                  'Configure source cleanup first; manual runs use the saved configuration.'
                )}
              </p>
            ) : (
              <>
                <div className='space-y-1 rounded-md border p-3'>
                  <div className='font-semibold'>
                    {t('accounts.cleanup.runSavedConfig', 'Saved configuration')}
                  </div>
                  <div>
                    {t('accounts.cleanup.destination', 'Destination folder')}:{' '}
                    <span className='font-medium'>bichon_auto_created_trash</span>
                  </div>
                  <div>
                    {t('accounts.cleanup.grace', 'Archive age before moving (days)')}:{' '}
                    <span className='font-medium'>
                      {config.eligible_after_archive_days ?? 30}
                    </span>
                  </div>
                  <div>
                    {t('accounts.cleanup.minAge', 'Only move messages older than (days, optional)')}:{' '}
                    <span className='font-medium'>
                      {config.min_message_age_days ?? '—'}
                    </span>
                  </div>
                  <div>
                    {t('accounts.cleanup.maxPerRun', 'Max messages per batch')}:{' '}
                    <span className='font-medium'>
                      {config.max_messages_per_run ?? 50}
                    </span>
                  </div>
                  {config.min_message_age_days != null && (
                    <p className='pt-1 text-muted-foreground'>
                      {t('accounts.cleanup.ageBothHint', {
                        grace: config.eligible_after_archive_days ?? 30,
                        minAge: config.min_message_age_days,
                        defaultValue:
                          'Both conditions must be met: Bichon must have archived the message at least {{grace}} days ago AND the message itself must be older than {{minAge}} days. Mail archived more recently is skipped as too_recently_archived even when the email is old.',
                      })}
                    </p>
                  )}
                </div>

                <div className='space-y-1.5'>
                  <div className='flex flex-wrap items-center gap-2'>
                    <Button
                      type='button'
                      variant='outline'
                      disabled={previewing || running}
                      onClick={runPreview}
                      className='gap-1.5'
                    >
                      {previewing && <Loader2 className='h-3.5 w-3.5 animate-spin' />}
                      {previewing
                        ? t('accounts.cleanup.previewing', 'Previewing…')
                        : t('accounts.cleanup.previewMove', 'Preview move')}
                    </Button>
                    <span className='text-[10px] text-muted-foreground'>
                      {t(
                        'accounts.cleanup.previewHint',
                        'Preview is read-only: it verifies candidates and shows what would move, without moving anything.'
                      )}
                    </span>
                  </div>
                  {previewing && (
                    <div className='flex flex-wrap items-center gap-2 text-muted-foreground'>
                      <Loader2 className='mt-0.5 h-3 w-3 shrink-0 animate-spin' />
                      <span>
                        {t('accounts.cleanup.previewRunning', {
                          seconds: previewSeconds,
                          defaultValue:
                            'Verifying messages… {{seconds}}s elapsed. Each candidate is compared with its copy on the server before anything moves.',
                        })}
                      </span>
                      <Button
                        type='button'
                        variant='ghost'
                        size='sm'
                        className='h-6 px-2 text-[10px]'
                        disabled={!previewJob.job}
                        onClick={previewJob.cancel}
                      >
                        {t('common.cancel', 'Cancel')}
                      </Button>
                    </div>
                  )}
                  {previewError && (
                    <div className='flex items-start gap-1.5 text-destructive'>
                      <AlertTriangle className='mt-0.5 h-3 w-3 shrink-0' />
                      <span>{previewError}</span>
                    </div>
                  )}
                </div>

                {/* Once the run reports, the report replaces the preview: the
                    preview token was single-use and the candidate set is
                    stale. */}
                {preview && !result && (
                  <div className='space-y-2 rounded-md bg-muted/40 p-3'>
                    <div className='font-semibold'>
                      {t('accounts.cleanup.movePreviewTitle', 'Move preview')}
                    </div>
                    <div>
                      {t('accounts.cleanup.movePreviewCounts', {
                        ready: preview.verified,
                        bytes: formatBytes(preview.bytes),
                        defaultValue:
                          '{{ready}} message(s) ready to move · {{bytes}}',
                      })}
                    </div>
                    {/* Routine policy skips (too recent, gone from the source,
                        flagged) are expected and need no operator action, so
                        they are not listed here; only problems that change what
                        execution will do are surfaced below. */}
                    {preview.will_abort && (
                      <div className='flex items-start gap-1.5 text-destructive'>
                        <AlertTriangle className='mt-0.5 h-3 w-3 shrink-0' />
                        <span>
                          {t(
                            'accounts.cleanup.willAbort',
                            'At least one message failed identity verification. Execution will stop at the first such message.'
                          )}
                        </span>
                      </div>
                    )}
                    {preview.warnings.map((warning) => (
                      <div
                        key={warning}
                        className='flex items-start gap-1.5 text-amber-600'
                      >
                        <AlertTriangle className='mt-0.5 h-3 w-3 shrink-0' />
                        <span>{warning}</span>
                      </div>
                    ))}
                    <label className='flex items-start gap-2 pt-1'>
                      <Checkbox
                        checked={confirmMove}
                        disabled={previewExpired}
                        onCheckedChange={(value) => setConfirmMove(value === true)}
                      />
                      <span>
                        {t(
                          'accounts.cleanup.moveAcknowledge',
                          'I understand these messages will leave the source mailbox but remain in bichon_auto_created_trash on the IMAP server.'
                        )}
                      </span>
                    </label>
                    {previewExpired && (
                      <div className='flex items-start gap-1.5 text-muted-foreground'>
                        <AlertTriangle className='mt-0.5 h-3 w-3 shrink-0' />
                        <span>
                          {t(
                            'accounts.cleanup.previewExpired',
                            'This preview is older than 15 minutes, so it can no longer be executed. Run the preview again to move messages.'
                          )}
                        </span>
                      </div>
                    )}
                    <Button
                      type='button'
                      disabled={
                        running ||
                        previewExpired ||
                        !confirmMove ||
                        preview.verified === 0
                      }
                      onClick={runMove}
                      className='gap-1.5'
                    >
                      {running && <Loader2 className='h-3.5 w-3.5 animate-spin' />}
                      {running
                        ? t('accounts.cleanup.executing', 'Moving…')
                        : t('accounts.cleanup.executeMove', 'Execute move')}
                    </Button>
                    {running && (
                      <div className='flex flex-wrap items-center gap-2 text-muted-foreground'>
                        <Loader2 className='mt-0.5 h-3 w-3 shrink-0 animate-spin' />
                        <span>
                          {t('accounts.cleanup.executeRunning', {
                            moved: liveRun?.moved ?? 0,
                            total: preview.verified,
                            skipped: liveRun?.skipped ?? 0,
                            failed: liveRun?.failed ?? 0,
                            seconds: runSeconds,
                            defaultValue:
                              'Moving messages… {{moved}} of {{total}} moved ({{skipped}} skipped, {{failed}} failed) · {{seconds}}s elapsed. You can close this dialog; the run continues on the server.',
                          })}
                        </span>
                        <Button
                          type='button'
                          variant='ghost'
                          size='sm'
                          className='h-6 px-2 text-[10px]'
                          disabled={!runJob.job}
                          onClick={runJob.cancel}
                        >
                          {t('common.cancel', 'Cancel')}
                        </Button>
                      </div>
                    )}
                  </div>
                )}

                {/* A move run reattached after a reload has no local preview
                    (its token is gone), so the progress block above would not
                    render; show the live counters on their own instead. */}
                {running && !preview && (
                  <div className='flex flex-wrap items-center gap-2 rounded-md bg-muted/40 p-3 text-muted-foreground'>
                    <Loader2 className='mt-0.5 h-3 w-3 shrink-0 animate-spin' />
                    <span>
                      {t('accounts.cleanup.executeRunningAdopted', {
                        moved: liveRun?.moved ?? 0,
                        skipped: liveRun?.skipped ?? 0,
                        failed: liveRun?.failed ?? 0,
                        seconds: runSeconds,
                        defaultValue:
                          'Moving messages… {{moved}} moved ({{skipped}} skipped, {{failed}} failed) · {{seconds}}s elapsed. You can close this dialog; the run continues on the server.',
                      })}
                    </span>
                    <Button
                      type='button'
                      variant='ghost'
                      size='sm'
                      className='h-6 px-2 text-[10px]'
                      disabled={!runJob.job}
                      onClick={runJob.cancel}
                    >
                      {t('common.cancel', 'Cancel')}
                    </Button>
                  </div>
                )}

                {runError && (
                  <div className='flex items-start gap-1.5 text-destructive'>
                    <AlertTriangle className='mt-0.5 h-3 w-3 shrink-0' />
                    <span>{runError}</span>
                  </div>
                )}

                {result && (
                  <div className='space-y-1 rounded-md bg-muted/40 p-3'>
                    <div>
                      {t('accounts.cleanup.moveResult', {
                        defaultValue: '{{status}}: {{moved}} moved, {{failed}} failed',
                        status: result.status,
                        moved: result.moved,
                        failed: result.failed,
                      })}
                    </div>
                    {result.message && (
                      <div className='flex items-start gap-1.5 text-amber-700'>
                        <AlertTriangle className='mt-0.5 h-3 w-3 shrink-0' />
                        <span>{result.message}</span>
                      </div>
                    )}
                    {/* Only successes and failures. Routine skips (too recent,
                        gone from the source, flagged) are policy outcomes the
                        operator cannot act on, so they are not counted here;
                        the API still records them for diagnostics. */}
                    {Object.entries(result.folders).map(([name, stats]) => (
                      <div key={name} className='space-y-1'>
                        <div className='text-muted-foreground'>
                          {name}: {t('accounts.cleanup.reportMoved', 'Moved')}{' '}
                          {stats.moved} ·{' '}
                          {t('accounts.cleanup.reportFailed', 'Failed')}{' '}
                          {stats.failed}
                        </div>
                        <CleanupUnprocessedList
                          messages={stats.unprocessed}
                          failed={stats.failed}
                        />
                      </div>
                    ))}
                  </div>
                )}
              </>
            )}
          </div>
        </div>

        <DialogFooter>
          <Button
            type='button'
            variant='outline'
            onClick={() => onOpenChange(false)}
          >
            {t('common.close', 'Close')}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

function formatBytes(value: number): string {
  if (value >= 1024 * 1024 * 1024) return `${(value / (1024 * 1024 * 1024)).toFixed(2)} GB`
  if (value >= 1024 * 1024) return `${(value / (1024 * 1024)).toFixed(1)} MB`
  if (value >= 1024) return `${(value / 1024).toFixed(1)} KB`
  return `${value} B`
}
