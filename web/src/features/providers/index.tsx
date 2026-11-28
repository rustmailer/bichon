//
// Copyright (c) 2025-2026 rustmailer.com (https://rustmailer.com)
//
// This file is part of the Bichon Email Archiving Project
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as
// published by the Free Software Foundation, either version 3 of the
// License, or (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <http://www.gnu.org/licenses/>.
//
// Cloud provider connections (Pro/Enterprise). A connection is the tenant
// app registration; imported mailboxes become ordinary synced accounts and
// appear in the Accounts page too. This page owns creation, the connection
// test, directory discovery + import, and the per-account runtime actions.
import { useMemo, useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useTranslation } from 'react-i18next'
import {
  AlertCircle,
  CheckCircle2,
  Cloud,
  KeyRound,
  Plus,
  RefreshCw,
  Trash2,
} from 'lucide-react'
import { errorMessage } from '@/lib/error'
import { toast } from '@/hooks/use-toast'
import { useCurrentUser } from '@/hooks/use-current-user'
import { useEdition } from '@/hooks/use-edition'
import {
  list_connections,
  list_presets,
  test_connection,
  type ConnectionTestOutcome,
  type ProviderConnectionView,
} from '@/api/providers/api'
import { FixedHeader } from '@/components/layout/fixed-header'
import { Main } from '@/components/layout/main'
import { Button } from '@/components/ui/button'
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from '@/components/ui/card'
import { Separator } from '@/components/ui/separator'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { cn } from '@/lib/utils'
import { AccountsTab } from './components/accounts'
import {
  CreateConnectionDialog,
  CredentialsDialog,
  DeleteConnectionDialog,
} from './components/connection-dialogs'
import { MailboxesTab } from './components/mailboxes'
import { StatusBadge, useFormatTime } from './status'

