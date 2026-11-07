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
import {
  Download,
  Loader2,
  RefreshCw,
  ShieldCheck,
  Trash2,
  X,
  XCircle,
} from 'lucide-react'
import { useTranslation } from 'react-i18next'
import axiosInstance from '@/api/axiosInstance'
import {
  cancelExport,
  createDownloadTicket,
  deleteExport,
  getExportVerifyProgress,
  listExports,
  startExportVerify,
  type ExportJobView,
  type ExportVerifyProgressView,
} from '@/api/export/api'
import { useEdition } from '@/hooks/use-edition'
import { toast } from '@/hooks/use-toast'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Progress } from '@/components/ui/progress'
import { Separator } from '@/components/ui/separator'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import { ConfirmDialog } from '@/components/confirm-dialog'
import { FixedHeader } from '@/components/layout/fixed-header'
import { Main } from '@/components/layout/main'

const POLL_INTERVAL_MS = 3000
const VERIFY_POLL_INTERVAL_MS = 1000
const VERIFY_RESUME_KEY = 'bichon.export.verifyJobId'

const ACTIVE_STATUSES = ['pending', 'running']

const getErrorMessage = (error: unknown) => {
  if (error instanceof AxiosError) {
    return (
      (error.response?.data as { message?: string } | undefined)?.message ||
      error.message
    )
  }
  return error instanceof Error ? error.message : String(error)
}

const formatBytes = (bytes: number) => {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  if (bytes < 1024 * 1024 * 1024)
    return `${(bytes / 1024 / 1024).toFixed(1)} MB`
  return `${(bytes / 1024 / 1024 / 1024).toFixed(2)} GB`
}

