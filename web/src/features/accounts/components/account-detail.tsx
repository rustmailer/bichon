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

import { ReactNode } from 'react'
import { useTranslation } from 'react-i18next'
import { ScrollArea } from '@/components/ui/scroll-area'
import { Badge } from '@/components/ui/badge'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { formatTimestamp } from '@/lib/utils'
import { AccountModel, FilterRule } from '@/api/account/api'
import { useEdition } from '@/hooks/use-edition'
import useProxyList from '@/hooks/use-proxy'

interface Props {
  open: boolean
  onOpenChange: (open: boolean) => void
  currentRow: AccountModel
}

// One label/value row of a definition list. Everything is text-xs so the
// sheet reads as a single consistent spec sheet.
function InfoRow({ label, children }: { label: ReactNode; children?: ReactNode }) {
  return (
    <div className='grid grid-cols-1 gap-y-0.5 px-4 py-2.5 sm:grid-cols-[160px_1fr] sm:gap-x-4'>
      <dt className='text-xs text-muted-foreground'>{label}</dt>
      <dd className='break-words text-xs'>{children ?? '—'}</dd>
    </div>
  )
}

function EnabledBadge({ enabled }: { enabled: boolean }) {
  const { t } = useTranslation()
  return enabled ? (
    <Badge className='border-emerald-200 bg-emerald-100 text-emerald-800'>
      {t('accounts.enabled', 'Enabled')}
    </Badge>
  ) : (
    <Badge variant='secondary'>{t('accounts.disabled', 'Disabled')}</Badge>
  )
}

function TypeBadge({ type }: { type?: string }) {
  const { t } = useTranslation()
  return (
    <Badge variant='outline'>
      {type === 'NoSync'
        ? t('accounts.noSyncAccount', 'Local account')
        : type === 'IMAP'
          ? t('accounts.imap', 'IMAP')
          : (type ?? '')}
    </Badge>
  )
}

function HoldBadge({ on }: { on: boolean }) {
  const { t } = useTranslation()
  if (!on) return <span className='text-xs text-muted-foreground'>—</span>
  return (
    <Badge className='border-amber-200 bg-amber-100 text-amber-800'>
      {t('accounts.legalHoldBadge', 'Legal hold')}
    </Badge>
  )
}

// Renders an include/exclude pattern list, or a dash when both are empty.
function FilterRules({ rule }: { rule?: FilterRule }) {
  const { t } = useTranslation()
  const include = rule?.include ?? []
  const exclude = rule?.exclude ?? []
  if (!include.length && !exclude.length) {
    return <span className='text-muted-foreground'>—</span>
  }
  return (
    <div className='space-y-1'>
      {include.length > 0 && (
        <div>
          <span className='text-muted-foreground'>
            {t('accounts.filters.include', 'Include')}:{' '}
          </span>
          {include.join(', ')}
        </div>
      )}
      {exclude.length > 0 && (
        <div>
          <span className='text-muted-foreground'>
            {t('accounts.filters.exclude', 'Exclude')}:{' '}
          </span>
          {exclude.join(', ')}
        </div>
      )}
    </div>
  )
}

function mb(bytes?: number): string {
  return bytes ? `${Math.round(bytes / 1024 / 1024)} MB` : '—'
}