export default function ProvidersPage() {
  const { t } = useTranslation()
  const queryClient = useQueryClient()
  const { isPro, isLoading: editionLoading } = useEdition()
  const { require_any_permission } = useCurrentUser()
  const formatTime = useFormatTime()

  const canView =
    isPro && require_any_permission(['system:root', 'user:manage'])

  const [selectedId, setSelectedId] = useState<number | null>(null)
  const [createOpen, setCreateOpen] = useState(false)
  const [credentialsFor, setCredentialsFor] =
    useState<ProviderConnectionView | null>(null)
  const [deleteFor, setDeleteFor] = useState<ProviderConnectionView | null>(
    null
  )
  const [testResult, setTestResult] = useState<
    (ConnectionTestOutcome & { connection_id: number }) | null
  >(null)

  const { data: connections, isLoading } = useQuery({
    queryKey: ['provider-connections'],
    queryFn: list_connections,
    enabled: canView,
    refetchInterval: 15_000,
  })

  const { data: presets } = useQuery({
    queryKey: ['provider-presets'],
    queryFn: list_presets,
    enabled: canView,
    staleTime: 60_000,
  })

  const selected = useMemo(() => {
    const list = connections ?? []
    return list.find((item) => item.id === selectedId) ?? list[0] ?? null
  }, [connections, selectedId])

  const presetLabel = (cloud: string) =>
    presets?.find((preset) => preset.id === cloud)?.display_name ?? cloud

  const test = useMutation({
    mutationFn: (id: number) => test_connection(id),
    onSuccess: (outcome, id) => {
      setTestResult({ ...outcome, connection_id: id })
      queryClient.invalidateQueries({ queryKey: ['provider-connections'] })
      if (outcome.ok) {
        toast({
          title: t('providers.test.ok', 'Connection test succeeded'),
          description: t('providers.test.latency', '{{ms}} ms', {
            ms: outcome.latency_ms,
          }),
        })
      } else {
        toast({
          title: t('providers.test.failed', 'Connection test failed'),
          description: outcome.error?.hint ?? outcome.error?.kind ?? '',
          variant: 'destructive',
        })
      }
    },
    onError: (error) =>
      toast({
        title: t('providers.test.failed', 'Connection test failed'),
        description: errorMessage(error),
        variant: 'destructive',
      }),
  })

  if (editionLoading) {
    return (
      <>
        <FixedHeader />
        <Main>
          <div className='mx-auto w-full max-w-7xl px-4 py-16 text-center text-muted-foreground'>
            {t('common.loading', 'Loading…')}
          </div>
        </Main>
      </>
    )
  }

  if (!canView) {
    return (
      <>
        <FixedHeader />
        <Main>
          <div className='mx-auto w-full max-w-7xl px-4 py-16 text-center text-muted-foreground'>
            {t(
              'providers.forbidden',
              'Cloud provider connections are available in the Pro edition to administrators only.'
            )}
          </div>
        </Main>
      </>
    )
  }

  const list = connections ?? []

  return (
    <>
      <FixedHeader />
      <Main>
        <div className='mx-auto w-full max-w-[100rem] px-4'>
          <div className='mb-2 flex flex-wrap items-center justify-between gap-2'>
            <div>
              <h2 className='text-lg font-bold tracking-tight'>
                {t('providers.title', 'Cloud providers')}
              </h2>
              <p className='text-xs text-muted-foreground'>
                {t(
                  'providers.description',
                  'Archive Microsoft 365 or Google Workspace mailboxes through the provider API. Each imported mailbox is a synced account.'
                )}
              </p>
            </div>
            <Button onClick={() => setCreateOpen(true)}>
              <Plus className='mr-1.5 h-4 w-4' />
              {t('providers.add', 'Add provider')}
            </Button>
          </div>
          <Separator className='my-4' />

          {isLoading ? (
            <div className='py-16 text-center text-sm text-muted-foreground'>
              {t('common.loading', 'Loading…')}
            </div>
          ) : list.length === 0 ? (
            <div className='flex h-72 flex-col items-center justify-center rounded-md border border-dashed text-center'>
              <Cloud className='h-10 w-10 text-muted-foreground/50' />
              <h3 className='mt-3 text-base font-semibold'>
                {t('providers.empty', 'No provider connections yet')}
              </h3>
              <p className='mt-1 max-w-md text-sm text-muted-foreground'>
                {t(
                  'providers.emptyHint',
                  'Register an app in Entra ID (or a Google Workspace service account) and add it here to import mailboxes.'
                )}
              </p>
              <Button className='mt-4' onClick={() => setCreateOpen(true)}>
                <Plus className='mr-1.5 h-4 w-4' />
                {t('providers.add', 'Add provider')}
              </Button>
            </div>
          ) : (
            <div className='grid gap-4 lg:grid-cols-[20rem_1fr]'>
              <div className='space-y-2'>
                {list.map((connection) => (
                  <button
                    key={connection.id}
                    type='button'
                    onClick={() => {
                      setSelectedId(connection.id)
                      setTestResult(null)
                    }}
                    className={cn(
                      'w-full rounded-md border p-3 text-left transition-colors hover:bg-accent',
                      selected?.id === connection.id && 'bg-accent'
                    )}
                  >
                    <div className='flex items-center justify-between gap-2'>
                      <span className='truncate font-medium'>
                        {connection.name}
                      </span>
                      <StatusBadge status={connection.status} />
                    </div>
                    <div className='mt-1 flex items-center justify-between text-xs text-muted-foreground'>
                      <span>{presetLabel(connection.cloud)}</span>
                      <span>
                        {t('providers.accountCount', '{{count}} accounts', {
                          count: connection.account_count,
                        })}
                      </span>
                    </div>
                  </button>
                ))}
              </div>

              {selected && (
                <Card>
                  <CardHeader className='pb-3'>
                    <div className='flex flex-wrap items-start justify-between gap-3'>
                      <div>
                        <CardTitle className='flex items-center gap-2'>
                          {selected.name}
                          <StatusBadge status={selected.status} />
                        </CardTitle>
                        <CardDescription>
                          {t('providers.meta', '{{cloud}} · tenant {{tenant}}', {
                            cloud: presetLabel(selected.cloud),
                            tenant:
                              selected.tenant_id ??
                              selected.admin_email ??
                              '—',
                          })}
                          {' · '}
                          {t('providers.lastOk', 'Last success')}:{' '}
                          {formatTime(selected.last_ok_at)}
                        </CardDescription>
                      </div>
                      <div className='flex flex-wrap gap-2'>
                        <Button
                          variant='outline'
                          size='sm'
                          disabled={test.isPending}
                          onClick={() => test.mutate(selected.id)}
                        >
                          <RefreshCw
                            className={cn(
                              'mr-1.5 h-4 w-4',
                              test.isPending &&
                                test.variables === selected.id &&
                                'animate-spin'
                            )}
                          />
                          {t('providers.test.action', 'Test connection')}
                        </Button>
                        <Button
                          variant='outline'
                          size='sm'
                          onClick={() => setCredentialsFor(selected)}
                        >
                          <KeyRound className='mr-1.5 h-4 w-4' />
                          {t('providers.credentials.action', 'Credentials')}
                        </Button>
                        <Button
                          variant='outline'
                          size='sm'
                          onClick={() => setDeleteFor(selected)}
                        >
                          <Trash2 className='mr-1.5 h-4 w-4' />
                          {t('common.delete', 'Delete')}
                        </Button>
                      </div>
                    </div>
                  </CardHeader>
                  <CardContent>
                    {testResult && testResult.connection_id === selected.id && (
                      <div
                        className={cn(
                          'mb-4 flex items-start gap-2 rounded-md border p-3 text-sm',
                          testResult.ok
                            ? 'border-emerald-500/40 text-emerald-700 dark:text-emerald-400'
                            : 'border-destructive/40 text-destructive'
                        )}
                      >
                        {testResult.ok ? (
                          <CheckCircle2 className='mt-0.5 h-4 w-4 shrink-0' />
                        ) : (
                          <AlertCircle className='mt-0.5 h-4 w-4 shrink-0' />
                        )}
                        <div>
                          <div className='font-medium'>
                            {testResult.ok
                              ? t(
                                  'providers.test.ok',
                                  'Connection test succeeded'
                                )
                              : t(
                                  'providers.test.failed',
                                  'Connection test failed'
                                )}
                          </div>
                          <div className='text-xs'>
                            {testResult.ok
                              ? t(
                                  'providers.test.okDetail',
                                  'Token obtained and directory reachable ({{ms}} ms)',
                                  { ms: testResult.latency_ms }
                                )
                              : (testResult.error?.hint ??
                                testResult.error?.kind ??
                                '')}
                          </div>
                        </div>
                      </div>
                    )}

                    <Tabs defaultValue='overview'>
                      <TabsList>
                        <TabsTrigger value='overview'>
                          {t('providers.tabs.overview', 'Overview')}
                        </TabsTrigger>
                        <TabsTrigger value='mailboxes'>
                          {t('providers.tabs.mailboxes', 'Mailboxes')}
                        </TabsTrigger>
                        <TabsTrigger value='accounts'>
                          {t('providers.tabs.accounts', 'Accounts')}
                        </TabsTrigger>
                      </TabsList>

                      <TabsContent value='overview'>
                        <dl className='grid gap-3 text-sm sm:grid-cols-2'>
                          <div>
                            <dt className='text-muted-foreground'>
                              {t('providers.fields.provider', 'Provider')}
                            </dt>
                            <dd className='font-medium'>
                              {selected.provider === 'Graph'
                                ? 'Microsoft 365 (Graph)'
                                : 'Google Workspace (Gmail)'}
                            </dd>
                          </div>
                          <div>
                            <dt className='text-muted-foreground'>
                              {t('providers.fields.cloud', 'Cloud partition')}
                            </dt>
                            <dd className='font-medium'>
                              {presetLabel(selected.cloud)}
                            </dd>
                          </div>
                          <div>
                            <dt className='text-muted-foreground'>
                              {t('providers.fields.tenant', 'Tenant')}
                            </dt>
                            <dd className='font-mono text-xs'>
                              {selected.tenant_id ?? '—'}
                            </dd>
                          </div>
                          <div>
                            <dt className='text-muted-foreground'>
                              {t(
                                'providers.fields.credentials',
                                'Credentials'
                              )}
                            </dt>
                            <dd>
                              {selected.credential_set
                                ? t(
                                    'providers.fields.credentialsSet',
                                    'Stored (write-only)'
                                  )
                                : t(
                                    'providers.fields.credentialsMissing',
                                    'Missing — update them'
                                  )}
                            </dd>
                          </div>
                          <div>
                            <dt className='text-muted-foreground'>
                              {t('providers.fields.graphBase', 'API endpoint')}
                            </dt>
                            <dd className='font-mono text-xs'>
                              {selected.endpoints?.graph_base ?? '—'}
                            </dd>
                          </div>
                          <div>
                            <dt className='text-muted-foreground'>
                              {t('providers.fields.lastError', 'Last error')}
                            </dt>
                            <dd className='text-xs'>
                              {selected.last_error_kind
                                ? `${selected.last_error_kind} · ${formatTime(
                                    selected.last_error_at
                                  )}`
                                : '—'}
                            </dd>
                          </div>
                        </dl>
                      </TabsContent>

                      <TabsContent value='mailboxes'>
                        <MailboxesTab connection={selected} />
                      </TabsContent>

                      <TabsContent value='accounts'>
                        <AccountsTab connection={selected} />
                      </TabsContent>
                    </Tabs>
                  </CardContent>
                </Card>
              )}
            </div>
          )}
        </div>
      </Main>

      <CreateConnectionDialog
        open={createOpen}
        onOpenChange={setCreateOpen}
        presets={presets ?? []}
        onCreated={(connection) => {
          setSelectedId(connection.id)
          setTestResult(null)
        }}
      />
      <CredentialsDialog
        connection={credentialsFor}
        onOpenChange={(open) => {
          if (!open) {
            setCredentialsFor(null)
            queryClient.invalidateQueries({
              queryKey: ['provider-connections'],
            })
          }
        }}
      />
      <DeleteConnectionDialog
        connection={deleteFor}
        onOpenChange={(open) => {
          if (!open) setDeleteFor(null)
        }}
        onDeleted={(id) => {
          if (selectedId === id) setSelectedId(null)
        }}
      />
    </>
  )
}
