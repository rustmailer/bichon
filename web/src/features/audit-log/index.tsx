//
// Copyright (c) 2025-2026 rustmailer.com (https://rustmailer.com)
//
// Audit log page (Pro edition) — query who did what, when.
//

import { Fragment, useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import { useTranslation } from 'react-i18next'
import { ChevronDown } from 'lucide-react'
import { Main } from '@/components/layout/main'
import { FixedHeader } from '@/components/layout/fixed-header'
import { TablePagination } from '@/components/pagination'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { cn } from '@/lib/utils'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import { TableSkeleton } from '@/components/table-skeleton'
import { DatePicker } from '@/components/date-picker'
import { VirtualizedSelect } from '@/components/virtualized-select'
import { list_audit_log } from '@/api/audit/api'
import { describeEvent } from './describe-event'
import { list_minimal_users } from '@/api/users/api'
import { minimal_account_list } from '@/api/account/api'
import { useEdition } from '@/hooks/use-edition'
import { useCurrentUser } from '@/hooks/use-current-user'
import { Separator } from '@/components/ui/separator'

const PAGE_SIZE = 50

const EVENT_TYPES = [
  'email.viewed',
  'email.deleted',
  'email.batch_deleted',
  'email.exported',
  'email.restored',
  'email.repaired',
  'email.tagged',
  'attachment.downloaded',
  'attachment.previewed',
  'attachment.tagged',
  'user.login',
  'user.created',
  'user.updated',
  'user.removed',
  'role.created',
  'role.updated',
  'role.removed',
  'account.created',
  'account.updated',
  'account.removed',
  'account.download_started',
  'account.download_stopped',
  'account.role_assigned',
  'access_token.created',
  'access_token.removed',
  'oauth2.created',
  'oauth2.updated',
  'oauth2.removed',
  'oauth2.token_stored',
  'import.performed',
  'mailbox.removed',
  'proxy.created',
  'proxy.updated',
  'proxy.removed',
  'sso.login',
  'sso.logout',
  'sso.config_updated',
  'mfa.enabled',
  'mfa.disabled',
  'mfa.reset_by_admin',
  'license.uploaded',
  'branding.updated',
  'search.performed',
  'settings.changed',
  'timestamp.anchored',
  'ldap.login',
  'ldap.login_failed',
  'siem.forward_failed',
  'siem.config_updated',
  'approval.requested',
  'approval.approved',
  'approval.executed',
  'approval.execution_failed',
  'approval.rejected',
  'approval.expired',
] as const

function eventTypeLabel(t: (key: string, defaultValue: string) => string, et: string): string {
  const labels: Record<string, string> = {
    'email.viewed': t('audit.eventTypes.emailViewed', 'Email viewed'),
    'email.deleted': t('audit.eventTypes.emailDeleted', 'Email deleted'),
    'email.batch_deleted': t(
      'audit.eventTypes.emailBatchDeleted',
      'Bulk email deletion',
    ),
    'email.exported': t('audit.eventTypes.emailExported', 'Email exported'),
    'email.restored': t('audit.eventTypes.emailRestored', 'Email restored'),
    'email.repaired': t('audit.eventTypes.emailRepaired', 'Email repaired'),
    'email.tagged': t('audit.eventTypes.emailTagged', 'Email tags changed'),
    'attachment.downloaded': t('audit.eventTypes.attachmentDownloaded', 'Attachment downloaded'),
    'attachment.previewed': t('audit.eventTypes.attachmentPreviewed', 'Attachment previewed'),
    'attachment.tagged': t('audit.eventTypes.attachmentTagged', 'Attachment tags changed'),
    'user.login': t('audit.eventTypes.userLogin', 'User login'),
    'user.created': t('audit.eventTypes.userCreated', 'User created'),
    'user.updated': t('audit.eventTypes.userUpdated', 'User updated'),
    'user.removed': t('audit.eventTypes.userRemoved', 'User removed'),
    'role.created': t('audit.eventTypes.roleCreated', 'Role created'),
    'role.updated': t('audit.eventTypes.roleUpdated', 'Role updated'),
    'role.removed': t('audit.eventTypes.roleRemoved', 'Role removed'),
    'account.created': t('audit.eventTypes.accountCreated', 'Account created'),
    'account.updated': t('audit.eventTypes.accountUpdated', 'Account updated'),
    'account.removed': t('audit.eventTypes.accountRemoved', 'Account removed'),
    'account.download_started': t(
      'audit.eventTypes.accountDownloadStarted',
      'Account sync started',
    ),
    'account.download_stopped': t(
      'audit.eventTypes.accountDownloadStopped',
      'Account sync stopped',
    ),
    'account.role_assigned': t('audit.eventTypes.accountRoleAssigned', 'Account access assigned'),
    'access_token.created': t('audit.eventTypes.accessTokenCreated', 'Access token created'),
    'access_token.removed': t('audit.eventTypes.accessTokenRemoved', 'Access token removed'),
    'oauth2.created': t('audit.eventTypes.oauth2Created', 'OAuth2 config created'),
    'oauth2.updated': t('audit.eventTypes.oauth2Updated', 'OAuth2 config updated'),
    'oauth2.removed': t('audit.eventTypes.oauth2Removed', 'OAuth2 config removed'),
    'oauth2.token_stored': t('audit.eventTypes.oauth2TokenStored', 'OAuth2 token stored'),
    'import.performed': t('audit.eventTypes.importPerformed', 'Import performed'),
    'mailbox.removed': t('audit.eventTypes.mailboxRemoved', 'Mailbox removed'),
    'proxy.created': t('audit.eventTypes.proxyCreated', 'Proxy created'),
    'proxy.updated': t('audit.eventTypes.proxyUpdated', 'Proxy updated'),
    'proxy.removed': t('audit.eventTypes.proxyRemoved', 'Proxy removed'),
    'sso.login': t('audit.eventTypes.ssoLogin', 'SSO login'),
    'sso.logout': t('audit.eventTypes.ssoLogout', 'SSO logout'),
    'sso.config_updated': t(
      'audit.eventTypes.ssoConfigUpdated',
      'SSO configuration updated',
    ),
    'mfa.enabled': t('audit.eventTypes.mfaEnabled', 'Two-factor authentication enabled'),
    'mfa.disabled': t('audit.eventTypes.mfaDisabled', 'Two-factor authentication disabled'),
    'mfa.reset_by_admin': t(
      'audit.eventTypes.mfaResetByAdmin',
      'Two-factor authentication reset by admin',
    ),
    'license.uploaded': t('audit.eventTypes.licenseUploaded', 'License uploaded'),
    'branding.updated': t('audit.eventTypes.brandingUpdated', 'Branding updated'),
    'search.performed': t('audit.eventTypes.searchPerformed', 'Search performed'),
    'settings.changed': t('audit.eventTypes.settingsChanged', 'Settings changed'),
    'timestamp.anchored': t(
      'audit.eventTypes.timestampAnchored',
      'Merkle root timestamped',
    ),
    'ldap.login': t('audit.eventTypes.ldapLogin', 'LDAP login'),
    'ldap.login_failed': t(
      'audit.eventTypes.ldapLoginFailed',
      'LDAP login failed',
    ),
    'siem.forward_failed': t(
      'audit.eventTypes.siemForwardFailed',
      'SIEM forward failed',
    ),
    'siem.config_updated': t(
      'audit.eventTypes.siemConfigUpdated',
      'SIEM configuration updated',
    ),
    'approval.requested': t(
      'audit.eventTypes.approvalRequested',
      'Approval requested',
    ),
    'approval.approved': t(
      'audit.eventTypes.approvalApproved',
      'Approval granted',
    ),
    'approval.executed': t(
      'audit.eventTypes.approvalExecuted',
      'Approval executed',
    ),
    'approval.execution_failed': t(
      'audit.eventTypes.approvalExecutionFailed',
      'Approval execution failed',
    ),
    'approval.rejected': t(
      'audit.eventTypes.approvalRejected',
      'Approval rejected',
    ),
    'approval.expired': t(
      'audit.eventTypes.approvalExpired',
      'Approval request expired',
    ),
  }
  return labels[et] ?? et
}

function formatTime(ts: number): string {
  const d = new Date(ts)
  const pad = (n: number) => String(n).padStart(2, '0')
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`
}


export default function AuditLog() {
  const { t } = useTranslation()
  const { isPro } = useEdition()
  const { require_any_permission } = useCurrentUser()
  const [page, setPage] = useState(1)
  const [pageSize, setPageSize] = useState(
    () =>
      Number(localStorage.getItem('bichon_audit_log_page_size')) || PAGE_SIZE
  )
  const [userFilter, setUserFilter] = useState('all')
  const [eventType, setEventType] = useState('all')
  const [accountFilter, setAccountFilter] = useState('all')
  const [jobFilter, setJobFilter] = useState('')
  const [startDate, setStartDate] = useState<Date | undefined>()
  const [endDate, setEndDate] = useState<Date | undefined>()
  const [applied, setApplied] = useState({
    user: 'all',
    type: 'all',
    account: 'all',
    jobId: '',
    start: undefined as Date | undefined,
    end: undefined as Date | undefined,
  })

  const { data: users, isLoading: isUsersLoading } = useQuery({
    queryKey: ['audit-log-users'],
    queryFn: () => list_minimal_users(),
    staleTime: 60_000,
  })

  const { data: accounts, isLoading: isAccountsLoading } = useQuery({
    queryKey: ['audit-log-accounts'],
    queryFn: () => minimal_account_list(),
    staleTime: 60_000,
  })

  const userOptions = [
    { value: 'all', label: t('audit.allUsers', 'All') },
    ...(users ?? []).map((u) => ({
      value: u.username,
      label: `${u.username}${u.email ? ` · ${u.email}` : ''}`,
    })),
  ]

  const accountOptions = [
    { value: 'all', label: t('audit.allAccounts', 'All') },
    ...(accounts ?? []).map((a) => ({
      value: String(a.id),
      label: a.email,
    })),
  ]

  const { data, isLoading, isFetching } = useQuery({
    queryKey: ['audit-log', page, pageSize, applied],
    queryFn: () =>
      list_audit_log({
        page,
        page_size: pageSize,
        user: applied.user === 'all' || !applied.user ? undefined : applied.user,
        event_type: applied.type === 'all' || !applied.type ? undefined : applied.type,
        account_id: applied.account === 'all' || !applied.account ? undefined : Number(applied.account),
        job_id: applied.jobId.trim() || undefined,
        start_ms: applied.start ? applied.start.getTime() : undefined,
        end_ms: applied.end
          ? new Date(
            applied.end.getFullYear(),
            applied.end.getMonth(),
            applied.end.getDate() + 1,
          ).getTime()
          : undefined,
      }),
    placeholderData: (prev) => prev,
  })

  const applyFilters = () => {
    setPage(1)
    setApplied({
      user: userFilter,
      type: eventType,
      account: accountFilter,
      jobId: jobFilter,
      start: startDate,
      end: endDate,
    })
  }

  const resetFilters = () => {
    setUserFilter('all')
    setEventType('all')
    setAccountFilter('all')
    setJobFilter('')
    setStartDate(undefined)
    setEndDate(undefined)
    setPage(1)
    setApplied({
      user: 'all',
      type: 'all',
      account: 'all',
      jobId: '',
      start: undefined,
      end: undefined,
    })
  }

  const handlePageSizeChange = (size: number) => {
    localStorage.setItem('bichon_audit_log_page_size', String(size))
    setPage(1)
    setPageSize(size)
  }

  // Which rows currently have their detail row expanded (keyed by record id).
  const [expanded, setExpanded] = useState<Record<string, boolean>>({})

  const toggleExpanded = (id: string) =>
    setExpanded((prev) => ({ ...prev, [id]: !prev[id] }))

  const canView =
    isPro &&
    require_any_permission([
      'system:root',
      'user:manage',
      'data:read:all',
      // Read-only separation-of-duties view (compliance officers).
      'compliance:audit',
    ])

  if (!canView) {
    return (
      <>
        <FixedHeader />
        <Main>
          <div className='mx-auto w-full max-w-7xl px-4 py-16 text-center text-muted-foreground'>
            {t('audit.forbidden', 'Audit log is available in the Pro edition only.')}
          </div>
        </Main>
      </>
    )
  }

  return (
    <>
      <FixedHeader />
      <Main>
        <div className='mx-auto w-full max-w-7xl px-4'>
          <h1 className='mb-4 text-lg font-semibold'>
            {t('audit.title', 'Audit Log')}
          </h1>
          <Separator className='mt-2 mb-4 lg:mt-3 lg:mb-6' />
          {/* Filters */}
          <div className='mb-4 flex flex-wrap items-end gap-2'>
            <div className='flex flex-col gap-1'>
              <label className='text-xs text-muted-foreground'>
                {t('audit.user', 'User')}
              </label>
              <VirtualizedSelect
                options={userOptions}
                value={userFilter}
                onSelectOption={(values) => setUserFilter(values[0])}
                placeholder={t('audit.userPlaceholder', 'username')}
                isLoading={isUsersLoading}
                className='h-9 w-52 justify-start text-sm font-normal'
                noItemsComponent={
                  <span className='text-xs'>{t('audit.noUsers', 'No users found')}</span>
                }
              />
            </div>
            <div className='flex flex-col gap-1'>
              <label className='text-xs text-muted-foreground'>
                {t('audit.eventType', 'Event type')}
              </label>
              <Select value={eventType} onValueChange={setEventType}>
                <SelectTrigger className='h-9 w-52 text-sm'>
                  <SelectValue placeholder={t('audit.allTypes', 'All')} />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value='all'>{t('audit.allTypes', 'All')}</SelectItem>
                  {EVENT_TYPES.map((et) => (
                    <SelectItem key={et} value={et}>
                      {eventTypeLabel(t, et)}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
            <div className='flex flex-col gap-1'>
              <label className='text-xs text-muted-foreground'>
                {t('audit.account', 'Account')}
              </label>
              <VirtualizedSelect
                options={accountOptions}
                value={accountFilter}
                onSelectOption={(values) => setAccountFilter(values[0])}
                placeholder={t('audit.accountPlaceholder', 'Select account')}
                isLoading={isAccountsLoading}
                height='240px'
                className='h-9 w-52 justify-start text-sm font-normal'
                noItemsComponent={
                  <span className='text-xs'>{t('audit.noAccounts', 'No accounts found')}</span>
                }
              />
            </div>
            <div className='flex flex-col gap-1'>
              <label className='text-xs text-muted-foreground'>
                {t('audit.jobId', 'Deletion job')}
              </label>
              <Input
                value={jobFilter}
                onChange={(e) => setJobFilter(e.target.value)}
                placeholder={t('audit.jobIdPlaceholder', 'job id')}
                className='h-9 w-56 text-sm'
              />
            </div>
            <div className='flex flex-col gap-1'>
              <label className='text-xs text-muted-foreground'>
                {t('audit.startDate', 'Start date')}
              </label>
              <DatePicker
                placeholder={t('audit.startDate', 'Start date')}
                selected={startDate}
                onSelect={setStartDate}
                className='w-44'
              />
            </div>
            <div className='flex flex-col gap-1'>
              <label className='text-xs text-muted-foreground'>
                {t('audit.endDate', 'End date')}
              </label>
              <DatePicker
                placeholder={t('audit.endDate', 'End date')}
                selected={endDate}
                onSelect={setEndDate}
                className='w-44'
              />
            </div>
            <div className='ms-auto flex items-end gap-2'>
              <Button className='text-xs' onClick={applyFilters} size="sm">{t('audit.apply', 'Apply')}</Button>
              <Button className="text-xs" variant='outline' size="sm" onClick={resetFilters}>
                {t('audit.reset', 'Reset')}
              </Button>
            </div>
          </div>

          {isLoading ? (
            <TableSkeleton columns={6} rows={10} />
          ) : (
            <>
              <div className='overflow-x-auto rounded-md border'>
                <Table>
                  <TableHeader>
                    <TableRow>
                      <TableHead className='text-xs'>{t('audit.time', 'Time')}</TableHead>
                      <TableHead className='text-xs'>{t('audit.user', 'User')}</TableHead>
                      <TableHead className='text-xs'>{t('audit.eventType', 'Event type')}</TableHead>
                      <TableHead className='text-xs'>{t('audit.detail', 'Detail')}</TableHead>
                      <TableHead className='text-xs'>
                        {t('audit.ip', 'IP')}
                      </TableHead>
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    {data?.items?.map((rec) => {
                      const detail = describeEvent(rec)
                      const hasPayload =
                        !!rec.payload && Object.keys(rec.payload).length > 0
                      const hasDetail = hasPayload || detail !== ''
                      const isOpen = !!expanded[rec.id]
                      return (
                        <Fragment key={rec.id}>
                          <TableRow>
                            <TableCell className='whitespace-nowrap text-xs'>
                              {formatTime(rec.ts_ms)}
                            </TableCell>
                            <TableCell className='text-xs'>{rec.user}</TableCell>
                            <TableCell>
                              <span className='rounded bg-muted px-1.5 py-0.5 text-xs'>
                                {eventTypeLabel(t, rec.event_type)}
                              </span>
                            </TableCell>
                            <TableCell className='max-w-md'>
                              {hasDetail ? (
                                <Button
                                  variant='ghost'
                                  size='sm'
                                  className='h-7 px-1 text-xs font-normal'
                                  aria-expanded={isOpen}
                                  onClick={() => toggleExpanded(rec.id)}
                                >
                                  <ChevronDown
                                    className={cn(
                                      'h-3.5 w-3.5 text-muted-foreground transition-transform',
                                      isOpen && 'rotate-180',
                                    )}
                                  />
                                  {isOpen
                                    ? t('audit.hideDetails', 'Hide details')
                                    : t('audit.showDetails', 'Show details')}
                                </Button>
                              ) : null}
                            </TableCell>
                            <TableCell className='text-xs'>{rec.ip ?? '—'}</TableCell>
                          </TableRow>
                          {isOpen && (
                            <TableRow className='bg-muted/40'>
                              <TableCell colSpan={5} className='p-4'>
                                <div className='space-y-2'>
                                  {detail && (
                                    <div className='text-xs font-medium'>
                                      {detail}
                                    </div>
                                  )}
                                  {hasPayload && (
                                    <pre className='max-h-40 overflow-auto rounded border bg-background p-2 text-xs'>
                                      {JSON.stringify(rec.payload, null, 2)}
                                    </pre>
                                  )}
                                </div>
                              </TableCell>
                            </TableRow>
                          )}
                        </Fragment>
                      )
                    })}
                    {data?.items?.length === 0 && (
                      <TableRow>
                        <TableCell colSpan={5} className='py-8 text-center text-muted-foreground'>
                          {t('audit.empty', 'No audit events found')}
                        </TableCell>
                      </TableRow>
                    )}
                  </TableBody>
                </Table>
              </div>
              {data && data.total > 0 && <div className='mt-2'>
                <TablePagination
                  totalItems={data?.total ?? 0}
                  pageIndex={page - 1}
                  pageSize={pageSize}
                  hasNextPage={() => (data?.total ?? 0) > page * pageSize}
                  setPageIndex={(i) => setPage(i + 1)}
                  setPageSize={handlePageSizeChange}
                />
              </div>}
              {isFetching && (
                <div className='mt-2 text-xs text-muted-foreground'>
                  {t('audit.loading', 'Loading…')}
                </div>
              )}
            </>
          )}
        </div>
      </Main>
    </>
  )
}
