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
//
// Integrity check page (Pro edition). Manually verify the archive: recompute
// content hashes against the envelope index, track run progress, browse run
// history and download per-account / failure CSV reports.
import { Fragment, useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import {
  AlertTriangle,
  CheckCircle2,
  ChevronDown,
  Download,
  Loader2,
  Play,
  RefreshCw,
  ShieldCheck,
  Wrench,
  X,
  XCircle,
} from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { minimal_account_list } from '@/api/account/api'
import {
  cancel_integrity_run,
  download_integrity_report,
  get_active_integrity_run,
  get_integrity_report,
  list_integrity_runs,
  start_integrity_run,
  type FailureRow,
  type IntegrityMode,
  type IntegrityReport,
  type JobProgress,
  type RunPage,
  type RunSummary,
} from '@/api/integrity/api'
import { cn } from '@/lib/utils'
import { useCurrentUser } from '@/hooks/use-current-user'
import { RepairMessageDialog } from '@/features/repair/repair-message-dialog'
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
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from '@/components/ui/card'
import { Drawer, DrawerContent, DrawerTitle } from '@/components/ui/drawer'
import { Progress } from '@/components/ui/progress'
import { ScrollArea } from '@/components/ui/scroll-area'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { Separator } from '@/components/ui/separator'
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
import { TablePagination } from '@/components/pagination'
import { TableSkeleton } from '@/components/table-skeleton'
import { VirtualizedSelect } from '@/components/virtualized-select'

const PAGE_SIZE = 20
const FAILURES_PAGE_SIZE = 20

function formatTime(ts: number): string {
  const d = new Date(ts)
  const pad = (n: number) => String(n).padStart(2, '0')
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`
}

function shortId(runId: string): string {
  return runId.length > 8 ? `${runId.slice(0, 8)}…` : runId
}

function statusLabel(
  t: (key: string, defaultValue: string) => string,
  status: string
): string {
  switch (status) {
    case 'running':
      return t('integrity.statusRunning', 'Running')
    case 'finished':
      return t('integrity.statusFinished', 'Finished')
    case 'cancelled':
      return t('integrity.statusCancelled', 'Cancelled')
    case 'failed':
      return t('integrity.statusFailed', 'Failed')
    default:
      return status
  }
}

function StatusBadge({ status }: { status: string }) {
  const { t } = useTranslation()
  const running = status === 'running'
  const variant =
    status === 'finished'
      ? 'default'
      : running
        ? 'secondary'
        : status === 'cancelled'
          ? 'outline'
          : 'destructive'
  return (
    <Badge variant={variant} className={running ? 'gap-1.5' : undefined}>
      {running && <Loader2 className='h-3 w-3 animate-spin' />}
      {statusLabel(t, status)}
    </Badge>
  )
}

function failureTypeLabel(
  t: (key: string, defaultValue: string) => string,
  kind: string
): string {
  const labels: Record<string, string> = {
    missing_email_blob: t(
      'integrity.failureMissingEmailBlob',
      'Missing email blob'
    ),
    blob_read_error: t('integrity.failureBlobReadError', 'Blob read error'),
    missing_attachment_blob: t(
      'integrity.failureMissingAttachmentBlob',
      'Missing attachment blob'
    ),
    content_hash_mismatch: t(
      'integrity.failureHashMismatch',
      'Content hash mismatch'
    ),
    attachment_count_mismatch: t(
      'integrity.failureAttachmentCount',
      'Attachment count mismatch'
    ),
    reattach_error: t('integrity.failureReattach', 'Re-attach error'),
  }
  return labels[kind] ?? kind
}

function formatBytes(bytes: number | null | undefined): string {
  if (!bytes) return '—'
  const units = ['B', 'KB', 'MB', 'GB']
  let v = bytes
  let i = 0
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024
    i += 1
  }
  return `${v.toFixed(i === 0 ? 0 : 1)} ${units[i]}`
}

function Stat({
  label,
  children,
}: {
  label: string
  children: React.ReactNode
}) {
  return (
    <div>
      <div className='text-xs text-muted-foreground'>{label}</div>
      <div className='mt-1'>{children}</div>
    </div>
  )
}
interface ActiveRunCardProps {
  active: JobProgress
  onCancelling: (value: boolean) => void
  /** Hide the cancel button for read-only compliance officers. */
  canCancel?: boolean
}

function ActiveRunCard({ active, onCancelling, canCancel = true }: ActiveRunCardProps) {
  const { t } = useTranslation()
  const queryClient = useQueryClient()
  const [busy, setBusy] = useState(false)
  const pct =
    active.total > 0 ? Math.round((active.processed / active.total) * 100) : 0

  const handleCancel = async () => {
    setBusy(true)
    onCancelling(true)
    try {
      await cancel_integrity_run(active.run_id)
      toast({ title: t('integrity.cancelRequested', 'Cancellation requested') })
    } catch {
      toast({
        title: t('integrity.cancelFailed', 'Failed to request cancellation'),
        variant: 'destructive',
      })
    } finally {
      setBusy(false)
      onCancelling(false)
    }
  }

  const refresh = () => {
    queryClient.invalidateQueries({ queryKey: ['integrity-active'] })
    queryClient.invalidateQueries({ queryKey: ['integrity-jobs'] })
  }

  return (
    <Card>
      <CardHeader>
        <CardTitle className='flex items-center gap-2'>
          <ShieldCheck className='h-5 w-5 text-primary' />
          {t('integrity.activeRun', 'Active integrity check')}
        </CardTitle>
        <CardDescription>
          {active.mode === 'quick'
            ? t(
                'integrity.quickModeHint',
                'Quick mode: verifies that email blobs exist.'
              )
            : t(
                'integrity.fullModeHint',
                'Full mode: re-attaches attachments and recomputes content hashes.'
              )}
        </CardDescription>
      </CardHeader>
      <CardContent className='space-y-4'>
        <div className='grid gap-4 sm:grid-cols-3'>
          <Stat label={t('integrity.status', 'Status')}>
            <StatusBadge status={active.status} />
          </Stat>
          <Stat label={t('integrity.startedBy', 'Started by')}>
            <span className='text-xs font-medium'>{active.triggered_by}</span>
          </Stat>
          <Stat label={t('integrity.startedAt', 'Started at')}>
            <span className='text-xs font-medium'>
              {formatTime(active.started_at)}
            </span>
          </Stat>
        </div>
        {active.current_account_name && (
          <div className='text-xs text-muted-foreground'>
            {t('integrity.currentAccount', 'Checking account')}:{' '}
            <span className='font-medium text-foreground'>
              {active.current_account_name}
            </span>
          </div>
        )}
        <div className='space-y-1'>
          <div className='flex items-center justify-between text-xs'>
            <span className='text-muted-foreground'>
              {active.processed.toLocaleString()} /{' '}
              {active.total.toLocaleString()}
            </span>
            <span className='font-medium'>{pct}%</span>
          </div>
          <Progress value={pct} />
        </div>
        <div className='flex items-center justify-between text-xs'>
          <div className='flex gap-4'>
            <span className='flex items-center gap-1 text-green-600'>
              <CheckCircle2 className='h-4 w-4' />
              {active.ok.toLocaleString()} {t('integrity.ok', 'ok')}
            </span>
            <span className='flex items-center gap-1 text-red-600'>
              <XCircle className='h-4 w-4' />
              {active.failed.toLocaleString()} {t('integrity.failed', 'failed')}
            </span>
          </div>
          <div className='flex gap-2'>
            <Button variant='outline' size='sm' onClick={refresh}>
              <RefreshCw className='mr-1 h-4 w-4' />
              {t('integrity.refresh', 'Refresh')}
            </Button>
            {canCancel && (
              <Button
                variant='destructive'
                size='sm'
                onClick={handleCancel}
                disabled={busy}
              >
                <XCircle className='mr-1 h-4 w-4' />
                {t('integrity.cancelRun', 'Cancel')}
              </Button>
            )}
          </div>
        </div>
      </CardContent>
    </Card>
  )
}

function RunForm({
  onStarted,
  hasActiveRun,
}: {
  onStarted: (runId: string) => void
  hasActiveRun: boolean
}) {
  const { t } = useTranslation()
  const queryClient = useQueryClient()
  const [mode, setMode] = useState<IntegrityMode>('full')
  const [accountIds, setAccountIds] = useState<string[]>([])
  const [starting, setStarting] = useState(false)
  const [confirmOpen, setConfirmOpen] = useState(false)

  const { data: accounts } = useQuery({
    queryKey: ['integrity-accounts'],
    queryFn: minimal_account_list,
    staleTime: 60_000,
  })

  const options = (accounts ?? []).map((a) => ({
    value: String(a.id),
    label: a.email,
  }))

  const handleStart = async () => {
    setStarting(true)
    try {
      const res = await start_integrity_run({
        mode,
        account_ids: accountIds.length > 0 ? accountIds.map(Number) : undefined,
      })
      toast({ title: t('integrity.runStarted', 'Integrity check started') })
      onStarted(res.run_id)
      queryClient.invalidateQueries({ queryKey: ['integrity-active'] })
      queryClient.invalidateQueries({ queryKey: ['integrity-jobs'] })
    } catch {
      toast({
        title: t('integrity.runStartFailed', 'Failed to start integrity check'),
        variant: 'destructive',
      })
    } finally {
      setStarting(false)
    }
  }

  return (
    <Card>
      <CardHeader>
        <CardTitle>{t('integrity.runCheck', 'Run integrity check')}</CardTitle>
        <CardDescription>
          {t(
            'integrity.runCheckHint',
            'Verify that archived email content matches the envelope index. The check is read-only and never modifies data.'
          )}
        </CardDescription>
      </CardHeader>
      <CardContent className='space-y-4'>
        <div className='grid gap-4 sm:grid-cols-2'>
          <div className='space-y-2'>
            <label className='text-xs font-medium'>
              {t('integrity.mode', 'Mode')}
            </label>
            <Select
              value={mode}
              onValueChange={(v) => setMode(v as IntegrityMode)}
            >
              <SelectTrigger className='w-full'>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value='full'>
                  {t('integrity.modeFull', 'Full (recompute content hashes)')}
                </SelectItem>
                <SelectItem value='quick'>
                  {t('integrity.modeQuick', 'Quick (check email blobs exist)')}
                </SelectItem>
              </SelectContent>
            </Select>
          </div>
          <div className='space-y-2'>
            <label className='text-xs font-medium'>
              {t('integrity.accounts', 'Accounts')}
            </label>
            <VirtualizedSelect
              multiple
              options={options}
              value={accountIds}
              onSelectOption={setAccountIds}
              isLoading={!accounts}
              placeholder={t('integrity.allAccounts', 'All accounts')}
              height='200px'
            />
          </div>
        </div>
        <div className='flex items-center justify-between'>
          <span className='text-xs text-muted-foreground'>
            {accountIds.length === 0
              ? t('integrity.scopeAllAccounts', 'Scope: all accounts')
              : t(
                  'integrity.scopeSelectedAccounts',
                  'Scope: {count} selected account(s)',
                  {
                    count: accountIds.length,
                  }
                )}
          </span>
          <Button onClick={() => setConfirmOpen(true)} disabled={starting}>
            <Play className='mr-1 h-4 w-4' />
            {starting
              ? t('integrity.starting', 'Starting…')
              : t('integrity.startRun', 'Start check')}
          </Button>
        </div>
        <AlertDialog open={confirmOpen} onOpenChange={setConfirmOpen}>
          <AlertDialogContent>
            <AlertDialogHeader>
              <AlertDialogTitle>
                {t('integrity.confirmStartTitle', 'Start integrity check?')}
              </AlertDialogTitle>
              <AlertDialogDescription>
                {t(
                  'integrity.confirmStartDesc',
                  'The check runs in the background and may take a while depending on scope and mode. You can leave this page and check the history later.'
                )}
              </AlertDialogDescription>
            </AlertDialogHeader>
            <div className='space-y-2 text-xs'>
              <div className='flex items-center justify-between rounded-md border px-3 py-2'>
                <span className='text-muted-foreground'>
                  {t('integrity.mode', 'Mode')}
                </span>
                <span className='font-medium'>
                  {mode === 'quick'
                    ? t('integrity.modeQuick', 'Quick (check email blobs exist)')
                    : t('integrity.modeFull', 'Full (recompute content hashes)')}
                </span>
              </div>
              <div className='flex items-center justify-between rounded-md border px-3 py-2'>
                <span className='text-muted-foreground'>
                  {t('integrity.accounts', 'Accounts')}
                </span>
                <span className='font-medium'>
                  {accountIds.length === 0
                    ? t('integrity.allAccounts', 'All accounts')
                    : t(
                        'integrity.scopeSelectedAccounts',
                        'Scope: {count} selected account(s)',
                        {
                          count: accountIds.length,
                        }
                      )}
                </span>
              </div>
              {hasActiveRun && (
                <div className='flex items-start gap-2 rounded-md border border-destructive/40 bg-destructive/5 px-3 py-2 text-destructive'>
                  <AlertTriangle className='mt-0.5 h-3.5 w-3.5 shrink-0' />
                  <span>
                    {t(
                      'integrity.confirmStartActive',
                      'An integrity check is already running. Starting another one will be rejected until it finishes.'
                    )}
                  </span>
                </div>
              )}
            </div>
            <AlertDialogFooter>
              <AlertDialogCancel>{t('common.cancel')}</AlertDialogCancel>
              <AlertDialogAction onClick={handleStart} disabled={starting}>
                {starting
                  ? t('integrity.starting', 'Starting…')
                  : t('integrity.startRun', 'Start check')}
              </AlertDialogAction>
            </AlertDialogFooter>
          </AlertDialogContent>
        </AlertDialog>
      </CardContent>
    </Card>
  )
}
function Report({ runId, onClose }: { runId: string; onClose?: () => void }) {
  const { t } = useTranslation()
  const [failuresPage, setFailuresPage] = useState(1)
  const [failuresPageSize, setFailuresPageSize] = useState(
    () =>
      Number(localStorage.getItem('bichon_integrity_failures_page_size')) ||
      FAILURES_PAGE_SIZE
  )

  const handleFailuresPageSizeChange = (size: number) => {
    localStorage.setItem('bichon_integrity_failures_page_size', String(size))
    setFailuresPage(1)
    setFailuresPageSize(size)
  }

  const { data: report, isLoading } = useQuery({
    queryKey: ['integrity-report', runId, failuresPage, failuresPageSize],
    queryFn: () =>
      get_integrity_report(runId, failuresPage, failuresPageSize),
    placeholderData: (prev) => prev,
    // Auto-refresh while the run is still in progress so the report status,
    // counters and failure rows update without a manual refresh.
    refetchInterval: (query) => {
      const data = query.state.data as IntegrityReport | undefined
      return data?.status === 'running' ? 5000 : false
    },
  })

  if (isLoading && !report) {
    return (
      <Card>
        <CardHeader>
          <CardTitle>{t('integrity.report', 'Report')}</CardTitle>
        </CardHeader>
        <CardContent>
          <TableSkeleton rows={5} />
        </CardContent>
      </Card>
    )
  }

  if (!report) return null
  const storage = report.failure_storage ?? 'db'

  return (
    <div className='flex h-full min-h-0 flex-col gap-3 overflow-hidden'>
      <Card className='flex min-h-0 flex-1 flex-col overflow-hidden'>
        <CardHeader className='flex flex-row items-center justify-between gap-3 space-y-0 px-4 py-3'>
          <div>
            <CardTitle>{t('integrity.report', 'Report')}</CardTitle>
            <CardDescription className='mt-1 text-xs'>
              {t('integrity.runId', 'Run')}:{' '}
              <span className='font-mono'>{shortId(report.run_id)}</span>
            </CardDescription>
          </div>
          <div className='flex gap-2'>
            <Button
              variant='outline'
              size='sm'
              onClick={() => download_integrity_report(runId, 'summary')}
            >
              <Download className='mr-1 h-4 w-4' />
              {t('integrity.downloadSummary', 'Summary CSV')}
            </Button>
            {storage !== 'truncated' && (
              <Button
                variant='outline'
                size='sm'
                onClick={() => download_integrity_report(runId, 'failures')}
              >
                <Download className='mr-1 h-4 w-4' />
                {t('integrity.downloadFailures', 'Failures CSV')}
              </Button>
            )}
            {onClose && (
              <Button
                variant='ghost'
                size='icon'
                onClick={onClose}
                aria-label={t('common.close', 'Close')}
              >
                <X className='h-4 w-4' />
              </Button>
            )}
          </div>
        </CardHeader>
        <CardContent className='flex min-h-0 flex-1 flex-col gap-3 overflow-hidden px-4 pb-4 pt-0'>
          <div className='grid shrink-0 gap-3 sm:grid-cols-2 lg:grid-cols-4'>
            <Stat label={t('integrity.status', 'Status')}>
              <StatusBadge status={report.status} />
            </Stat>
            <Stat label={t('integrity.mode', 'Mode')}>
              <span className='text-xs font-medium capitalize'>
                {report.mode}
              </span>
            </Stat>
            <Stat label={t('integrity.startedBy', 'Started by')}>
              <span className='text-xs font-medium'>{report.triggered_by}</span>
            </Stat>
            <Stat label={t('integrity.startedAt', 'Started at')}>
              <span className='text-xs font-medium'>
                {formatTime(report.started_at)}
              </span>
            </Stat>
            <Stat label={t('integrity.total', 'Total')}>
              <span className='text-xs font-medium'>
                {report.total.toLocaleString()}
              </span>
            </Stat>
            <Stat label={t('integrity.ok', 'OK')}>
              <span className='flex items-center gap-1 text-xs font-medium text-green-600'>
                <CheckCircle2 className='h-4 w-4' />
                {report.ok.toLocaleString()}
              </span>
            </Stat>
            <Stat label={t('integrity.failed', 'Failed')}>
              <span className='flex items-center gap-1 text-xs font-medium text-red-600'>
                <AlertTriangle className='h-4 w-4' />
                {report.failed.toLocaleString()}
              </span>
            </Stat>
            <Stat label={t('integrity.integrityRate', 'Integrity rate')}>
              <span className='text-xs font-medium'>
                {report.integrity_pct}%
              </span>
            </Stat>
          </div>
          {report.message && (
            <div className='shrink-0 rounded border border-destructive/40 bg-destructive/10 p-3 text-xs text-destructive'>
              {report.message}
            </div>
          )}
          <Separator className='shrink-0' />
          <AccountStats accounts={report.accounts} />
          <FailuresTable
            report={report}
            page={failuresPage}
            onPageChange={setFailuresPage}
            pageSize={failuresPageSize}
            onPageSizeChange={handleFailuresPageSizeChange}
          />
        </CardContent>
      </Card>
    </div>
  )
}

function AccountStats({ accounts }: { accounts: IntegrityReport['accounts'] }) {
  const { t } = useTranslation()
  if (accounts.length === 0) return null
  return (
    <div className='flex min-h-0 shrink-0 flex-col gap-2'>
      <h4 className='text-xs font-semibold'>
        {t('integrity.perAccount', 'Per-account summary')}
      </h4>
      <ScrollArea orientation='both' className='max-h-[200px]'>
        <Table className='text-xs'>
          <TableHeader>
            <TableRow>
              <TableHead>{t('integrity.account', 'Account')}</TableHead>
              <TableHead className='text-right'>
                {t('integrity.total', 'Total')}
              </TableHead>
              <TableHead className='text-right'>
                {t('integrity.ok', 'OK')}
              </TableHead>
              <TableHead className='text-right'>
                {t('integrity.failed', 'Failed')}
              </TableHead>
              <TableHead className='text-right'>
                {t('integrity.integrityRate', 'Integrity')}
              </TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {accounts.map((a) => (
              <TableRow key={a.account_id}>
                <TableCell className='font-medium'>{a.account_name}</TableCell>
                <TableCell className='text-right'>
                  {a.total.toLocaleString()}
                </TableCell>
                <TableCell className='text-right text-green-600'>
                  {a.ok.toLocaleString()}
                </TableCell>
                <TableCell className='text-right text-red-600'>
                  {a.failed.toLocaleString()}
                </TableCell>
                <TableCell className='text-right'>{a.integrity_pct}%</TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      </ScrollArea>
    </div>
  )
}

function failureSubject(f: FailureRow): string {
  return [f.subject, f.mailbox_name].filter(Boolean).join(' · ') || '—'
}

function FailureDetail({ f }: { f: FailureRow }) {
  const { t } = useTranslation()
  const hashMismatch =
    f.expected_hash && f.actual_hash && f.expected_hash !== f.actual_hash
  return (
    <div className='space-y-3'>
      <div>
        <div className='text-xs font-medium text-muted-foreground'>
          {t('integrity.subject', 'Subject')}
        </div>
        <div className='mt-0.5 break-words font-medium'>{f.subject || '—'}</div>
      </div>
      {f.detail && (
        <div>
          <div className='text-xs font-medium text-muted-foreground'>
            {t('integrity.detail', 'Detail')}
          </div>
          <div className='mt-0.5 whitespace-pre-wrap break-words text-xs'>
            {f.detail}
          </div>
        </div>
      )}
      <div className='grid gap-x-6 gap-y-1.5 sm:grid-cols-2'>
        {f.mailbox_name && (
          <MetaRow label={t('integrity.mailbox', 'Mailbox')}>
            <span className='break-all'>{f.mailbox_name}</span>
          </MetaRow>
        )}
        {f.message_id && (
          <MetaRow label={t('integrity.messageId', 'Message-ID')}>
            <span className='break-all font-mono'>{f.message_id}</span>
          </MetaRow>
        )}
        {f.uid != null && (
          <MetaRow label={t('integrity.uid', 'UID')}>{f.uid}</MetaRow>
        )}
        {f.internal_date != null && (
          <MetaRow label={t('integrity.date', 'Date')}>
            {formatTime(f.internal_date)}
          </MetaRow>
        )}
        {f.expected_hash && (
          <MetaRow label={t('integrity.expectedHash', 'Expected hash')}>
            <span className='break-all font-mono'>{f.expected_hash}</span>
          </MetaRow>
        )}
        {f.actual_hash && (
          <MetaRow label={t('integrity.actualHash', 'Actual hash')}>
            <span
              className={cn(
                'break-all font-mono',
                hashMismatch && 'text-red-600'
              )}
            >
              {f.actual_hash}
            </span>
          </MetaRow>
        )}
      </div>
    </div>
  )
}

function MetaRow({
  label,
  children,
}: {
  label: string
  children: React.ReactNode
}) {
  return (
    <div className='flex min-w-0 gap-2 text-xs'>
      <span className='shrink-0 text-muted-foreground'>{label}</span>
      <span className='min-w-0'>{children}</span>
    </div>
  )
}

function FailuresTable({
  report,
  page,
  onPageChange,
  pageSize,
  onPageSizeChange,
}: {
  report: IntegrityReport
  page: number
  onPageChange: (page: number) => void
  pageSize: number
  onPageSizeChange: (pageSize: number) => void
}) {
  const { t } = useTranslation()
  const failures = report.failures
  const [expanded, setExpanded] = useState<Record<string, boolean>>({})
  const [repairTarget, setRepairTarget] = useState<FailureRow | null>(null)
  const { require_any_permission } = useCurrentUser()

  const toggle = (key: string) =>
    setExpanded((prev) => ({ ...prev, [key]: !prev[key] }))

  // Only the two missing-blob kinds can be repaired. `attachment_count_mismatch`
  // returns `failed` before a repair is ever attempted, and `blob_read_error`
  // (present but corrupt) is deliberately not touched by the repair path — a
  // button for either would only ever write a meaningless history entry.
  //
  // Gated per row rather than by the page-level `canManage`: that one checks
  // account:manage, while repairing needs data:manage, so a compliance officer
  // holding only compliance:audit must not see the button.
  const canRepair = (f: FailureRow) =>
    (f.failure_type === 'missing_email_blob' ||
      f.failure_type === 'missing_attachment_blob') &&
    require_any_permission(
      ['system:root', 'data:manage', 'data:manage:all'],
      f.account_id
    )

  return (
    <div className='flex min-h-0 flex-1 flex-col gap-2'>
      <h4 className='text-xs font-semibold'>
        {t('integrity.failures', 'Failures')} ({report.failed.toLocaleString()})
      </h4>
      {report.failure_storage === 'file' && (
        <div className='flex items-start gap-2 rounded border bg-muted/50 p-3 text-xs text-muted-foreground'>
          <Download className='mt-0.5 h-4 w-4 shrink-0' />
          <span>
            {t(
              'integrity.previewFailures',
              'Only showing the first {{preview}} of {{total}} failures. Download the full CSV for all failures.',
              {
                preview: failures.total.toLocaleString(),
                total: report.failed.toLocaleString(),
              }
            )}
          </span>
        </div>
      )}
      {report.failure_storage === 'truncated' && (
        <div className='flex items-start gap-2 rounded border border-destructive/40 bg-destructive/10 p-3 text-xs text-destructive'>
          <AlertTriangle className='mt-0.5 h-4 w-4 shrink-0' />
          <span>
            {t(
              'integrity.failureDetailTruncated',
              'Failure detail truncated: {{total}} failures found, per-message detail was not recorded. Showing {{preview}} failures as a sample.',
              {
                total: report.failed.toLocaleString(),
                preview: failures.total.toLocaleString(),
              }
            )}
          </span>
        </div>
      )}
      {failures.total === 0 ? (
        <div className='flex items-center gap-2 rounded border p-4 text-xs text-muted-foreground'>
          <CheckCircle2 className='h-4 w-4 text-green-600' />
          {t('integrity.noFailures', 'No failures found.')}
        </div>
      ) : (
        <>
          <ScrollArea orientation='both' className='min-h-[120px] flex-1'>
            <Table className='text-xs'>
              <TableHeader>
                <TableRow>
                  <TableHead className='w-10'>
                    <span className='sr-only'>
                      {t('integrity.expand', 'Expand')}
                    </span>
                  </TableHead>
                  <TableHead>{t('integrity.account', 'Account')}</TableHead>
                  <TableHead>{t('integrity.failureType', 'Type')}</TableHead>
                  <TableHead>{t('integrity.subject', 'Subject')}</TableHead>
                  <TableHead className='text-right'>
                    {t('integrity.size', 'Size')}
                  </TableHead>
                  <TableHead className='text-right'>
                    {t('integrity.actions', 'Actions')}
                  </TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {failures.items.map((f) => {
                  const key = `${f.envelope_id}-${f.failure_type}`
                  const isOpen = !!expanded[key]
                  return (
                    <Fragment key={key}>
                      <TableRow>
                        <TableCell className='w-10'>
                          <Button
                            variant='ghost'
                            size='icon'
                            className='h-7 w-7'
                            aria-expanded={isOpen}
                            aria-label={
                              isOpen
                                ? t('integrity.collapse', 'Collapse')
                                : t('integrity.expand', 'Expand')
                            }
                            onClick={() => toggle(key)}
                          >
                            <ChevronDown
                              className={cn(
                                'h-4 w-4 transition-transform',
                                isOpen && 'rotate-180'
                              )}
                            />
                          </Button>
                        </TableCell>
                        <TableCell className='font-medium'>
                          {f.account_name ?? f.account_id}
                        </TableCell>
                        <TableCell>
                          <Badge variant='destructive'>
                            {failureTypeLabel(t, f.failure_type)}
                          </Badge>
                        </TableCell>
                        <TableCell className='max-w-md truncate'>
                          {failureSubject(f)}
                        </TableCell>
                        <TableCell className='text-right'>
                          {formatBytes(f.size)}
                        </TableCell>
                        <TableCell className='text-right'>
                          {canRepair(f) && (
                            <Button
                              variant='outline'
                              size='sm'
                              className='h-7 gap-1 px-2 text-xs'
                              onClick={() => setRepairTarget(f)}
                            >
                              <Wrench className='h-3.5 w-3.5' />
                              {t('repair_message.confirm', 'Repair')}
                            </Button>
                          )}
                        </TableCell>
                      </TableRow>
                      {isOpen && (
                        <TableRow className='bg-muted/40'>
                          <TableCell colSpan={6} className='p-4'>
                            <FailureDetail f={f} />
                          </TableCell>
                        </TableRow>
                      )}
                    </Fragment>
                  )
                })}
              </TableBody>
            </Table>
          </ScrollArea>
          <div className='flex items-center justify-between'>
            <TablePagination
              totalItems={failures.total}
              pageIndex={page - 1}
              pageSize={pageSize}
              hasNextPage={() => failures.total > page * pageSize}
              setPageIndex={(i) => onPageChange(i + 1)}
              setPageSize={onPageSizeChange}
            />
          </div>
        </>
      )}

      {/* The report is a stored snapshot in integrity.db; a repair does not
          change it, so there is nothing to refetch here. */}
      <RepairMessageDialog
        open={repairTarget !== null}
        onOpenChange={(open) => !open && setRepairTarget(null)}
        accountId={repairTarget?.account_id}
        envelopeId={repairTarget?.envelope_id}
        subject={repairTarget?.subject ?? undefined}
      />
    </div>
  )
}
export default function IntegrityPage() {
  const { t } = useTranslation()
  const { isPro } = useEdition()
  const { require_any_permission } = useCurrentUser()
  const [page, setPage] = useState(1)
  const [pageSize, setPageSize] = useState(
    () => Number(localStorage.getItem('bichon_integrity_page_size')) || PAGE_SIZE
  )
  const [selectedRun, setSelectedRun] = useState<string | null>(null)

  // Read access includes compliance officers (`compliance:audit`,
  // separation of duties); starting/canceling runs stays manage-only.
  const canView =
    isPro &&
    require_any_permission([
      'system:root',
      'account:manage:all',
      'account:manage',
      'compliance:audit',
    ])
  const canManage =
    isPro &&
    require_any_permission([
      'system:root',
      'account:manage:all',
      'account:manage',
    ])

  const { data: active } = useQuery({
    queryKey: ['integrity-active'],
    queryFn: get_active_integrity_run,
    enabled: canView,
    refetchInterval: (query) =>
      query.state.data &&
      'status' in query.state.data &&
      query.state.data.status === 'running'
        ? 5000
        : false,
  })
  const activeRun =
    active && 'status' in active ? (active as JobProgress) : null

  const { data: jobs, isLoading: jobsLoading } = useQuery({
    queryKey: ['integrity-jobs', page, pageSize],
    queryFn: () => list_integrity_runs(page, pageSize),
    enabled: canView,
    placeholderData: (prev) => prev,
    // Poll while the newest run is still in progress so the history status
    // flips to finished/cancelled/failed without a manual refresh.
    refetchInterval: (query) => {
      const data = query.state.data as RunPage | undefined
      return data?.items?.[0]?.status === 'running' ? 5000 : false
    },
  })

  const handlePageSizeChange = (size: number) => {
    localStorage.setItem('bichon_integrity_page_size', String(size))
    setPage(1)
    setPageSize(size)
  }

  if (!canView) {
    return (
      <>
        <FixedHeader />
        <Main>
          <div className='mx-auto w-full max-w-7xl px-4 py-16 text-center text-muted-foreground'>
            {t(
              'integrity.forbidden',
              'Integrity check is available in the Pro edition with account management permission.'
            )}
          </div>
        </Main>
      </>
    )
  }

  return (
    <>
      <FixedHeader />
      <Main>
        <div className='mx-auto w-full max-w-7xl space-y-6 text-xs'>
          <div>
            <h1 className='text-lg font-semibold'>
              {t('integrity.title', 'Integrity check')}
            </h1>
            <p className='text-xs text-muted-foreground'>
              {t(
                'integrity.subtitle',
                'Verify archived email content against the envelope index and download compliance reports.'
              )}
            </p>
          </div>

          {activeRun && (
            <ActiveRunCard
              active={activeRun}
              onCancelling={() => {}}
              canCancel={canManage}
            />
          )}

          {canManage && (
            <RunForm
              onStarted={setSelectedRun}
              hasActiveRun={activeRun !== null}
            />
          )}

          <Card>
            <CardHeader>
              <CardTitle>{t('integrity.history', 'Run history')}</CardTitle>
            </CardHeader>
            <CardContent className='space-y-4'>
              {jobsLoading && !jobs ? (
                <TableSkeleton rows={6} />
              ) : (
                <>
                  <Table className='text-xs'>
                    <TableHeader>
                      <TableRow>
                        <TableHead>{t('integrity.run', 'Run')}</TableHead>
                        <TableHead>{t('integrity.status', 'Status')}</TableHead>
                        <TableHead>{t('integrity.mode', 'Mode')}</TableHead>
                        <TableHead>
                          {t('integrity.startedBy', 'Started by')}
                        </TableHead>
                        <TableHead>
                          {t('integrity.startedAt', 'Started at')}
                        </TableHead>
                        <TableHead className='text-right'>
                          {t('integrity.ok', 'OK')} /{' '}
                          {t('integrity.failed', 'Failed')}
                        </TableHead>
                        <TableHead className='text-right'>
                          {t('integrity.actions', 'Actions')}
                        </TableHead>
                      </TableRow>
                    </TableHeader>
                    <TableBody>
                      {(jobs?.items ?? []).map((run: RunSummary) => (
                        <TableRow
                          key={run.run_id}
                          className={
                            selectedRun === run.run_id
                              ? 'bg-accent/50'
                              : 'cursor-pointer'
                          }
                          onClick={() => setSelectedRun(run.run_id)}
                        >
                          <TableCell className='font-mono'>
                            {shortId(run.run_id)}
                          </TableCell>
                          <TableCell>
                            <StatusBadge status={run.status} />
                          </TableCell>
                          <TableCell className='capitalize'>
                            {run.mode}
                          </TableCell>
                          <TableCell>{run.triggered_by}</TableCell>
                          <TableCell>{formatTime(run.started_at)}</TableCell>
                          <TableCell className='text-right'>
                            <span className='text-green-600'>
                              {run.ok.toLocaleString()}
                            </span>
                            {' / '}
                            <span className='text-red-600'>
                              {run.failed.toLocaleString()}
                            </span>
                          </TableCell>
                          <TableCell className='text-right'>
                            <Button
                              variant='ghost'
                              size='sm'
                              onClick={(e) => {
                                e.stopPropagation()
                                setSelectedRun(run.run_id)
                              }}
                            >
                              {t('integrity.viewReport', 'Report')}
                            </Button>
                          </TableCell>
                        </TableRow>
                      ))}
                      {(jobs?.items ?? []).length === 0 && (
                        <TableRow>
                          <TableCell
                            colSpan={7}
                            className='py-8 text-center text-muted-foreground'
                          >
                            {t(
                              'integrity.noRuns',
                              'No integrity checks have been run yet.'
                            )}
                          </TableCell>
                        </TableRow>
                      )}
                    </TableBody>
                  </Table>
                  {jobs && jobs.total > 0 && (
                    <div className='flex items-center justify-end'>
                      <TablePagination
                        totalItems={jobs.total}
                        pageIndex={page - 1}
                        pageSize={pageSize}
                        hasNextPage={() => jobs.total > page * pageSize}
                        setPageIndex={(i) => setPage(i + 1)}
                        setPageSize={handlePageSizeChange}
                      />
                    </div>
                  )}
                </>
              )}
            </CardContent>
          </Card>

          <Drawer
            open={selectedRun !== null}
            onOpenChange={(open) => {
              if (!open) setSelectedRun(null)
            }}
            direction='right'
          >
            <DrawerContent side='right'>
              <DrawerTitle className='sr-only'>
                {t('integrity.report', 'Report')}
              </DrawerTitle>
              <div className='flex h-full flex-col overflow-hidden p-3'>
                {selectedRun && (
                  <Report
                    runId={selectedRun}
                    onClose={() => setSelectedRun(null)}
                  />
                )}
              </div>
            </DrawerContent>
          </Drawer>
        </div>
      </Main>
    </>
  )
}
