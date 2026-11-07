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
import { AxiosError } from 'axios'
import { Download, Loader2 } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { IconAlertCircle } from '@tabler/icons-react'
import {
  createExport,
  downloadExport,
  getExportJob,
  is_queued_export,
  previewExport,
  type ExportJobView,
  type ExportPreviewView,
  type ExportRequest,
} from '@/api/export/api'
import { OP_EXPORT_BATCH } from '@/api/approvals/api'
import { useGatedOp } from '@/features/approvals/use-gated-op'
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { toast } from '@/hooks/use-toast'

const POLL_INTERVAL_MS = 2000

const formatBytes = (bytes: number) => {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MB`
  return `${(bytes / 1024 / 1024 / 1024).toFixed(2)} GB`
}

const getErrorMessage = (error: unknown) => {
  if (error instanceof AxiosError) {
    return (
      (error.response?.data as { message?: string } | undefined)?.message ||
      error.message
    )
  }
  return error instanceof Error ? error.message : String(error)
}

interface ExportDialogProps {
  /** What to export; `null` closes the dialog. */
  request: ExportRequest | null
  /** Overrides the saved-search title (e.g. "Export attachments"). */
  title?: string
  onClose: () => void
}

/**
 * Preview → confirm → poll → download for one batch export.
 *
 * Shared by the saved-search menu and the "download the current results"
 * buttons so both entry points behave identically, including the dual-control
 * case where the server queues the request instead of starting it.
 */
export function ExportDialog({ request, title, onClose }: ExportDialogProps) {
  const { t } = useTranslation()
  const [preview, setPreview] = React.useState<ExportPreviewView | null>(null)
  const [job, setJob] = React.useState<ExportJobView | null>(null)
  const [busy, setBusy] = React.useState(false)
  const { gated, announceIfQueued } = useGatedOp(OP_EXPORT_BATCH)
  const pollRef = React.useRef<ReturnType<typeof setInterval> | null>(null)

  const stopPolling = React.useCallback(() => {
    if (pollRef.current) {
      clearInterval(pollRef.current)
      pollRef.current = null
    }
  }, [])

  // A new request starts a new preview; the old job/preview state is done.
  React.useEffect(() => {
    stopPolling()
    setPreview(null)
    setJob(null)
    setBusy(false)
    if (!request) return

    let cancelled = false
    previewExport(request)
      .then((value) => {
        if (!cancelled) setPreview(value)
      })
      .catch((error) => {
        if (cancelled) return
        onClose()
        toast({
          title: t('exports.previewFailed'),
          description: getErrorMessage(error),
          variant: 'destructive',
        })
      })
    return () => {
      cancelled = true
      stopPolling()
    }
    // The request object is created once per click by the caller.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [request])

  const triggerDownload = (finished: ExportJobView) => {
    downloadExport(finished.job_id)
      .then((blob) => {
        const url = URL.createObjectURL(blob)
        const link = document.createElement('a')
        link.href = url
        // The server names the artifact ({job_id}.mbox / {job_id}.zip); the
        // extension decides whether the browser saves a message archive or a
        // zip, so keep the server's name rather than assuming mbox.
        link.download =
          finished.artifact_name ??
          `${finished.job_id}.${finished.format === 'Attachments' ? 'zip' : 'mbox'}`
        document.body.appendChild(link)
        link.click()
        document.body.removeChild(link)
        URL.revokeObjectURL(url)
        toast({ title: t('exports.downloadSaved') })
        onClose()
      })
      .catch((error) => {
        toast({
          title: t('exports.downloadFailed'),
          description: getErrorMessage(error),
          variant: 'destructive',
        })
        onClose()
      })
  }

  const startExport = () => {
    if (!request || !preview) return
    setBusy(true)
    createExport(request)
      .then((result) => {
        // Dual control is on for batch exports: the server queued the request
        // and answered 202. There is no job to poll and nothing has been
        // exported — polling `undefined` here would show a failure for a
        // request that is in fact waiting on a second person.
        if (is_queued_export(result)) {
          announceIfQueued(result)
          setBusy(false)
          onClose()
          return
        }
        const started = result
        setJob(started)
        pollRef.current = setInterval(() => {
          getExportJob(started.job_id)
            .then((updated) => {
              setJob(updated)
              if (updated.status === 'finished') {
                stopPolling()
                setBusy(false)
                triggerDownload(updated)
              } else if (
                updated.status === 'failed' ||
                updated.status === 'cancelled'
              ) {
                stopPolling()
                setBusy(false)
                toast({
                  title:
                    updated.status === 'failed'
                      ? t('exports.failed')
                      : t('exports.cancelled'),
                  description: updated.error ?? undefined,
                  variant: 'destructive',
                })
                onClose()
              }
            })
            .catch((error) => {
              stopPolling()
              setBusy(false)
              toast({
                title: t('exports.statusFailed'),
                description: getErrorMessage(error),
                variant: 'destructive',
              })
              onClose()
            })
        }, POLL_INTERVAL_MS)
      })
      .catch((error) => {
        setBusy(false)
        toast({
          title: t('exports.startFailed'),
          description: getErrorMessage(error),
          variant: 'destructive',
        })
        onClose()
      })
  }

  // `request.format` is known before the preview arrives, so the title and the
  // "Attachments" metric do not flicker from the mbox wording while loading.
  const isAttachments =
    request?.format === 'Attachments' || preview?.format === 'Attachments'
  const dialogTitle =
    title ??
    t(isAttachments ? 'exports.titleAttachments' : 'exports.title')

  return (
    <Dialog open={request !== null} onOpenChange={(open) => !open && onClose()}>
      <DialogContent className='sm:max-w-md'>
        <DialogHeader>
          <DialogTitle>{dialogTitle}</DialogTitle>
        </DialogHeader>
        {!preview ? (
          <div className='flex items-center gap-2 text-sm text-muted-foreground'>
            <Loader2 className='h-4 w-4 animate-spin' />
            {t('exports.exporting')}
          </div>
        ) : !job ? (
          <div className='space-y-3'>
            <div className='grid grid-cols-3 gap-2 text-center'>
              <div className='rounded-md border p-2'>
                <div className='text-lg font-semibold'>
                  {preview.accounts.length}
                </div>
                <div className='text-xs text-muted-foreground'>
                  {t('exports.previewAccounts')}
                </div>
              </div>
              <div className='rounded-md border p-2'>
                <div className='text-lg font-semibold'>
                  {preview.total_emails.toLocaleString()}
                </div>
                <div className='text-xs text-muted-foreground'>
                  {isAttachments
                    ? t('exports.previewAttachments')
                    : t('exports.previewEmails')}
                </div>
              </div>
              <div className='rounded-md border p-2'>
                <div className='text-lg font-semibold'>
                  {formatBytes(preview.total_size)}
                </div>
                <div className='text-xs text-muted-foreground'>
                  {t('exports.previewSize')}
                </div>
              </div>
            </div>
            {gated ? (
              <Alert>
                <IconAlertCircle className='h-4 w-4' />
                <AlertTitle>
                  {t(
                    'approvals.gatedExportTitle',
                    'Dual control is on for this operation'
                  )}
                </AlertTitle>
                <AlertDescription>
                  {t(
                    'approvals.gatedExportDesc',
                    'This will NOT export anything now. It submits a request that a second person must approve, and only then does the export run.'
                  )}
                </AlertDescription>
              </Alert>
            ) : (
              <p className='text-sm text-muted-foreground break-words'>
                {isAttachments
                  ? t('exports.confirmAttachments', {
                      count: preview.total_emails,
                      name: preview.saved_search_name,
                    })
                  : t('exports.confirm', {
                      count: preview.total_emails,
                      name: preview.saved_search_name,
                    })}
              </p>
            )}
            <DialogFooter>
              <Button variant='outline' onClick={onClose} disabled={busy}>
                {t('common.cancel')}
              </Button>
              <Button onClick={startExport} disabled={busy}>
                {busy && <Loader2 className='mr-2 h-4 w-4 animate-spin' />}
                {gated
                  ? t('approvals.submitForApproval', 'Submit for approval')
                  : t('exports.export')}
              </Button>
            </DialogFooter>
          </div>
        ) : job.status === 'finished' ? (
          <div className='space-y-3'>
            <div className='text-sm text-muted-foreground'>
              {t('exports.complete')}
            </div>
            <DialogFooter>
              <Button onClick={() => triggerDownload(job)}>
                <Download className='mr-2 h-4 w-4' />
                {t('exports.download')}
              </Button>
            </DialogFooter>
          </div>
        ) : job.status === 'failed' || job.status === 'cancelled' ? (
          <div className='text-sm text-destructive'>
            {job.error ?? t('exports.failed')}
          </div>
        ) : (
          <div className='space-y-3'>
            <div className='flex items-center gap-2 text-sm text-muted-foreground'>
              <Loader2 className='h-4 w-4 animate-spin' />
              {t('exports.exporting')}
            </div>
            <div className='text-xs text-muted-foreground'>
              {t('exports.progress', {
                processed: job.processed,
                total: job.total_emails,
                exported: job.exported,
                failed: job.failed,
              })}
            </div>
          </div>
        )}
      </DialogContent>
    </Dialog>
  )
}
