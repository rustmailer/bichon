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
  AlertTriangle,
  Archive,
  ChevronLeft,
  ChevronRight,
  History,
  Loader2,
  Play,
  RefreshCw,
  RotateCcw,
  ShieldCheck,
} from 'lucide-react'
import { useTranslation } from 'react-i18next'
import {
  getBackupStatus,
  getLatestBackupDrill,
  listBackupRecords,
  requestBackupRebase,
  triggerBackup,
  type ArtifactStat,
  type BackupRecord,
  type BackupStatusView,
  type DrillReport,
} from '@/api/backup/api'
import { useEdition } from '@/hooks/use-edition'
import { toast } from '@/hooks/use-toast'
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Separator } from '@/components/ui/separator'
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from '@/components/ui/sheet'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import { FixedHeader } from '@/components/layout/fixed-header'
import { Main } from '@/components/layout/main'
import { BackupConfigForm } from '@/features/backup/components/BackupConfigForm'
import ManifestBrowser from '@/features/backup/components/ManifestBrowser'

const POLL_INTERVAL_MS = 3000
/** Consecutive idle polls tolerated while polling before concluding the run
 *  is over — the trigger response can race the server-side run spawn and
 *  still report `running=false`. */
const MAX_IDLE_POLLS = 10
/** The server keeps at most this many run records (BackupRecord::HISTORY_LIMIT)
 *  and the records API is newest-first with only a `limit` — the drawer
 *  fetches them all and pages client-side. */
const HISTORY_LIMIT = 200
const HISTORY_PAGE_SIZE = 10

const getErrorMessage = (error: unknown) => {
  if (error instanceof AxiosError) {
    return (
      (error.response?.data as { message?: string } | undefined)?.message ||
      error.message
    )
  }
  return error instanceof Error ? error.message : String(error)
}

