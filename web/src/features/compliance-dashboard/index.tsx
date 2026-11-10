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
// Compliance dashboard (Enterprise).
//
// Six compliance capabilities each have their own page. This is the one screen
// that answers "what is our compliance state right now" without opening all
// six — the question an auditor asks first.
//
// Strictly read-only: nothing here changes state. Every operation it reports
// on is performed on the page that owns it, which keeps this page safe to show
// to a compliance officer whose entire permission set is `compliance:audit`.
import { useState } from 'react'
import { useMutation, useQuery } from '@tanstack/react-query'
import { useTranslation } from 'react-i18next'
import type { TFunction } from 'i18next'
import {
  Activity,
  CheckCircle2,
  Clock,
  Download,
  FileCheck2,
  Fingerprint,
  Lock,
  ShieldAlert,
  Users,
} from 'lucide-react'
import { download_evidence_pack } from '@/api/compliance-evidence/api'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { toast } from '@/hooks/use-toast'
import {
  get_compliance_overview,
  type ComplianceOverview,
} from '@/api/compliance-overview/api'
import { Badge } from '@/components/ui/badge'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { TableSkeleton } from '@/components/table-skeleton'
import { FixedHeader } from '@/components/layout/fixed-header'
import { Main } from '@/components/layout/main'
import { useEdition } from '@/hooks/use-edition'
import { useCurrentUser } from '@/hooks/use-current-user'
import { formatNumber } from '@/lib/utils'

