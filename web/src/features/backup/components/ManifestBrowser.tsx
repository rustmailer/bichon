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
import { saveAs } from 'file-saver'
import {
  Archive,
  Download,
  Loader2,
  RefreshCw,
  ShieldCheck,
  SplitSquareVertical,
  TableProperties,
  Trash2,
} from 'lucide-react'
import { useTranslation } from 'react-i18next'
import {
  deleteBackupManifest,
  downloadAuditDelta,
  getBackupManifest,
  listBackupManifests,
  runBackupDrill,
  type DrillReport,
  type ManifestView,
  type ObjectView,
} from '@/api/backup/api'
import { useEdition } from '@/hooks/use-edition'
import { toast } from '@/hooks/use-toast'
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from '@/components/ui/alert-dialog'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { ScrollArea } from '@/components/ui/scroll-area'
import { Separator } from '@/components/ui/separator'

/** Minimum spacing between user-triggered list refreshes. Listing restore
 *  points is a request against the backup target (billable on S3), so a burst
 *  of clicks must not turn into a burst of list calls. */
const MANUAL_REFRESH_COOLDOWN_MS = 10_000

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

const drillStats = (drill: DrillReport) => ({
  checked: drill.bytes_checked ? drill.objects_total : 0,
  total: drill.objects_total,
  bytes: formatBytes(drill.bytes_checked || drill.bytes_total),
})