const formatTime = (ts: number | null) => {
  if (ts === null) return '—'
  const d = new Date(ts)
  const pad = (n: number) => String(n).padStart(2, '0')
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`
}

const formatBytes = (bytes: number) => {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  if (bytes < 1024 * 1024 * 1024)
    return `${(bytes / 1024 / 1024).toFixed(1)} MB`
  return `${(bytes / 1024 / 1024 / 1024).toFixed(2)} GB`
}

const drillStats = (drill: DrillReport) => ({
  checked: drill.bytes_checked ? drill.objects_total : 0,
  total: drill.objects_total,
  bytes: formatBytes(drill.bytes_checked || drill.bytes_total),
})

const KIND_I18N: Record<string, string> = {
  memdb: 'backup.kind.memdb',
  'imap-uid': 'backup.kind.imapUid',
  blob: 'backup.kind.blob',
  'envelope-index': 'backup.kind.envelopeIndex',
  'attachment-index': 'backup.kind.attachmentIndex',
  audit: 'backup.kind.audit',
  integrity: 'backup.kind.integrity',
  timestamp: 'backup.kind.timestamp',
}

/** Which parts of the archive were uploaded vs skipped in one run — e.g.
 *  "邮件存储 +308 MB · 数据库 跳过 2". */
function TransferBreakdown({ byKind }: { byKind: ArtifactStat[] }) {
  const { t } = useTranslation()
  if (!byKind || byKind.length === 0) return null
  const parts = byKind
    .filter((s) => s.new_count > 0 || s.skipped_count > 0)
    .map((s) => {
      const bits: string[] = []
      if (s.new_count > 0) {
        bits.push(
          `+${formatBytes(s.new_bytes)}${s.new_count > 1 ? ` ×${s.new_count}` : ''}`
        )
      }
      if (s.skipped_count > 0) {
        // Deduped objects still have a size — show it, or an unchanged index
        // looks like it was not counted at all.
        const size =
          s.skipped_bytes > 0 ? ` (${formatBytes(s.skipped_bytes)})` : ''
        bits.push(
          `${t('backup.skippedObjects', 'skipped')} ${s.skipped_count}${size}`
        )
      }
      return `${t(KIND_I18N[s.kind] ?? s.kind)} ${bits.join(' ')}`
    })
  if (parts.length === 0) return null
  const text = parts.join(' · ')
  return (
    <div className='truncate text-xs text-muted-foreground' title={text}>
      {text}
    </div>
  )
}

export default function BackupPage() {
  const { t } = useTranslation()
  const { isPro } = useEdition()
  const [status, setStatus] = React.useState<BackupStatusView | null>(null)
  const [records, setRecords] = React.useState<BackupRecord[]>([])
  const [drill, setDrill] = React.useState<DrillReport | null>(null)
  const [loading, setLoading] = React.useState(true)
  const [refreshing, setRefreshing] = React.useState(false)
  const [triggering, setTriggering] = React.useState(false)
  const [rebasing, setRebasing] = React.useState(false)
  const [historyOpen, setHistoryOpen] = React.useState(false)
  const [historyPage, setHistoryPage] = React.useState(1)
  /** Set when a run finishes successfully, so the restore-point list can be
   *  flagged as out of date. The list is not refetched automatically: listing
   *  the backup target costs a request per call (billable on S3), so the user
   *  decides when to refresh via the browser's refresh button. */
  const [manifestsStale, setManifestsStale] = React.useState(false)
  const pollRef = React.useRef<ReturnType<typeof setInterval> | null>(null)
  const idlePollsRef = React.useRef(0)
  /** Id of the newest successful run already reflected in the restore-point
   *  list (`''` when that list was fetched against an empty history). A new
   *  successful run commits a restore point, so its record id changing is the
   *  signal to flag the list as stale. */
  const acknowledgedSuccessRef = React.useRef<string | null>(null)

  const stopPolling = () => {
    if (pollRef.current) {
      clearInterval(pollRef.current)
      pollRef.current = null
    }
    idlePollsRef.current = 0
  }

  const refresh = React.useCallback(
    async (silent = false) => {
      if (!silent) {
        setLoading(true)
      } else {
        setRefreshing(true)
      }
      try {
        const [statusData, recordsData] = await Promise.all([
          getBackupStatus(),
          listBackupRecords(HISTORY_LIMIT),
        ])
        setStatus(statusData)
        setRecords(recordsData)
        // A successful run commits a restore point. Flag the list as stale
        // instead of refetching it — each fetch is a list call against the
        // backup target (billable on S3), so the refresh is left to the user.
        // Comparing record ids also catches runs that start and finish
        // between two polls.
        const latestSuccess = recordsData.find((r) => r.status === 'success')
        if (acknowledgedSuccessRef.current === null) {
          // First load: whatever is already committed is in the mounted list.
          acknowledgedSuccessRef.current = latestSuccess?.id ?? ''
        } else if (
          latestSuccess &&
          latestSuccess.id !== acknowledgedSuccessRef.current
        ) {
          acknowledgedSuccessRef.current = latestSuccess.id
          setManifestsStale(true)
        }
        if (statusData.running) {
          idlePollsRef.current = 0
          if (!pollRef.current) {
            pollRef.current = setInterval(() => refresh(true), POLL_INTERVAL_MS)
          }
        } else if (pollRef.current) {
          // A poll racing the server-side run spawn can still observe
          // running=false right after a trigger; tolerate a few idle polls
          // before concluding the run is over.
          idlePollsRef.current += 1
          if (idlePollsRef.current > MAX_IDLE_POLLS) {
            stopPolling()
            idlePollsRef.current = 0
          }
        }
        if (isPro) {
          getLatestBackupDrill()
            .then(({ report }) => setDrill(report))
            .catch(() => {})
        }
      } catch (error) {
        // Polls stay silent: a transient failure during a running backup
        // must not toast every interval. User-initiated refreshes report.
        if (!silent) {
          toast({
            title: t('backup.loadFailed'),
            description: getErrorMessage(error),
            variant: 'destructive',
          })
        }
      } finally {
        setLoading(false)
        setRefreshing(false)
      }
    },
    [t, isPro]
  )

  React.useEffect(() => {
    refresh()
    return stopPolling
  }, [refresh])

  const handleTrigger = () => {
    setTriggering(true)
    triggerBackup()
      .then((data) => {
        setStatus(data)
        toast({ title: t('backup.triggerStarted') })
        // The server spawns the run asynchronously — the 202 body can still
        // report running=false. Start polling unconditionally; a poll that
        // observes running=false for MAX_IDLE_POLLS consecutive rounds stops
        // it again.
        idlePollsRef.current = 0
        if (!pollRef.current) {
          pollRef.current = setInterval(() => refresh(true), POLL_INTERVAL_MS)
        }
        refresh(true)
      })
      .catch((error) => {
        toast({
          title: t('backup.triggerFailed'),
          description: getErrorMessage(error),
          variant: 'destructive',
        })
      })
      .finally(() => setTriggering(false))
  }

  const handleRebase = () => {
    setRebasing(true)
    requestBackupRebase()
      .then(() => {
        toast({
          title: t('backup.rebaseRequested', 'Rebase requested'),
          description: t(
            'backup.rebaseHint',
            'The next backup run produces a fresh audit baseline and clears the delta chain.'
          ),
        })
      })
      .catch((error) => {
        toast({
          title: t('backup.rebaseFailed', 'Failed to request rebase'),
          description: getErrorMessage(error),
          variant: 'destructive',
        })
      })
      .finally(() => setRebasing(false))
  }

  const historyTotalPages = Math.max(
    1,
    Math.ceil(records.length / HISTORY_PAGE_SIZE)
  )
  const safeHistoryPage = Math.min(historyPage, historyTotalPages)

  const statusInfo = (
    s: string
  ): {
    label: string
    variant: 'default' | 'secondary' | 'destructive' | 'outline'
  } => {
    switch (s) {
      case 'success':
        return { label: t('backup.statusSuccess'), variant: 'secondary' }
      case 'failed':
        return { label: t('backup.statusFailed'), variant: 'destructive' }
      case 'running':
        return { label: t('backup.statusRunning'), variant: 'default' }
      default:
        return { label: s, variant: 'outline' }
    }
  }

  return (
    <>
      <FixedHeader />
      <Main>
        <div className='mx-auto w-full max-w-7xl px-4'>
          <div className='mb-4 flex items-center justify-between'>
            <h1 className='flex items-center gap-2 text-lg font-semibold'>
              <Archive className='h-5 w-5' />
              {t('backup.title')}
            </h1>
            <div className='flex items-center gap-2'>
              <Button
                onClick={handleTrigger}
                disabled={triggering || status?.running || !status?.enabled}
                className='text-xs'
              >
                {triggering || status?.running ? (
                  <Loader2 className='mr-2 h-4 w-4 animate-spin' />
                ) : (
                  <Play className='mr-2 h-4 w-4' />
                )}
                {t('backup.runNow')}
              </Button>
              <Button
                variant='outline'
                onClick={() => {
                  setHistoryPage(1)
                  setHistoryOpen(true)
                }}
                disabled={loading}
                className='text-xs'
              >
                <History className='mr-2 h-4 w-4' />
                {t('backup.history', 'Backup history')}
                {records.length > 0 && (
                  <Badge variant='secondary' className='ml-2'>
                    {records.length}
                  </Badge>
                )}
              </Button>
              <Button
                variant='ghost'
                size='icon'
                onClick={() => refresh(true)}
                disabled={refreshing}
                title={t('backup.refresh')}
              >
                {refreshing ? (
                  <Loader2 className='h-4 w-4 animate-spin' />
                ) : (
                  <RefreshCw className='h-4 w-4' />
                )}
              </Button>
            </div>
          </div>
          <Separator className='mt-2 mb-4 lg:mt-3 lg:mb-6' />

          {loading && !status ? (
            <div className='flex items-center justify-center py-10'>
              <Loader2 className='h-5 w-5 animate-spin text-muted-foreground' />
            </div>
          ) : (
            <>
              {status && !status.enabled && (
                <Alert variant='destructive' className='mb-4'>
                  <AlertTriangle className='h-4 w-4' />
                  <AlertTitle>{t('backup.disabledTitle')}</AlertTitle>
                  <AlertDescription>
                    {t('backup.disabledWarning')}
                  </AlertDescription>
                </Alert>
              )}

              {/* Metric cards — glanceable status at the top, one fact each. */}
              <div className='grid grid-cols-2 gap-4 lg:grid-cols-4'>
                <Card>
                  <CardHeader className='pb-2'>
                    <CardTitle className='text-xs font-medium text-muted-foreground'>
                      {t('backup.statusCard')}
                    </CardTitle>
                  </CardHeader>
                  <CardContent className='space-y-1.5 text-xs'>
                    <div className='flex items-center gap-2'>
                      <Badge
                        variant={status?.enabled ? 'secondary' : 'destructive'}
                      >
                        {status?.enabled
                          ? t('backup.enabled')
                          : t('backup.disabled')}
                      </Badge>
                      {status?.running && (
                        <Loader2 className='h-4 w-4 animate-spin text-primary' />
                      )}
                    </div>
                    {status?.running ? (
                      <div
                        className='truncate text-xs text-muted-foreground'
                        title={`${t('backup.phase')}: ${status.phase}`}
                      >
                        {t('backup.statusRunning')} · {t('backup.phase')}:{' '}
                        {status.phase}
                      </div>
                    ) : (
                      <div className='truncate text-xs text-muted-foreground'>
                        {t('backup.lastSuccess')}:{' '}
                        {status?.last_success_at
                          ? formatTime(status.last_success_at)
                          : t('backup.never')}
                      </div>
                    )}
                    {status?.last_error && (
                      <div
                        className='truncate text-xs text-destructive'
                        title={status.last_error}
                      >
                        {status.last_error}
                      </div>
                    )}
                    {status?.target && (
                      <div
                        className='truncate text-xs text-muted-foreground'
                        title={status.target}
                      >
                        {t('backup.statusTarget', 'Saving to')}: {status.target}
                      </div>
                    )}
                  </CardContent>
                </Card>

                <Card>
                  <CardHeader className='pb-2'>
                    <CardTitle className='text-xs font-medium text-muted-foreground'>
                      {t('backup.nextRun')}
                    </CardTitle>
                  </CardHeader>
                  <CardContent className='space-y-1.5 text-xs'>
                    <div className='truncate'>
                      {status?.next_run_at
                        ? formatTime(Date.parse(status.next_run_at))
                        : t('backup.never')}
                    </div>
                    {status?.schedule && (
                      <div
                        className='truncate text-xs text-muted-foreground'
                        title={status.schedule}
                      >
                        <code className='rounded bg-muted px-1.5 py-0.5 text-xs'>
                          {status.schedule}
                        </code>
                      </div>
                    )}
                  </CardContent>
                </Card>

                <Card>
                  <CardHeader className='pb-2'>
                    <CardTitle className='flex items-center justify-between gap-1 text-xs font-medium text-muted-foreground'>
                      <span className='truncate'>
                        {t('backup.lastRestorePoint')}
                      </span>
                      {isPro && status?.enabled && (
                        <Button
                          variant='ghost'
                          size='sm'
                          className='h-6 shrink-0 gap-1 px-1.5 text-xs'
                          onClick={handleRebase}
                          disabled={rebasing || status?.running}
                          title={t(
                            'backup.rebaseHint',
                            'The next backup run produces a fresh audit baseline and clears the delta chain.'
                          )}
                        >
                          {rebasing ? (
                            <Loader2 className='h-3 w-3 animate-spin' />
                          ) : (
                            <RotateCcw className='h-3 w-3' />
                          )}
                          {t('backup.rebase', 'Rebase audit chain')}
                        </Button>
                      )}
                    </CardTitle>
                  </CardHeader>
                  <CardContent className='space-y-1.5 text-xs'>
                    <div className='truncate'>
                      {status?.last_manifest_id ? (
                        <code
                          className='rounded bg-muted px-1.5 py-0.5 text-xs'
                          title={status.last_manifest_id}
                        >
                          {status.last_manifest_id}
                        </code>
                      ) : (
                        t('backup.none')
                      )}
                    </div>
                    <div className='truncate text-xs text-muted-foreground'>
                      {status?.last_success_at
                        ? formatTime(status.last_success_at)
                        : t('backup.never')}
                    </div>
                    {status?.last_uploaded_bytes !== null &&
                      status?.last_uploaded_bytes !== undefined && (
                        <div
                          className='truncate text-xs text-muted-foreground'
                          title={`${t('backup.uploaded')}: ${formatBytes(status.last_uploaded_bytes)} · ${status.last_new_objects ?? 0} ${t('backup.newObjects', 'new')} · ${status.last_skipped_objects ?? 0} ${t('backup.skippedObjects', 'skipped')}`}
                        >
                          {formatBytes(status.last_uploaded_bytes)}
                          {status.last_new_objects !== null &&
                            ` · ${status.last_new_objects} ${t(
                              'backup.newObjects',
                              'new'
                            )}`}
                          {status.last_skipped_objects !== null &&
                            ` · ${status.last_skipped_objects} ${t(
                              'backup.skippedObjects',
                              'skipped'
                            )}`}
                        </div>
                      )}
                  </CardContent>
                </Card>

                {isPro && drill && (
                  <Card>
                    <CardHeader className='pb-2'>
                      <CardTitle className='text-xs font-medium text-muted-foreground'>
                        {t('backup.drillCard', 'Restore drills')}
                      </CardTitle>
                    </CardHeader>
                    <CardContent className='space-y-1.5 text-xs'>
                      <div className='flex items-center gap-2'>
                        <ShieldCheck
                          className={`h-4 w-4 ${
                            drill.passed
                              ? 'text-emerald-500'
                              : 'text-destructive'
                          }`}
                        />
                        <Badge
                          variant={drill.passed ? 'secondary' : 'destructive'}
                        >
                          {drill.passed
                            ? t('backup.statusSuccess')
                            : t('backup.statusFailed')}
                        </Badge>
                      </div>
                      <div className='truncate text-xs text-muted-foreground'>
                        {formatTime(drill.ran_at)}
                      </div>
                      <div
                        className='truncate text-xs text-muted-foreground'
                        title={t('backup.drillStats', drillStats(drill))}
                      >
                        {t('backup.drillStats', drillStats(drill))}
                      </div>
                      {drill.error && (
                        <div
                          className='truncate text-xs text-destructive'
                          title={drill.error}
                        >
                          {drill.error}
                        </div>
                      )}
                    </CardContent>
                  </Card>
                )}
              </div>

              {/* Config (left) and restore points (right), side by side. */}
              <div className='mt-4 grid grid-cols-1 gap-4 lg:grid-cols-5'>
                <div className={status?.enabled ? 'lg:col-span-3' : 'lg:col-span-5'}>
                  <BackupConfigForm onSaved={() => refresh(true)} />
                </div>

                {status?.enabled && (
                  <Card className='lg:col-span-2'>
                    <CardHeader className='pb-2'>
                      <CardTitle className='text-xs font-medium'>
                        {t('backup.manifestBrowser', 'Restore points')}
                      </CardTitle>
                    </CardHeader>
                    <CardContent>
                      <ManifestBrowser
                        stacked
                        stale={manifestsStale}
                        onRefreshed={() => setManifestsStale(false)}
                      />
                    </CardContent>
                  </Card>
                )}
              </div>
            </>
          )}
        </div>
      </Main>

      {/* Backup history lives in a drawer to keep the page itself compact. */}
      <Sheet open={historyOpen} onOpenChange={setHistoryOpen}>
        <SheetContent
          side='right'
          className='flex w-full flex-col gap-0 overflow-hidden sm:max-w-4xl'
        >
          <SheetHeader className='pb-2'>
            <SheetTitle>{t('backup.records')}</SheetTitle>
            <SheetDescription className='text-xs'>
              {t('backup.recordsHint', 'The most recent 200 backup runs.')}
            </SheetDescription>
          </SheetHeader>
          <div className='flex-1 overflow-y-auto px-4 pb-6'>
            <div className='overflow-x-auto rounded-md border'>
              <Table className='text-xs'>
                <TableHeader>
                  <TableRow>
                    <TableHead className='text-xs'>
                      {t('backup.time', 'Time')}
                    </TableHead>
                    <TableHead className='text-xs'>
                      {t('backup.trigger')}
                    </TableHead>
                    <TableHead className='text-xs'>
                      {t('backup.status')}
                    </TableHead>
                    <TableHead className='text-xs'>
                      {t('backup.restorePoint', 'Restore point')}
                    </TableHead>
                    <TableHead className='text-xs'>
                      {t('backup.transfer', 'Transferred')}
                    </TableHead>
                    <TableHead className='text-xs'>
                      {t('backup.error')}
                    </TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {records.length === 0 ? (
                    <TableRow>
                      <TableCell
                        colSpan={6}
                        className='py-8 text-center text-muted-foreground'
                      >
                        {t('backup.recordsEmpty')}
                      </TableCell>
                    </TableRow>
                  ) : (
                    records
                      .slice(
                        (safeHistoryPage - 1) * HISTORY_PAGE_SIZE,
                        safeHistoryPage * HISTORY_PAGE_SIZE
                      )
                      .map((record) => {
                        const info = statusInfo(record.status)
                        // The history is one list across targets, so a record
                        // from a target the user has since switched away from
                        // would otherwise look like it belongs to the current
                        // one. Only flag it once the current target is known.
                        const offTarget =
                          !!status?.target && record.target !== status.target
                        return (
                          <TableRow key={record.id}>
                            <TableCell className='whitespace-nowrap text-xs'>
                              {formatTime(record.started_at)}
                            </TableCell>
                            <TableCell className='text-xs'>
                              <div>{record.trigger}</div>
                              {record.target && (
                                <div className='mt-0.5 flex items-center gap-1'>
                                  <span
                                    className={`max-w-[150px] truncate text-xs ${
                                      offTarget
                                        ? 'text-amber-600 dark:text-amber-500'
                                        : 'text-muted-foreground'
                                    }`}
                                    title={record.target}
                                  >
                                    {record.target}
                                  </span>
                                  {offTarget && (
                                    <span className='shrink-0 text-xs text-amber-600 dark:text-amber-500'>
                                      {t(
                                        'backup.otherTarget',
                                        'different target'
                                      )}
                                    </span>
                                  )}
                                </div>
                              )}
                            </TableCell>
                            <TableCell>
                              <Badge variant={info.variant}>{info.label}</Badge>
                            </TableCell>
                            <TableCell>
                              {record.summary?.manifest_id ? (
                                <div>
                                  <code className='rounded bg-muted px-1.5 py-0.5 text-xs'>
                                    {record.summary.manifest_id}
                                  </code>
                                  <div className='mt-0.5 text-xs text-muted-foreground'>
                                    {formatTime(
                                      record.finished_at ?? record.started_at
                                    )}
                                  </div>
                                </div>
                              ) : (
                                <span className='text-xs text-muted-foreground'>
                                  {t('backup.none')}
                                </span>
                              )}
                            </TableCell>
                            <TableCell className='text-xs text-muted-foreground'>
                              {record.status === 'success' && record.summary ? (
                                <div className='max-w-[240px] text-right'>
                                  <div
                                    className='truncate'
                                    title={`${record.summary.new_objects} ${t('backup.newObjects', 'new')} · ${record.summary.skipped_objects} ${t('backup.skippedObjects', 'skipped')} · ${formatBytes(record.summary.uploaded_bytes)}`}
                                  >
                                    {record.summary.new_objects}{' '}
                                    {t('backup.newObjects', 'new')} ·{' '}
                                    {record.summary.skipped_objects}{' '}
                                    {t('backup.skippedObjects', 'skipped')} ·{' '}
                                    {formatBytes(record.summary.uploaded_bytes)}
                                  </div>
                                  <TransferBreakdown
                                    byKind={record.summary.by_kind}
                                  />
                                </div>
                              ) : (
                                <span>-</span>
                              )}
                            </TableCell>
                            <TableCell>
                              {record.error ? (
                                <span
                                  className='block max-w-[220px] truncate text-xs text-destructive'
                                  title={record.error}
                                >
                                  {record.error}
                                </span>
                              ) : (
                                <span className='text-xs text-muted-foreground'>
                                  -
                                </span>
                              )}
                            </TableCell>
                          </TableRow>
                        )
                      })
                  )}
                </TableBody>
              </Table>
            </div>
            {records.length > HISTORY_PAGE_SIZE && (
              <div className='mt-3 flex items-center justify-end gap-2 text-xs text-muted-foreground'>
                <span>
                  {safeHistoryPage} / {historyTotalPages}
                </span>
                <Button
                  variant='outline'
                  size='icon'
                  className='h-7 w-7'
                  disabled={safeHistoryPage <= 1}
                  onClick={() => setHistoryPage((p) => Math.max(1, p - 1))}
                  title={t('backup.prevPage', 'Previous page')}
                >
                  <ChevronLeft className='h-4 w-4' />
                </Button>
                <Button
                  variant='outline'
                  size='icon'
                  className='h-7 w-7'
                  disabled={safeHistoryPage >= historyTotalPages}
                  onClick={() =>
                    setHistoryPage((p) => Math.min(historyTotalPages, p + 1))
                  }
                  title={t('backup.nextPage', 'Next page')}
                >
                  <ChevronRight className='h-4 w-4' />
                </Button>
              </div>
            )}
          </div>
        </SheetContent>
      </Sheet>
    </>
  )
}