function formatTime(ts: number): string {
  const d = new Date(ts)
  const pad = (n: number) => String(n).padStart(2, '0')
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`
}

/** A label-over-value pair, matching the shape the integrity report uses. */
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

/** A card that says "nothing here yet" rather than showing a zero that reads
 *  like a measurement. On a fresh install most of these cards are empty, and
 *  "0" invites the question "is that a count, or is it broken?". */
function EmptyNote({ children }: { children: React.ReactNode }) {
  return <p className='text-xs text-muted-foreground'>{children}</p>
}

function SectionCard({
  title,
  icon: Icon,
  children,
}: {
  title: string
  icon: React.ElementType
  children: React.ReactNode
}) {
  return (
    <Card>
      <CardHeader className='flex flex-row items-center justify-between space-y-0 pb-2'>
        <CardTitle className='text-sm font-medium'>{title}</CardTitle>
        <Icon className='h-4 w-4 text-muted-foreground' />
      </CardHeader>
      <CardContent className='space-y-3'>{children}</CardContent>
    </Card>
  )
}

/** The run statuses the integrity page already names. Kept local rather than
 *  shared: that page has its own badge with its own spinner, and this is only
 *  the label half of it. */
function statusLabel(t: TFunction, status: string): string {
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

function IntegrityCard({ data }: { data: ComplianceOverview['integrity'] }) {
  const { t } = useTranslation()
  const run = data.last_run
  if (!run) {
    return (
      <EmptyNote>
        {t(
          'complianceDashboard.integrityNeverRun',
          'No integrity check has run yet.'
        )}
      </EmptyNote>
    )
  }
  // `failed` here is the run's own outcome, not a count of bad messages. A run
  // that never got started (no accounts to check, on a fresh install) records
  // `status: "failed"` with `failed: 0` — the badge must follow the status or
  // an empty install reads as "the archive is corrupt".
  const clean = run.status === 'finished' && run.failed === 0
  const bad = run.status === 'failed' || run.failed > 0
  return (
    <>
      <div className='flex items-center gap-2'>
        <Badge variant={clean ? 'default' : bad ? 'destructive' : 'secondary'}>
          {clean ? (
            <CheckCircle2 className='mr-1 h-3 w-3' />
          ) : bad ? (
            <ShieldAlert className='mr-1 h-3 w-3' />
          ) : null}
          {statusLabel(t, run.status)}
        </Badge>
        <span className='text-xs text-muted-foreground'>
          {run.mode === 'quick'
            ? t('integrity.modeQuick', 'Quick (check email blobs exist)')
            : run.mode === 'full'
              ? t('integrity.modeFull', 'Full (recompute content hashes)')
              : run.mode}
        </span>
      </div>
      <div className='grid grid-cols-2 gap-3'>
        <Stat label={t('complianceDashboard.checked', 'Messages checked')}>
          <span className='tabular-nums'>{formatNumber(run.total)}</span>
        </Stat>
        <Stat label={t('complianceDashboard.failed', 'Failed')}>
          <span
            className={
              clean
                ? 'tabular-nums'
                : 'tabular-nums text-destructive dark:text-red-400'
            }
          >
            {formatNumber(run.failed)}
          </span>
        </Stat>
      </div>
      <EmptyNote>
        {run.finished_at
          ? t('complianceDashboard.lastRunAt', 'Last run {{time}}', {
              time: formatTime(run.finished_at),
            })
          : t('complianceDashboard.lastRunStarted', 'Started {{time}}', {
              time: formatTime(run.started_at),
            })}
        {' · '}
        {t('complianceDashboard.runsOnRecord', '{{count}} run(s) on record', {
          count: data.total_runs,
        })}
      </EmptyNote>
    </>
  )
}

function HoldCard({ data }: { data: ComplianceOverview['legal_hold'] }) {
  const { t } = useTranslation()
  if (data.total_accounts === 0) {
    return (
      <EmptyNote>
        {t(
          'complianceDashboard.noAccounts',
          'No accounts are configured yet.'
        )}
      </EmptyNote>
    )
  }
  return (
    <>
      <div className='grid grid-cols-2 gap-3'>
        <Stat label={t('complianceDashboard.held', 'Accounts on hold')}>
          <span className='tabular-nums'>{formatNumber(data.held_accounts)}</span>
        </Stat>
        <Stat label={t('complianceDashboard.coverage', 'Of all accounts')}>
          <span className='tabular-nums'>{data.covered_pct.toFixed(2)}%</span>
        </Stat>
      </div>
      {data.latest_placed_at ? (
        <EmptyNote>
          {t('complianceDashboard.latestHold', 'Most recent hold placed {{time}}', {
            time: formatTime(data.latest_placed_at),
          })}
          {data.latest_placed_reason ? ` — ${data.latest_placed_reason}` : ''}
        </EmptyNote>
      ) : (
        <EmptyNote>
          {t('complianceDashboard.noHolds', 'No account is currently on hold.')}
        </EmptyNote>
      )}
    </>
  )
}

function ExportCard({ data }: { data: ComplianceOverview['exports'] }) {
  const { t } = useTranslation()
  if (data.total === 0) {
    return (
      <EmptyNote>
        {t('complianceDashboard.noExports', 'No export has been produced yet.')}
      </EmptyNote>
    )
  }
  const bad = data.verified_mismatched > 0 || data.verification_failed > 0
  return (
    <>
      <div className='grid grid-cols-3 gap-3'>
        <Stat label={t('complianceDashboard.verifiedOk', 'Verified')}>
          <span className='tabular-nums'>{formatNumber(data.verified_ok)}</span>
        </Stat>
        <Stat label={t('complianceDashboard.mismatched', 'Mismatched')}>
          <span
            className={
              data.verified_mismatched > 0
                ? 'tabular-nums text-destructive dark:text-red-400'
                : 'tabular-nums'
            }
          >
            {formatNumber(data.verified_mismatched)}
          </span>
        </Stat>
        <Stat label={t('complianceDashboard.verificationFailed', 'Check failed')}>
          <span
            className={
              data.verification_failed > 0
                ? 'tabular-nums text-destructive dark:text-red-400'
                : 'tabular-nums'
            }
          >
            {formatNumber(data.verification_failed)}
          </span>
        </Stat>
      </div>
      <EmptyNote>
        {t('complianceDashboard.exportsTotal', '{{count}} export(s) in total.', {
          count: data.total,
        })}
        {` ${t('complianceDashboard.unverified', 'Unverified')}: ${formatNumber(
          data.unverified
        )}`}
        {data.verifying > 0
          ? ` · ${t('complianceDashboard.verifying', 'checking now')}: ${formatNumber(
              data.verifying
            )}`
          : ''}
      </EmptyNote>
      {bad ? (
        <EmptyNote>
          {t(
            'complianceDashboard.exportBadHint',
            'At least one export does not verify against its own hashes. Re-run the check from the Compliance Export page before relying on it as evidence.'
          )}
        </EmptyNote>
      ) : null}
      {data.latest_finished_at ? (
        <EmptyNote>
          {t('complianceDashboard.latestExport', 'Most recent {{time}}', {
            time: formatTime(data.latest_finished_at),
          })}
        </EmptyNote>
      ) : null}
    </>
  )
}

function ApprovalCard({ data }: { data: ComplianceOverview['approvals'] }) {
  const { t } = useTranslation()
  if (!data.enabled) {
    return (
      <EmptyNote>
        {t(
          'complianceDashboard.dualControlOff',
          'Dual control is switched off. Destructive operations run without a second person.'
        )}
      </EmptyNote>
    )
  }
  return (
    <>
      <div className='grid grid-cols-2 gap-3'>
        <Stat label={t('complianceDashboard.pending', 'Awaiting a decision')}>
          <span className='tabular-nums'>{formatNumber(data.pending)}</span>
        </Stat>
        <Stat label={t('complianceDashboard.gatedOps', 'Gated operations')}>
          <span className='tabular-nums'>{formatNumber(data.gated_ops)}</span>
        </Stat>
      </div>
      <EmptyNote>
        {t(
          'complianceDashboard.breakGlassTotal',
          'Break-glass overrides used: {{count}}',
          { count: data.break_glass_total }
        )}
        {data.break_glass_min_hours < 0
          ? ` — ${t(
              'complianceDashboard.breakGlassOff',
              'the override is disabled'
            )}`
          : ` — ${t(
              'complianceDashboard.breakGlassWait',
              'available after {{hours}}h pending',
              { hours: data.break_glass_min_hours }
            )}`}
      </EmptyNote>
    </>
  )
}

function TimestampCard({ data }: { data: ComplianceOverview['timestamps'] }) {
  const { t } = useTranslation()
  const latest = data.latest
  if (!latest) {
    return (
      <EmptyNote>
        {t(
          'complianceDashboard.noAnchors',
          'Nothing has been anchored to a timestamp authority yet.'
        )}
      </EmptyNote>
    )
  }
  return (
    <>
      <div className='grid grid-cols-2 gap-3'>
        <Stat label={t('complianceDashboard.anchors', 'Anchors')}>
          <span className='tabular-nums'>{formatNumber(data.anchor_count)}</span>
        </Stat>
        <Stat label={t('complianceDashboard.leaves', 'Messages covered')}>
          <span className='tabular-nums'>{formatNumber(latest.leaf_count)}</span>
        </Stat>
      </div>
      <EmptyNote>
        {t('complianceDashboard.lastAnchored', 'Last anchored {{time}}', {
          time: formatTime(latest.gen_time),
        })}
        {latest.tsa_url
          ? ` — ${latest.tsa_url}`
          : ` — ${t(
              'complianceDashboard.noTsa',
              'local only, no timestamp authority configured'
            )}`}
      </EmptyNote>
    </>
  )
}

/** The one card that exists to surface a defect rather than a status. */
function AuditCard({ data }: { data: ComplianceOverview['audit'] }) {
  const { t } = useTranslation()
  const dropped = data.dropped > 0
  return (
    <>
      <div className='flex items-center gap-2'>
        <Badge variant={dropped ? 'destructive' : 'default'}>
          {dropped ? (
            <ShieldAlert className='mr-1 h-3 w-3' />
          ) : (
            <CheckCircle2 className='mr-1 h-3 w-3' />
          )}
          {dropped
            ? t('complianceDashboard.chainIncomplete', 'Incomplete')
            : t('complianceDashboard.chainIntact', 'Intact')}
        </Badge>
      </div>
      <div className='grid grid-cols-2 gap-3'>
        <Stat label={t('complianceDashboard.dropped', 'Events dropped')}>
          <span
            className={
              dropped
                ? 'tabular-nums text-destructive dark:text-red-400'
                : 'tabular-nums'
            }
          >
            {formatNumber(data.dropped)}
          </span>
        </Stat>
        <Stat label={t('complianceDashboard.pendingEvents', 'Buffered')}>
          <span className='tabular-nums'>{formatNumber(data.pending)}</span>
        </Stat>
      </div>
      <EmptyNote>
        {dropped
          ? t(
              'complianceDashboard.droppedHint',
              'The audit channel overflowed and these events never reached the log. Treat the chain as having a gap.'
            )
          : t(
              'complianceDashboard.retentionHint',
              'Audit records are kept for {{days}} day(s).',
              {
                days:
                  data.retention_days > 0
                    ? String(data.retention_days)
                    : t('complianceDashboard.retentionOff', 'unlimited'),
              }
            )}
      </EmptyNote>
    </>
  )
}

export default function ComplianceDashboardPage() {
  const { t } = useTranslation()
  const { isEnterprise } = useEdition()
  const { require_any_permission } = useCurrentUser()

  // Same read gate as the audit log and the approval queue: the read-only
  // compliance officer is the intended reader, so `compliance:audit` suffices.
  const canView =
    isEnterprise &&
    require_any_permission(['compliance:audit', 'system:root', 'user:manage'])

  const { data, isLoading } = useQuery({
    queryKey: ['compliance-overview'],
    queryFn: get_compliance_overview,
    enabled: canView,
  })

  // ---- evidence pack ---------------------------------------------------
  const today = new Date()
  const isoDate = (d: Date) => d.toISOString().slice(0, 10)
  const [evidenceOpen, setEvidenceOpen] = useState(false)
  const [fromDate, setFromDate] = useState(() => {
    const d = new Date(today)
    d.setDate(d.getDate() - 30)
    return isoDate(d)
  })
  const [toDate, setToDate] = useState(() => isoDate(today))
  const evidenceMutation = useMutation({
    mutationFn: () =>
      download_evidence_pack(
        new Date(`${fromDate}T00:00:00`).getTime(),
        new Date(`${toDate}T23:59:59.999`).getTime()
      ),
    onSuccess: () => {
      setEvidenceOpen(false)
      toast({
        title: t(
          'complianceDashboard.evidencePackReady',
          'Evidence pack downloaded'
        ),
      })
    },
    onError: (e: Error) => {
      toast({
        title: t(
          'complianceDashboard.evidencePackFailed',
          'Could not build the evidence pack'
        ),
        description: e.message,
        variant: 'destructive',
      })
    },
  })

  if (!canView) {
    return (
      <>
        <FixedHeader />
        <Main>
          <div className='mx-auto w-full max-w-7xl px-4 py-16 text-center text-muted-foreground'>
            {t(
              'complianceDashboard.forbidden',
              'The compliance dashboard requires the Enterprise edition with the compliance:audit, system:root or user:manage permission.'
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
        <div className='mx-auto w-full max-w-7xl space-y-6 px-4'>
          <div className='flex flex-wrap items-start justify-between gap-3'>
            <div>
              <h1 className='text-lg font-semibold'>
                {t('complianceDashboard.title', 'Compliance dashboard')}
              </h1>
              <p className='text-xs text-muted-foreground'>
                {t(
                  'complianceDashboard.subtitle',
                  'The current state of every compliance control in one place. Read-only — each control is configured on its own page.'
                )}
              </p>
            </div>
            <Button
              variant='outline'
              size='sm'
              onClick={() => setEvidenceOpen(true)}
            >
              <Download className='mr-1 h-4 w-4' />
              {t(
                'complianceDashboard.evidencePack',
                'Export evidence pack'
              )}
            </Button>
          </div>

          {isLoading || !data ? (
            <TableSkeleton columns={3} rows={4} showPagination={false} />
          ) : (
            <>
              <div className='grid gap-4 sm:grid-cols-2 lg:grid-cols-3'>
                <SectionCard
                  title={t('complianceDashboard.integrity', 'Integrity')}
                  icon={FileCheck2}
                >
                  <IntegrityCard data={data.integrity} />
                </SectionCard>

                <SectionCard
                  title={t('complianceDashboard.legalHold', 'Legal hold')}
                  icon={Lock}
                >
                  <HoldCard data={data.legal_hold} />
                </SectionCard>

                <SectionCard
                  title={t('complianceDashboard.exports', 'Exports')}
                  icon={FileCheck2}
                >
                  <ExportCard data={data.exports} />
                </SectionCard>

                <SectionCard
                  title={t('complianceDashboard.dualControl', 'Dual control')}
                  icon={Users}
                >
                  <ApprovalCard data={data.approvals} />
                </SectionCard>

                <SectionCard
                  title={t(
                    'complianceDashboard.timestamps',
                    'Timestamp anchoring'
                  )}
                  icon={Fingerprint}
                >
                  <TimestampCard data={data.timestamps} />
                </SectionCard>

                <SectionCard
                  title={t('complianceDashboard.audit', 'Audit log')}
                  icon={Activity}
                >
                  <AuditCard data={data.audit} />
                </SectionCard>
              </div>

              <p className='flex items-center gap-1.5 text-xs text-muted-foreground'>
                <Clock className='h-3 w-3' />
                {t('complianceDashboard.generatedAt', 'Snapshot taken {{time}}', {
                  time: formatTime(data.generated_at),
                })}
              </p>
            </>
          )}
        </div>

        <Dialog open={evidenceOpen} onOpenChange={setEvidenceOpen}>
          <DialogContent className='sm:max-w-md'>
            <DialogHeader>
              <DialogTitle>
                {t(
                  'complianceDashboard.evidencePackTitle',
                  'Export an evidence pack'
                )}
              </DialogTitle>
              <DialogDescription>
                {t(
                  'complianceDashboard.evidencePackDesc',
                  'Download a ZIP with the audit records in this range, the chain-verification result, legal-hold cases, retention policy, the latest integrity run and the timestamp anchors.'
                )}
              </DialogDescription>
            </DialogHeader>
            <div className='grid grid-cols-2 gap-3'>
              <div className='space-y-2'>
                <Label>{t('complianceDashboard.from', 'From')}</Label>
                <Input
                  type='date'
                  value={fromDate}
                  max={toDate}
                  onChange={(e) => setFromDate(e.target.value)}
                />
              </div>
              <div className='space-y-2'>
                <Label>{t('complianceDashboard.to', 'To')}</Label>
                <Input
                  type='date'
                  value={toDate}
                  min={fromDate}
                  onChange={(e) => setToDate(e.target.value)}
                />
              </div>
            </div>
            <DialogFooter>
              <Button variant='outline' onClick={() => setEvidenceOpen(false)}>
                {t('common.cancel')}
              </Button>
              <Button
                disabled={!fromDate || !toDate || evidenceMutation.isPending}
                onClick={() => evidenceMutation.mutate()}
              >
                <Download className='mr-1 h-4 w-4' />
                {evidenceMutation.isPending
                  ? t('complianceDashboard.building', 'Building…')
                  : t('complianceDashboard.download', 'Download')}
              </Button>
            </DialogFooter>
          </DialogContent>
        </Dialog>
      </Main>
    </>
  )
}