const formatTime = (ts: string | number) => {
  const d = typeof ts === 'number' ? new Date(ts) : new Date(Date.parse(ts))
  const pad = (n: number) => String(n).padStart(2, '0')
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`
}

const shortKey = (key: string) => {
  // obj/sha256/<hex> → the first 12 hex chars
  const hex = key.split('/').pop() ?? key
  return hex.slice(0, 12)
}

interface ObjectRowProps {
  label: string
  object: ObjectView
}

function ObjectRow({ label, object }: ObjectRowProps) {
  return (
    <div className='flex items-center justify-between gap-2 border-b px-3 py-1.5 text-xs last:border-0'>
      <span className='shrink-0 font-medium text-muted-foreground'>
        {label}
      </span>
      <code
        className='min-w-0 truncate rounded bg-muted px-1.5 py-0.5'
        title={object.key}
      >
        {shortKey(object.key)}
      </code>
      <span className='shrink-0 text-muted-foreground'>
        {formatBytes(object.bytes)}
      </span>
    </div>
  )
}

interface ManifestBrowserProps {
  /** Compact mode for the page's side column: the list and detail stack
   *  vertically instead of side by side, and the picker stretches. */
  stacked?: boolean
  /** True when a backup run may have committed a restore point since the list
   *  was last fetched. Shown as a badge; the list itself is only refetched on
   *  user request. */
  stale?: boolean
  /** Called after the list has been successfully refetched. */
  onRefreshed?: () => void
}

/** Browse restore points: pick a manifest, see its object composition, and
 * download the audit delta chain as gzip JSONL (the timestamped compliance
 * export artifact). Restore points can be deleted (the objects only they
 * reference are reclaimed from the target); restore itself is deliberately
 * left to the CLI. On Pro/Enterprise a restore-drill button verifies a
 * restore point against the target without writing anything. */
export default function ManifestBrowser({
  stacked = false,
  stale = false,
  onRefreshed,
}: ManifestBrowserProps) {
  const { t } = useTranslation()
  const { isPro } = useEdition()
  const [manifests, setManifests] = React.useState<ManifestView[]>([])
  const [loading, setLoading] = React.useState(true)
  const [refreshing, setRefreshing] = React.useState(false)
  const inFlightRef = React.useRef(false)
  const lastManualRefreshRef = React.useRef(0)
  // Keep the latest callback in a ref so it does not become an effect
  // dependency — the parent re-renders on every poll, and an inline callback
  // would otherwise refetch the list every few seconds.
  const onRefreshedRef = React.useRef(onRefreshed)
  React.useEffect(() => {
    onRefreshedRef.current = onRefreshed
  }, [onRefreshed])
  const [selectedId, setSelectedId] = React.useState<string | null>(null)
  const [detail, setDetail] = React.useState<ManifestView | null>(null)
  const [detailLoading, setDetailLoading] = React.useState(false)
  const [error, setError] = React.useState<string | null>(null)
  const [drilling, setDrilling] = React.useState(false)
  const [drillReport, setDrillReport] = React.useState<DrillReport | null>(null)
  const [deleting, setDeleting] = React.useState(false)
  const [deleteTarget, setDeleteTarget] = React.useState<ManifestView | null>(
    null
  )

  const refreshManifests = React.useCallback(async (silent = false) => {
    if (!silent) {
      setLoading(true)
    }
    try {
      const data = await listBackupManifests()
      setManifests(data)
      setError(null)
      onRefreshedRef.current?.()
      if (!data.length) {
        setSelectedId(null)
        setDetail(null)
      }
    } catch (e) {
      setError(getErrorMessage(e))
    } finally {
      setLoading(false)
    }
  }, [])

  React.useEffect(() => {
    refreshManifests()
  }, [refreshManifests])

  /** User-triggered refresh, rate-limited: a list call goes to the backup
   *  target and costs a request (S3 bills per 1,000 list calls), so repeated
   *  clicks within the cooldown window are rejected. */
  const handleManualRefresh = async () => {
    if (inFlightRef.current) return
    const elapsed = Date.now() - lastManualRefreshRef.current
    if (
      lastManualRefreshRef.current > 0 &&
      elapsed < MANUAL_REFRESH_COOLDOWN_MS
    ) {
      toast({
        title: t(
          'backup.refreshTooSoon',
          'Please wait {{seconds}}s before refreshing again',
          { seconds: Math.ceil((MANUAL_REFRESH_COOLDOWN_MS - elapsed) / 1000) }
        ),
        description: t(
          'backup.refreshTooSoonHint',
          'Listing restore points sends a request to the backup target.'
        ),
      })
      return
    }
    inFlightRef.current = true
    lastManualRefreshRef.current = Date.now()
    setRefreshing(true)
    try {
      await refreshManifests(true)
    } finally {
      inFlightRef.current = false
      setRefreshing(false)
    }
  }

  const handleSelect = async (id: string) => {
    setSelectedId(id)
    setDetailLoading(true)
    setDetail(null)
    try {
      const m = await getBackupManifest(id)
      setDetail(m)
    } catch (e) {
      toast({
        title: t('backup.loadFailed'),
        description: getErrorMessage(e),
        variant: 'destructive',
      })
    } finally {
      setDetailLoading(false)
    }
  }

  const handleDrill = async (manifestId: string) => {
    setDrilling(true)
    try {
      const report = await runBackupDrill(manifestId)
      setDrillReport(report)
      toast({
        title: report.passed
          ? t('backup.drillPassed', 'Restore drill passed')
          : t('backup.drillFailed', 'Restore drill failed'),
        description: report.error ?? report.note ?? undefined,
        variant: report.passed ? 'default' : 'destructive',
      })
    } catch (e) {
      toast({
        title: t('backup.drillFailed', 'Restore drill failed'),
        description: getErrorMessage(e),
        variant: 'destructive',
      })
    } finally {
      setDrilling(false)
    }
  }

  const handleDownloadDelta = async (
    manifestId: string,
    seqFrom: number,
    seqTo: number
  ) => {
    try {
      const blob = await downloadAuditDelta(manifestId, seqFrom, seqTo)
      saveAs(blob, `audit-${seqFrom}-${seqTo}.jsonl.gz`)
    } catch (e) {
      toast({
        title: t('backup.downloadFailed', 'Failed to download'),
        description: getErrorMessage(e),
        variant: 'destructive',
      })
    }
  }

  const handleDelete = async () => {
    if (!deleteTarget) return
    const target = deleteTarget
    setDeleting(true)
    try {
      const result = await deleteBackupManifest(target.id)
      toast({
        title: t('backup.deleteDone', 'Restore point deleted'),
        description: t('backup.deleteDoneHint', {
          objects: result.objects_removed,
          bytes: formatBytes(result.bytes_reclaimed),
          defaultValue:
            '{{objects}} object(s) removed, {{bytes}} freed from the target',
        }),
      })
      setDeleteTarget(null)
      if (selectedId === target.id) {
        setSelectedId(null)
        setDetail(null)
      }
      await refreshManifests(true)
    } catch (e) {
      toast({
        title: t('backup.deleteFailed', 'Failed to delete the restore point'),
        description: getErrorMessage(e),
        variant: 'destructive',
      })
    } finally {
      setDeleting(false)
    }
  }

  const objectRows: { label: string; object: ObjectView | null }[] = [
    {
      label: t('backup.objects.memdb', 'memdb'),
      object: detail?.memdb ?? null,
    },
    {
      label: t('backup.objects.imapUid', 'imap uid'),
      object: detail?.imap_uid ?? null,
    },
    {
      label: t('backup.objects.integrity', 'integrity'),
      object: detail?.integrity ?? null,
    },
    {
      label: t('backup.objects.timestamp', 'timestamp'),
      object: detail?.timestamp ?? null,
    },
  ]

  return (
    <div className='space-y-4'>
      <div className='flex flex-wrap items-center justify-between gap-2'>
        <div className='flex min-w-0 flex-1 items-center gap-2'>
          {!stacked && (
            <span className='text-xs font-medium'>
              {t('backup.manifestBrowser', 'Restore points')}
            </span>
          )}
          {stale && (
            <Badge
              variant='outline'
              className='border-amber-500/50 text-xs text-amber-600 dark:text-amber-500'
            >
              {t('backup.staleRestorePoints', 'New backup available')}
            </Badge>
          )}
          <Button
            variant='outline'
            size='sm'
            className='ml-auto h-8 gap-1 px-2 text-xs'
            title={t(
              'backup.refreshHint',
              'Refresh the restore-point list (sends a request to the backup target)'
            )}
            onClick={handleManualRefresh}
            disabled={refreshing}
          >
            {refreshing ? (
              <Loader2 className='h-3.5 w-3.5 animate-spin' />
            ) : (
              <RefreshCw className='h-3.5 w-3.5' />
            )}
            {t('backup.refresh')}
          </Button>
        </div>
      </div>
      <Separator />
      {error ? (
        <div className='py-8 text-center text-xs text-destructive'>{error}</div>
      ) : loading ? (
        <div className='flex items-center justify-center py-10'>
          <Loader2 className='h-5 w-5 animate-spin text-muted-foreground' />
        </div>
      ) : manifests.length === 0 ? (
        <div className='py-8 text-center text-xs text-muted-foreground'>
          {t('backup.manifestsEmpty')}
        </div>
      ) : (
        <div
          className={
            stacked
              ? 'grid grid-cols-1 gap-4'
              : 'grid grid-cols-1 gap-4 lg:grid-cols-2'
          }
        >
          <ScrollArea
            className={
              stacked
                ? 'h-[300px] rounded-md border'
                : 'max-h-[520px] rounded-md border'
            }
          >
            <div className='divide-y'>
              {manifests.map((m) => (
                <div
                  key={m.id}
                  className={`flex w-full items-center gap-2 px-3 py-2 text-left text-xs ${
                    m.id === selectedId ? 'bg-accent' : ''
                  }`}
                >
                  <button
                    type='button'
                    onClick={() => handleSelect(m.id)}
                    className='flex min-w-0 flex-1 cursor-pointer items-center gap-2'
                  >
                    <Archive className='h-4 w-4 shrink-0 text-muted-foreground' />
                    <span className='min-w-0 flex-1'>
                      <span className='block truncate font-medium'>{m.id}</span>
                      <span className='block text-xs text-muted-foreground'>
                        {formatTime(m.created)}
                        {' · '}
                        {m.trigger}
                      </span>
                    </span>
                    <span className='shrink-0 text-xs text-muted-foreground'>
                      {m.object_count} {t('backup.objectsLabel', 'objects')} ·{' '}
                      {formatBytes(m.bytes_total)}
                    </span>
                  </button>
                  <Button
                    variant='ghost'
                    size='icon'
                    className='h-7 w-7 shrink-0 text-muted-foreground hover:text-destructive'
                    title={t('backup.deletePoint', 'Delete restore point')}
                    disabled={deleting}
                    onClick={() => setDeleteTarget(m)}
                  >
                    <Trash2 className='h-3.5 w-3.5' />
                  </Button>
                </div>
              ))}
            </div>
          </ScrollArea>

          <div
            className={`flex flex-col overflow-hidden rounded-md border ${
              stacked ? 'min-h-[280px] max-h-[440px]' : 'h-[520px]'
            }`}
          >
            <div className='flex items-center justify-between border-b px-3 py-2'>
              <span className='min-w-0 flex-1 truncate text-xs text-muted-foreground'>
                {detail
                  ? `${detail.id} · ${detail.trigger}`
                  : t(
                      'backup.manifestHint',
                      'Select a restore point to inspect it'
                    )}
              </span>
              {detail && isPro && (
                <Button
                  variant='outline'
                  size='sm'
                  className='h-7 shrink-0 gap-1 text-xs'
                  disabled={drilling}
                  onClick={() => handleDrill(detail.id)}
                >
                  {drilling ? (
                    <Loader2 className='h-3.5 w-3.5 animate-spin' />
                  ) : (
                    <ShieldCheck className='h-3.5 w-3.5' />
                  )}
                  {t('backup.runDrill', 'Run restore drill')}
                </Button>
              )}
            </div>
            <ScrollArea className='relative flex-1'>
              {detailLoading && (
                <div className='absolute inset-0 z-10 flex items-center justify-center bg-background/60'>
                  <Loader2 className='h-5 w-5 animate-spin text-muted-foreground' />
                </div>
              )}
              {!detail ? (
                <div className='flex h-full items-center justify-center px-4 text-center text-xs text-muted-foreground'>
                  {t('backup.manifestHint')}
                </div>
              ) : (
                <div className='p-3 text-xs'>
                  {drillReport && drillReport.manifest_id === detail.id && (
                    <div
                      className={`mb-3 rounded-md border px-3 py-2 ${
                        drillReport.passed
                          ? 'border-emerald-500/40 bg-emerald-500/5'
                          : 'border-destructive/40 bg-destructive/5'
                      }`}
                    >
                      <div className='flex items-center gap-2 font-medium'>
                        {drillReport.passed ? (
                          <ShieldCheck className='h-3.5 w-3.5 text-emerald-500' />
                        ) : (
                          <ShieldCheck className='h-3.5 w-3.5 text-destructive' />
                        )}
                        {drillReport.passed
                          ? t('backup.drillPassed', 'Restore drill passed')
                          : t('backup.drillFailed', 'Restore drill failed')}
                        <span className='ml-auto font-normal text-muted-foreground'>
                          {formatTime(drillReport.ran_at)}
                        </span>
                      </div>
                      <div className='mt-1 text-muted-foreground'>
                        {t('backup.drillStats', drillStats(drillReport))}
                      </div>
                      {drillReport.error && (
                        <div className='mt-1 break-words text-destructive'>
                          {drillReport.error}
                        </div>
                      )}
                    </div>
                  )}

                  <div className='mb-2 flex items-center gap-2 font-medium'>
                    <TableProperties className='h-3.5 w-3.5 text-muted-foreground' />
                    {t('backup.objectsTitle', 'Objects')}
                  </div>
                  {objectRows.map(
                    ({ label, object }) =>
                      object && (
                        <ObjectRow key={label} label={label} object={object} />
                      )
                  )}
                  {detail.segments.map((s, i) => (
                    <ObjectRow
                      key={`seg-${s.key}`}
                      label={t('backup.objects.segment', 'segment {{n}}', {
                        n: s.segment_id ?? i + 1,
                      })}
                      object={s}
                    />
                  ))}
                  {detail.tantivy.map((s, i) => (
                    <ObjectRow
                      key={`tantivy-${s.key}`}
                      label={
                        s.name === 'envelope'
                          ? t('backup.kind.envelopeIndex', 'index (envelope)')
                          : s.name === 'attachment'
                            ? t(
                                'backup.kind.attachmentIndex',
                                'index (attachment)'
                              )
                            : t('backup.objects.index', 'index {{n}}', {
                                n: i + 1,
                              })
                      }
                      object={s}
                    />
                  ))}
                  {!objectRows.some((r) => r.object) &&
                    detail.segments.length === 0 &&
                    detail.tantivy.length === 0 && (
                      <div className='py-4 text-center text-muted-foreground'>
                        {t('backup.objectsEmpty', 'No objects referenced')}
                      </div>
                    )}

                  {detail.audit && (
                    <>
                      <div className='mb-2 mt-4 flex items-center gap-2 font-medium'>
                        <SplitSquareVertical className='h-3.5 w-3.5 text-muted-foreground' />
                        {t('backup.auditTitle', 'Audit chain')}
                      </div>
                      <div className='mb-2 space-y-1 text-muted-foreground'>
                        <div>
                          {t('backup.auditCursor', 'Cursor')}:{' '}
                          <code className='rounded bg-muted px-1 py-0.5'>
                            {detail.audit.cursor}
                          </code>
                        </div>
                        <div>
                          {t('backup.auditCumulative', 'Cumulative deltas')}:{' '}
                          {formatBytes(detail.audit.cumulative_delta_bytes)}
                          {detail.audit.base_ratio !== null && (
                            <>
                              {' · '}
                              {t('backup.auditBaseRatio', 'base ratio')}:{' '}
                              {(detail.audit.base_ratio * 100).toFixed(1)}%
                            </>
                          )}
                        </div>
                      </div>
                      {detail.audit.base && (
                        <ObjectRow
                          label={t('backup.auditBase', 'baseline')}
                          object={detail.audit.base}
                        />
                      )}
                      {detail.audit.deltas.map((d) => (
                        <div
                          key={`${d.seq_from}-${d.seq_to}`}
                          className='flex items-center justify-between gap-2 border-b px-3 py-1.5 text-xs last:border-0'
                        >
                          <code className='shrink-0 rounded bg-muted px-1.5 py-0.5'>
                            {d.seq_from + 1}–{d.seq_to}
                          </code>
                          <span className='min-w-0 flex-1 truncate text-muted-foreground'>
                            <code className='rounded bg-muted px-1.5 py-0.5'>
                              {shortKey(d.key)}
                            </code>
                          </span>
                          <span className='shrink-0 text-muted-foreground'>
                            {formatBytes(d.bytes)}
                          </span>
                          <Button
                            variant='ghost'
                            size='sm'
                            className='h-6 shrink-0 gap-1 px-2 text-xs'
                            onClick={() =>
                              handleDownloadDelta(
                                detail.id,
                                d.seq_from,
                                d.seq_to
                              )
                            }
                          >
                            <Download className='h-3 w-3' />
                            {t('backup.download')}
                          </Button>
                        </div>
                      ))}
                      {!detail.audit.base &&
                        detail.audit.deltas.length === 0 && (
                          <div className='py-2 text-muted-foreground'>
                            {t(
                              'backup.auditEmpty',
                              'No audit chain in this restore point'
                            )}
                          </div>
                        )}
                    </>
                  )}
                </div>
              )}
            </ScrollArea>
          </div>
        </div>
      )}

      <AlertDialog
        open={deleteTarget !== null}
        onOpenChange={(open) => {
          if (!open) setDeleteTarget(null)
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              {t('backup.deleteConfirmTitle', 'Delete this restore point?')}
            </AlertDialogTitle>
            <AlertDialogDescription>
              {t(
                'backup.deleteConfirmBody',
                'Restore point {{id}} ({{created}} · {{bytes}}) will be permanently deleted. Objects shared with other restore points are kept; the rest are removed from the target. This cannot be undone.',
                {
                  id: deleteTarget?.id,
                  created: deleteTarget ? formatTime(deleteTarget.created) : '',
                  bytes: deleteTarget
                    ? formatBytes(deleteTarget.bytes_total)
                    : '',
                }
              )}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={deleting}>
              {t('backup.cancel', 'Cancel')}
            </AlertDialogCancel>
            <AlertDialogAction
              disabled={deleting}
              onClick={(e) => {
                e.preventDefault()
                handleDelete()
              }}
              className='bg-destructive text-white hover:bg-destructive/90'
            >
              {deleting && (
                <Loader2 className='mr-1 h-3.5 w-3.5 animate-spin' />
              )}
              {t('backup.deleteConfirmAction', 'Delete')}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  )
}