const formatTime = (ts: number) => {
  const d = new Date(ts)
  const pad = (n: number) => String(n).padStart(2, '0')
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`
}
export default function ExportTasksPage() {
  const { t } = useTranslation()
  const { isPro } = useEdition()
  const [jobs, setJobs] = React.useState<ExportJobView[]>([])
  const [loading, setLoading] = React.useState(true)
  const [refreshing, setRefreshing] = React.useState(false)
  const [cancelTarget, setCancelTarget] = React.useState<ExportJobView | null>(
    null
  )
  const [deleteTarget, setDeleteTarget] = React.useState<ExportJobView | null>(
    null
  )

  const [verifyJobId, setVerifyJobId] = React.useState<string | null>(null)
  const [verifyProgress, setVerifyProgress] =
    React.useState<ExportVerifyProgressView | null>(null)
  const [verifyError, setVerifyError] = React.useState<string | null>(null)
  const pollRef = React.useRef<ReturnType<typeof setInterval> | null>(null)
  const verifyPollRef = React.useRef<ReturnType<typeof setInterval> | null>(
    null
  )

  const stopPolling = () => {
    if (pollRef.current) {
      clearInterval(pollRef.current)
      pollRef.current = null
    }
  }

  const refresh = React.useCallback(
    async (silent = false) => {
      if (!silent) {
        setLoading(true)
      } else {
        setRefreshing(true)
      }
      try {
        const data = await listExports()
        setJobs(data)
        const hasActive = data.some((job) =>
          ACTIVE_STATUSES.includes(job.status)
        )
        if (hasActive && !pollRef.current) {
          pollRef.current = setInterval(() => refresh(true), POLL_INTERVAL_MS)
        } else if (!hasActive) {
          stopPolling()
        }
      } catch (error) {
        toast({
          title: t('export_tasks.loadFailed'),
          description: getErrorMessage(error),
          variant: 'destructive',
        })
      } finally {
        setLoading(false)
        setRefreshing(false)
      }
    },
    [t]
  )

  React.useEffect(() => {
    refresh()
    return stopPolling
  }, [refresh])
  const handleDownload = (job: ExportJobView) => {
    createDownloadTicket(job.job_id)
      .then(({ url }) => {
        const base = axiosInstance.defaults.baseURL || ''
        const href = base ? `${base.replace(/\/$/, '')}/${url}` : `/${url}`
        window.open(href, '_blank')
        toast({ title: t('export_tasks.downloadStarted') })
      })
      .catch((error) => {
        toast({
          title: t('export_tasks.downloadFailed'),
          description: getErrorMessage(error),
          variant: 'destructive',
        })
      })
  }

  const handleCancel = () => {
    if (!cancelTarget) return
    cancelExport(cancelTarget.job_id)
      .then(() => {
        setCancelTarget(null)
        toast({ title: t('export_tasks.cancelStarted') })
        refresh(true)
      })
      .catch((error) => {
        toast({
          title: t('export_tasks.cancelFailed'),
          description: getErrorMessage(error),
          variant: 'destructive',
        })
      })
  }

  const handleDelete = () => {
    if (!deleteTarget) return
    deleteExport(deleteTarget.job_id)
      .then(() => {
        setDeleteTarget(null)
        toast({ title: t('export_tasks.deleted') })
        refresh(true)
      })
      .catch((error) => {
        toast({
          title: t('export_tasks.deleteFailed'),
          description: getErrorMessage(error),
          variant: 'destructive',
        })
      })
  }

  const stopVerifyPolling = () => {
    if (verifyPollRef.current) {
      clearInterval(verifyPollRef.current)
      verifyPollRef.current = null
    }
  }

  const pollVerify = React.useCallback((jobId: string) => {
    stopVerifyPolling()
    verifyPollRef.current = setInterval(() => {
      getExportVerifyProgress(jobId)
        .then((progress) => {
          setVerifyProgress(progress)
          if (progress.status === 'finished' || progress.status === 'failed') {
            stopVerifyPolling()
            localStorage.removeItem(VERIFY_RESUME_KEY)
          }
        })
        .catch((error) => {
          stopVerifyPolling()
          setVerifyError(getErrorMessage(error))
          localStorage.removeItem(VERIFY_RESUME_KEY)
        })
    }, VERIFY_POLL_INTERVAL_MS)
  }, [])

  const handleVerify = (job: ExportJobView) => {
    setVerifyJobId(job.job_id)
    setVerifyError(null)
    setVerifyProgress(null)
    localStorage.setItem(VERIFY_RESUME_KEY, job.job_id)
    startExportVerify(job.job_id)
      .then((progress) => {
        setVerifyProgress(progress)
        if (progress.status === 'running') {
          pollVerify(job.job_id)
        } else if (
          progress.status === 'finished' ||
          progress.status === 'failed'
        ) {
          localStorage.removeItem(VERIFY_RESUME_KEY)
        }
      })
      .catch((error) => {
        setVerifyError(getErrorMessage(error))
        localStorage.removeItem(VERIFY_RESUME_KEY)
      })
  }

  const closeVerifyPanel = () => {
    stopVerifyPolling()
    setVerifyJobId(null)
    setVerifyProgress(null)
    setVerifyError(null)
    localStorage.removeItem(VERIFY_RESUME_KEY)
  }

  React.useEffect(() => {
    const jobId = localStorage.getItem(VERIFY_RESUME_KEY)
    if (!jobId || !isPro) return
    setVerifyJobId(jobId)
    getExportVerifyProgress(jobId)
      .then((progress) => {
        setVerifyProgress(progress)
        if (progress.status === 'running') {
          pollVerify(jobId)
        } else {
          localStorage.removeItem(VERIFY_RESUME_KEY)
        }
      })
      .catch((error) => {
        setVerifyError(getErrorMessage(error))
        localStorage.removeItem(VERIFY_RESUME_KEY)
      })
    return stopVerifyPolling
  }, [isPro, pollVerify])

  const verifyReasonLabel = (reason: string) => {
    switch (reason) {
      case 'blob_missing':
        return t('export_tasks.verifyReasonBlobMissing')
      case 'attachment_missing':
        return t('export_tasks.verifyReasonAttachmentMissing')
      case 'content_changed':
        return t('export_tasks.verifyReasonContentChanged')
      default:
        return reason
    }
  }

  const statusInfo = (
    status: string
  ): {
    label: string
    variant: 'default' | 'secondary' | 'destructive' | 'outline'
  } => {
    switch (status) {
      case 'pending':
        return { label: t('export_tasks.statusPending'), variant: 'outline' }
      case 'running':
        return { label: t('export_tasks.statusRunning'), variant: 'default' }
      case 'finished':
        return { label: t('export_tasks.statusFinished'), variant: 'secondary' }
      case 'failed':
        return { label: t('export_tasks.statusFailed'), variant: 'destructive' }
      case 'cancelled':
        return { label: t('export_tasks.statusCancelled'), variant: 'outline' }
      default:
        return { label: status, variant: 'outline' }
    }
  }

  const renderProgress = (job: ExportJobView) => {
    if (job.status === 'finished') {
      return (
        <span className='text-xs text-muted-foreground'>
          {job.exported}/{job.total_emails}
        </span>
      )
    }
    if (job.status === 'failed' || job.status === 'cancelled') {
      return job.error ? (
        <span
          className='block max-w-[220px] truncate text-xs text-destructive'
          title={job.error}
        >
          {job.error}
        </span>
      ) : (
        <span className='text-xs text-muted-foreground'>-</span>
      )
    }
    const pct =
      job.total_emails > 0
        ? Math.min(100, Math.round((job.processed / job.total_emails) * 100))
        : 0
    return (
      <div className='flex w-full max-w-[180px] flex-col gap-1'>
        <Progress value={pct} className='h-1.5' />
        <span className='text-xs text-muted-foreground'>
          {t('export_tasks.progress', {
            processed: job.processed,
            total: job.total_emails,
            exported: job.exported,
            failed: job.failed,
          })}
        </span>
      </div>
    )
  }
  return (
    <>
      <FixedHeader />
      <Main>
        <div className='mx-auto w-full max-w-7xl px-4'>
          <div className='mb-4 flex items-center justify-between'>
            <h1 className='text-lg font-semibold'>{t('export_tasks.title')}</h1>
            <Button
              variant='outline'
              size='sm'
              onClick={() => refresh(true)}
              disabled={refreshing}
            >
              {refreshing ? (
                <Loader2 className='mr-2 h-4 w-4 animate-spin' />
              ) : (
                <RefreshCw className='mr-2 h-4 w-4' />
              )}
              {t('export_tasks.refresh')}
            </Button>
          </div>
          <Separator className='mt-2 mb-4 lg:mt-3 lg:mb-6' />

          <div className='overflow-x-auto rounded-md border'>
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead className='text-xs'>
                    {t('export_tasks.name')}
                  </TableHead>
                  <TableHead className='text-xs'>
                    {t('export_tasks.status')}
                  </TableHead>
                  <TableHead className='text-xs'>
                    {t('export_tasks.progressLabel')}
                  </TableHead>
                  <TableHead className='text-xs'>
                    {t('export_tasks.items')}
                  </TableHead>
                  <TableHead className='text-xs'>
                    {t('export_tasks.size')}
                  </TableHead>
                  <TableHead className='text-xs'>
                    {t('export_tasks.created')}
                  </TableHead>
                  <TableHead className='text-xs'>
                    {t('export_tasks.actions')}
                  </TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {loading ? (
                  <TableRow>
                    <TableCell
                      colSpan={7}
                      className='py-8 text-center text-muted-foreground'
                    >
                      <Loader2 className='mx-auto h-5 w-5 animate-spin' />
                    </TableCell>
                  </TableRow>
                ) : jobs.length === 0 ? (
                  <TableRow>
                    <TableCell
                      colSpan={7}
                      className='py-8 text-center text-muted-foreground'
                    >
                      {t('export_tasks.empty')}
                    </TableCell>
                  </TableRow>
                ) : (
                  jobs.map((job) => {
                    const info = statusInfo(job.status)
                    return (
                      <TableRow key={job.job_id}>
                        <TableCell className='text-xs font-medium'>
                          <div
                            className='max-w-[200px] truncate'
                            title={job.saved_search_name}
                          >
                            {job.saved_search_name}
                          </div>
                        </TableCell>
                        <TableCell>
                          <Badge variant={info.variant}>{info.label}</Badge>
                        </TableCell>
                        <TableCell>{renderProgress(job)}</TableCell>
                        <TableCell className='text-xs'>
                          {job.total_emails.toLocaleString()}
                        </TableCell>
                        <TableCell className='text-xs'>
                          {formatBytes(job.total_size)}
                        </TableCell>
                        <TableCell className='whitespace-nowrap text-xs'>
                          {formatTime(job.created_at)}
                        </TableCell>
                        <TableCell>
                          <div className='flex items-center gap-1'>
                            {job.status === 'finished' && (
                              <Button
                                variant='ghost'
                                size='icon'
                                className='h-7 w-7'
                                title={t('export_tasks.download')}
                                onClick={() => handleDownload(job)}
                              >
                                <Download className='h-3.5 w-3.5' />
                              </Button>
                            )}

                            {isPro && job.status === 'finished' && (
                              <Button
                                variant='ghost'
                                size='icon'
                                className='h-7 w-7'
                                title={t('export_tasks.verify')}
                                disabled={
                                  verifyJobId === job.job_id &&
                                  verifyProgress?.status === 'running'
                                }
                                onClick={() => handleVerify(job)}
                              >
                                {verifyJobId === job.job_id &&
                                verifyProgress?.status === 'running' ? (
                                  <Loader2 className='h-3.5 w-3.5 animate-spin' />
                                ) : (
                                  <ShieldCheck className='h-3.5 w-3.5' />
                                )}
                              </Button>
                            )}
                            {(job.status === 'pending' ||
                              job.status === 'running') && (
                              <Button
                                variant='ghost'
                                size='icon'
                                className='h-7 w-7 text-muted-foreground hover:text-destructive'
                                title={t('export_tasks.cancel')}
                                onClick={() => setCancelTarget(job)}
                              >
                                <XCircle className='h-3.5 w-3.5' />
                              </Button>
                            )}
                            <Button
                              variant='ghost'
                              size='icon'
                              className='h-7 w-7 text-muted-foreground hover:text-destructive'
                              title={t('export_tasks.delete')}
                              onClick={() => setDeleteTarget(job)}
                            >
                              <Trash2 className='h-3.5 w-3.5' />
                            </Button>
                          </div>
                        </TableCell>
                      </TableRow>
                    )
                  })
                )}
              </TableBody>
            </Table>
          </div>
        </div>
      </Main>

      <ConfirmDialog
        open={cancelTarget !== null}
        onOpenChange={(isOpen) => !isOpen && setCancelTarget(null)}
        title={t('export_tasks.cancelTitle')}
        desc={t('export_tasks.cancelDesc', {
          name: cancelTarget?.saved_search_name ?? '',
        })}
        confirmText={t('export_tasks.cancelConfirm')}
        handleConfirm={handleCancel}
      />
      <ConfirmDialog
        open={deleteTarget !== null}
        onOpenChange={(isOpen) => !isOpen && setDeleteTarget(null)}
        title={t('export_tasks.deleteTitle')}
        desc={t('export_tasks.deleteDesc', {
          name: deleteTarget?.saved_search_name ?? '',
        })}
        confirmText={t('export_tasks.deleteConfirm')}
        destructive
        handleConfirm={handleDelete}
      />

      {verifyJobId && (
        <div className='mx-auto w-full max-w-7xl px-4 pb-4'>
          <div className='mt-4 rounded-md border bg-muted/40 p-4'>
            <div className='mb-3 flex items-center justify-between'>
              <div className='flex items-center gap-2'>
                {verifyProgress?.status === 'running' ? (
                  <Loader2 className='h-4 w-4 animate-spin text-muted-foreground' />
                ) : verifyProgress?.result?.artifact_hash_match ? (
                  <ShieldCheck className='h-4 w-4 text-emerald-600' />
                ) : (
                  <XCircle className='h-4 w-4 text-destructive' />
                )}
                <span className='text-sm font-medium'>
                  {t('export_tasks.verifyTitle')}
                </span>
              </div>
              <Button
                variant='ghost'
                size='icon'
                className='h-7 w-7'
                title={t('export_tasks.verifyClose')}
                onClick={closeVerifyPanel}
              >
                <X className='h-3.5 w-3.5' />
              </Button>
            </div>

            {verifyError ? (
              <div className='text-sm text-destructive'>{verifyError}</div>
            ) : verifyProgress?.status === 'running' ? (
              <div className='space-y-2'>
                <Progress
                  value={
                    verifyProgress.total > 0
                      ? Math.min(
                          100,
                          Math.round(
                            (verifyProgress.checked / verifyProgress.total) *
                              100
                          )
                        )
                      : 0
                  }
                  className='h-2'
                />
                <div className='flex flex-wrap gap-x-4 gap-y-1 text-xs text-muted-foreground'>
                  <span>
                    {t('export_tasks.verifyProgress', {
                      checked: verifyProgress.checked,
                      total: verifyProgress.total,
                    })}
                  </span>
                  <span>
                    {t('export_tasks.verifyMatched')}: {verifyProgress.matched}
                  </span>
                  <span>
                    {t('export_tasks.verifyMismatched')}:{' '}
                    {verifyProgress.mismatched}
                  </span>
                </div>
              </div>
            ) : verifyProgress?.result ? (
              <div className='space-y-2 text-sm'>
                <div className='flex items-center gap-2'>
                  {verifyProgress.result.artifact_hash_match ? (
                    <ShieldCheck className='h-4 w-4 text-emerald-600' />
                  ) : (
                    <XCircle className='h-4 w-4 text-destructive' />
                  )}
                  <span className='font-medium'>
                    {t('export_tasks.verifyHashMatch')}
                  </span>
                  <span>
                    {verifyProgress.result.artifact_hash_match
                      ? t('export_tasks.verifyPassed')
                      : t('export_tasks.verifyFailed')}
                  </span>
                </div>
                <div className='flex gap-4'>
                  <div>
                    <span className='text-muted-foreground'>
                      {t('export_tasks.verifyChecked')}:
                    </span>{' '}
                    {verifyProgress.result.checked}
                  </div>
                  <div>
                    <span className='text-muted-foreground'>
                      {t('export_tasks.verifyMatched')}:
                    </span>{' '}
                    {verifyProgress.result.matched}
                  </div>
                  <div>
                    <span className='text-muted-foreground'>
                      {t('export_tasks.verifyMismatched')}:
                    </span>{' '}
                    {verifyProgress.result.mismatched}
                  </div>
                </div>
                {verifyProgress.result.actual_artifact_hash && (
                  <div className='break-all font-mono text-xs text-muted-foreground'>
                    {t('export_tasks.verifyHash')}:{' '}
                    {verifyProgress.result.actual_artifact_hash}
                  </div>
                )}
                {verifyProgress.result.mismatches.length > 0 && (
                  <div className='max-h-40 space-y-1 overflow-y-auto rounded-md border p-2'>
                    {verifyProgress.result.mismatches.slice(0, 20).map((m) => (
                      <div
                        key={m.envelope_id}
                        className='text-xs text-destructive'
                      >
                        {m.envelope_id}
                        {m.subject ? ` \u2014 ${m.subject}` : ''}
                        <span className='text-muted-foreground'>
                          {' '}
                          ({verifyReasonLabel(m.reason)})
                        </span>
                      </div>
                    ))}
                    {verifyProgress.result.mismatches.length > 20 && (
                      <div className='text-xs text-muted-foreground'>
                        {t('export_tasks.verifyMore', {
                          count: verifyProgress.result.mismatches.length - 20,
                        })}
                      </div>
                    )}
                  </div>
                )}
              </div>
            ) : (
              <div className='text-sm text-destructive'>
                {verifyProgress?.error || t('export_tasks.verifyError')}
              </div>
            )}
          </div>
        </div>
      )}
    </>
  )
}