export function AccountDetailDrawer({ open, onOpenChange, currentRow }: Props) {
  const { t } = useTranslation()
  const { isPro, isEnterprise, isLoading: editionLoading } = useEdition()
  const { getUrlById } = useProxyList()
  const row = currentRow
  // Provider accounts exist in the shared account model, but only a Pro /
  // Enterprise build has a connector for them. A community build (or a Pro
  // build whose licence lapsed) starts fine and leaves the archive intact;
  // this notice is the only explanation the operator gets (R-BC-05 / R-ED-04).
  const isProviderAccount =
    row.account_type === 'Graph' || row.account_type === 'Gmail'
  const showProviderNotice = !editionLoading && !isPro && isProviderAccount

  const unitLabel = (unit?: string) =>
    unit ? t(`accounts.${unit.toLowerCase()}`, unit.toLowerCase()) : ''

  const hasSince = !!row.date_since
  const hasBefore = !!row.date_before?.value

  const sinceText = (() => {
    if (row.date_since?.fixed) return row.date_since.fixed
    if (row.date_since?.relative?.value) {
      return t('accounts.lastRelativeValue', 'last {{value}} {{unit}}', {
        value: row.date_since.relative.value,
        unit: unitLabel(row.date_since.relative.unit),
      })
    }
    return ''
  })()

  const beforeText = hasBefore
    ? t('accounts.beforeRelativeValue', 'older than {{value}} {{unit}}', {
        value: row.date_before!.value,
        unit: unitLabel(row.date_before!.unit),
      })
    : ''

  const proxyLabel = (() => {
    if (!row.imap?.use_proxy) return t('accounts.useNoProxy', 'No proxy')
    return getUrlById(row.imap.use_proxy) || `#${row.imap.use_proxy}`
  })()

  const creator =
    row.created_user_name && row.created_user_email
      ? `${row.created_user_name} · ${row.created_user_email}`
      : (row.created_user_name ?? row.created_user_email ?? '—')

  const capabilities = row.capabilities ?? []

  const quotaWindowLabel = (() => {
    if (!row.imap_quota_window) return null
    const labels: Record<string, string> = {
      hourly: t('accounts.quotaWindowHourly', 'Hourly'),
      daily: t('accounts.quotaWindowDaily', 'Daily'),
      weekly: t('accounts.quotaWindowWeekly', 'Weekly'),
      monthly: t('accounts.quotaWindowMonthly', 'Monthly'),
    }
    return labels[row.imap_quota_window] ?? row.imap_quota_window
  })()

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className='max-w-5xl'>
        <DialogHeader className='mb-1 text-left'>
          <DialogTitle className='flex flex-wrap items-center gap-2 text-base'>
            {row.email}
            <TypeBadge type={row.account_type} />
            <EnabledBadge enabled={!!row.enabled} />
            {isEnterprise && <HoldBadge on={!!row.legal_hold} />}
          </DialogTitle>
          <DialogDescription>
            {t('accounts.accountDetails', 'Account details')}
          </DialogDescription>
        </DialogHeader>
        {showProviderNotice && (
          <div className='rounded-md border border-amber-200 bg-amber-50 px-3 py-2 text-xs text-amber-900'>
            {t(
              'accounts.providerRequiresPro',
              'Microsoft 365 and Google Workspace accounts require Bichon Pro or Enterprise. Syncing is paused; mail already archived stays available.'
            )}
          </div>
        )}
        <ScrollArea className='h-[34rem] w-full pr-4 -mr-4 py-1'>
          <div className='space-y-4'>
            <Card>
              <CardHeader className='border-b py-3'>
                <CardTitle className='text-sm'>
                  {t('accounts.settings.general', 'General')}
                </CardTitle>
              </CardHeader>
              <CardContent className='p-0'>
                <dl className='divide-y divide-border'>
                  <InfoRow label={t('accounts.id', 'ID')}>{row.id}</InfoRow>
                  <InfoRow label={t('accounts.type', 'Type')}>
                    <TypeBadge type={row.account_type} />
                  </InfoRow>
                  <InfoRow label={t('accounts.email', 'Email')}>{row.email}</InfoRow>
                  <InfoRow label={t('accounts.accountName', 'Account name')}>
                    {row.account_name || '—'}
                  </InfoRow>
                  <InfoRow label={t('accounts.login_name', 'Login name')}>
                    {row.login_name || '—'}
                  </InfoRow>
                  <InfoRow label={t('accounts.enabled', 'Enabled')}>
                    <EnabledBadge enabled={!!row.enabled} />
                  </InfoRow>
                  <InfoRow
                    label={t('accounts.autoDownloadNewMailboxes', 'Auto-add new mailboxes')}
                  >
                    {row.auto_download_new_mailboxes
                      ? t('common.yes', 'Yes')
                      : t('common.no', 'No')}
                  </InfoRow>
                  <InfoRow label={t('accounts.retentionDays', 'Retention window (days)')}>
                    {row.retention_days
                      ? `${row.retention_days} ${t('accounts.days', 'Days')}`
                      : t('accounts.retentionDaysPlaceholder', 'Keep everything')}
                  </InfoRow>
                  {isEnterprise && (
                    <InfoRow label={t('accounts.legalHoldBadge', 'Legal hold')}>
                      <HoldBadge on={!!row.legal_hold} />
                    </InfoRow>
                  )}
                  {isEnterprise && row.legal_hold && (
                    <>
                      <InfoRow label={t('accounts.holdReason', 'Hold reason')}>
                        {row.hold_reason || '—'}
                      </InfoRow>
                      <InfoRow label={t('accounts.holdPlacedBy', 'Hold placed by')}>
                        {row.hold_placed_by ? `#${row.hold_placed_by}` : '—'}
                      </InfoRow>
                      <InfoRow label={t('accounts.holdPlacedAt', 'Hold placed at')}>
                        {row.hold_placed_at ? formatTimestamp(row.hold_placed_at) : '—'}
                      </InfoRow>
                    </>
                  )}
                  <InfoRow label={t('accounts.createdBy', 'Created by')}>{creator}</InfoRow>
                  <InfoRow label={t('accounts.createdAt', 'Created At')}>
                    {row.created_at ? formatTimestamp(row.created_at) : '—'}
                  </InfoRow>
                  <InfoRow label={t('accounts.updatedAt', 'Updated At')}>
                    {row.updated_at ? formatTimestamp(row.updated_at) : '—'}
                  </InfoRow>
                </dl>
              </CardContent>
            </Card>

            <div className='grid gap-4 xl:grid-cols-2'>
              <Card>
                <CardHeader className='border-b py-3'>
                  <CardTitle className='text-sm'>
                    {t('accounts.downloadScope', 'Download scope')}
                  </CardTitle>
                </CardHeader>
                <CardContent className='p-0'>
                  <dl className='divide-y divide-border'>
                    <InfoRow label={t('accounts.downloadInterval', 'Download interval')}>
                      {row.download_interval_min != null
                        ? t('accounts.everyMinutes', {
                            minutes: row.download_interval_min,
                          })
                        : '—'}
                    </InfoRow>
                    <InfoRow label={t('accounts.downloadBatchSize', 'Download batch size')}>
                      {row.download_batch_size ?? '—'}
                    </InfoRow>
                    <InfoRow label={t('accounts.maxEmailSizeBytes', 'Max email size')}>
                      {row.max_email_size_bytes
                        ? mb(row.max_email_size_bytes)
                        : t('accounts.maxEmailSizeBytesUnlimited', 'Unlimited')}
                    </InfoRow>
                    <InfoRow label={t('accounts.downloadSchedule', 'Download schedule')}>
                      {row.download_schedule
                        ? (
                          <code className='rounded bg-muted/50 px-1.5 py-0.5 text-xs'>
                            {row.download_schedule}
                          </code>
                        )
                        : '—'}
                    </InfoRow>
                    {hasSince ? (
                      <InfoRow label={t('accounts.since', 'Since')}>{sinceText}</InfoRow>
                    ) : null}
                    {hasBefore ? (
                      <InfoRow label={t('accounts.syncBefore', 'Before')}>{beforeText}</InfoRow>
                    ) : null}
                    {!hasSince && !hasBefore && (
                      <InfoRow label={t('accounts.syncStart', 'Start')}>
                        {t('accounts.downloadAll', 'All mail')}
                      </InfoRow>
                    )}
                    <InfoRow label={t('accounts.imapQuotaWindow', 'IMAP quota window')}>
                      {quotaWindowLabel ?? '—'}
                    </InfoRow>
                    <InfoRow label={t('accounts.imapQuotaBytes', 'IMAP quota')}>
                      {mb(row.imap_quota_bytes)}
                    </InfoRow>
                    <InfoRow label={t('accounts.capabilities', 'Capabilities')}>
                      {capabilities.length > 0 ? (
                        <div className='flex flex-wrap gap-1'>
                          {capabilities.map((c) => (
                            <Badge
                              key={c}
                              variant='outline'
                              className='px-1.5 py-0 font-normal'
                            >
                              {c}
                            </Badge>
                          ))}
                        </div>
                      ) : (
                        '—'
                      )}
                    </InfoRow>
                    <InfoRow label={t('accounts.pgpKey', 'PGP key')}>
                      {row.pgp_key ? (
                        <pre className='max-h-24 overflow-auto rounded border bg-muted/40 p-1.5 text-xs'>
                          {row.pgp_key}
                        </pre>
                      ) : (
                        t('accounts.notConfigured', 'Not configured')
                      )}
                    </InfoRow>
                  </dl>
                </CardContent>
              </Card>

              <Card>
                <CardHeader className='border-b py-3'>
                  <CardTitle className='text-sm'>
                    {t('accounts.serverConfiguration', 'Server configuration')}
                  </CardTitle>
                </CardHeader>
                <CardContent className='p-0'>
                  <dl className='divide-y divide-border'>
                    <InfoRow label={t('accounts.host', 'Host')}>
                      {row.imap?.host || '—'}
                    </InfoRow>
                    <InfoRow label={t('accounts.port', 'Port')}>
                      {row.imap?.port ?? '—'}
                    </InfoRow>
                    <InfoRow label={t('accounts.encryption', 'Encryption')}>
                      {row.imap?.encryption || '—'}
                    </InfoRow>
                    <InfoRow label={t('accounts.auth', 'Auth')}>
                      <Badge className='border-blue-200 bg-blue-100 text-blue-800'>
                        {row.imap?.auth?.auth_type === 'OAuth2' ? 'OAuth2' : 'Password'}
                      </Badge>
                    </InfoRow>
                    <InfoRow label={t('accounts.useDangerous', 'Trust any TLS certificate')}>
                      {row.use_dangerous ? t('common.yes', 'Yes') : t('common.no', 'No')}
                    </InfoRow>
                    <InfoRow label={t('accounts.useProxyField', 'Proxy')}>
                      {proxyLabel}
                    </InfoRow>
                  </dl>
                </CardContent>
              </Card>
            </div>

            <div className={isPro ? 'grid gap-4 xl:grid-cols-2' : ''}>
              <Card>
                <CardHeader className='border-b py-3'>
                  <CardTitle className='text-sm'>
                    {t('accounts.rules.archiveTitle', 'Archive filtering')}
                  </CardTitle>
                </CardHeader>
                <CardContent className='p-0'>
                  {row.archive_rules ? (
                    <dl className='divide-y divide-border'>
                      <InfoRow label={t('accounts.filters.enableFiltering', 'Filtering')}>
                        <EnabledBadge enabled={!!row.archive_rules.enabled} />
                      </InfoRow>
                      <InfoRow label={t('accounts.filters.senderFilter', 'Senders')}>
                        <FilterRules rule={row.archive_rules.senders} />
                      </InfoRow>
                      <InfoRow label={t('accounts.filters.subjectFilter', 'Subjects')}>
                        <FilterRules rule={row.archive_rules.subjects} />
                      </InfoRow>
                      <InfoRow label={t('accounts.filters.skipLargerThan', 'Skip larger than')}>
                        {row.archive_rules.skip_larger_than
                          ? mb(row.archive_rules.skip_larger_than)
                          : t('accounts.filters.noLimit', 'No limit')}
                      </InfoRow>
                      <InfoRow label={t('accounts.filters.spamHeaders', 'Spam headers')}>
                        {row.archive_rules.spam_headers?.length
                          ? row.archive_rules.spam_headers.join(', ')
                          : t('accounts.filters.noSpamHeaders', 'None')}
                      </InfoRow>
                    </dl>
                  ) : (
                    <p className='px-4 py-3 text-xs text-muted-foreground'>
                      {t('accounts.notConfigured', 'Not configured')}
                    </p>
                  )}
                </CardContent>
              </Card>

              {isPro && (
                <Card>
                  <CardHeader className='border-b py-3'>
                    <CardTitle className='text-sm'>
                      {t('accounts.rules.extractionTitle', 'Attachment extraction')}
                    </CardTitle>
                  </CardHeader>
                  <CardContent className='p-0'>
                    {row.extraction_rules ? (
                      <dl className='divide-y divide-border'>
                        <InfoRow label={t('accounts.filters.enableFiltering', 'Filtering')}>
                          <EnabledBadge enabled={!!row.extraction_rules.enabled} />
                        </InfoRow>
                        <InfoRow label={t('accounts.filters.extraction.extensions', 'Extensions')}>
                          <FilterRules rule={row.extraction_rules.extensions} />
                        </InfoRow>
                        <InfoRow label={t('accounts.filters.extraction.folders', 'Folders')}>
                          <FilterRules rule={row.extraction_rules.folders} />
                        </InfoRow>
                        <InfoRow
                          label={t('accounts.filters.extraction.attachmentNames', 'Attachment names')}
                        >
                          <FilterRules rule={row.extraction_rules.attachment_names} />
                        </InfoRow>
                        <InfoRow label={t('accounts.filters.extraction.senders', 'Senders')}>
                          <FilterRules rule={row.extraction_rules.senders} />
                        </InfoRow>
                      </dl>
                    ) : (
                      <p className='px-4 py-3 text-xs text-muted-foreground'>
                        {t('accounts.notConfigured', 'Not configured')}
                      </p>
                    )}
                  </CardContent>
                </Card>
              )}
            </div>

            <Card>
              <CardHeader className='border-b py-3'>
                <CardTitle className='text-sm'>
                  {t('accounts.selectedMailboxes', 'Selected mailboxes')}
                </CardTitle>
              </CardHeader>
              <CardContent className='p-3'>
                {row.download_folders?.length ? (
                  <div className='space-y-2'>
                    <div className='text-xs text-muted-foreground'>
                      {t('accounts.foldersConfiguredForSync', {
                        count: row.download_folders.length,
                      })}
                    </div>
                    <ScrollArea className='max-h-56 rounded-md border'>
                      <div className='p-1.5'>
                        {row.download_folders.map((folder, index) => (
                          <div
                            key={index}
                            className='flex items-center rounded-md px-2.5 py-1.5 text-xs hover:bg-accent'
                          >
                            <span className='truncate'>{folder}</span>
                          </div>
                        ))}
                      </div>
                    </ScrollArea>
                  </div>
                ) : (
                  <div className='py-6 text-center text-xs text-muted-foreground'>
                    {t('accounts.folderSync.noFolders', 'No folders to sync')}
                  </div>
                )}
              </CardContent>
            </Card>
          </div>
        </ScrollArea>
      </DialogContent>
    </Dialog>
  )
}
