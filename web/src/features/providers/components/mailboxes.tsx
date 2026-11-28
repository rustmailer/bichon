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
// Directory discovery and mailbox import. The listing is a cache of
// suggestions; importing creates synced accounts from the selected rows and
// reports every address back (created / already archived / not importable).
import { useMemo, useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useTranslation } from 'react-i18next'
import { RefreshCw, Search, Upload } from 'lucide-react'
import { errorMessage } from '@/lib/error'
import { toast } from '@/hooks/use-toast'
import {
  discover_mailboxes,
  get_candidates,
  import_accounts,
  type DiscoveryCandidate,
  type DiscoveryView,
  type ImportOutcome,
  type ProviderConnectionView,
} from '@/api/providers/api'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from '@/components/ui/card'
import { Checkbox } from '@/components/ui/checkbox'
import { Input } from '@/components/ui/input'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import { useFormatTime } from '../status'

function selectable(candidate: DiscoveryCandidate): boolean {
  return candidate.archivable && !candidate.already_imported
}

export function MailboxesTab({
  connection,
}: {
  connection: ProviderConnectionView
}) {
  const { t } = useTranslation()
  const queryClient = useQueryClient()
  const formatTime = useFormatTime()
  const [filter, setFilter] = useState('')
  const [selected, setSelected] = useState<string[]>([])
  const [result, setResult] = useState<ImportOutcome | null>(null)

  const { data: discovery, isLoading } = useQuery<DiscoveryView | undefined>({
    queryKey: ['provider-candidates', connection.id],
    queryFn: () => get_candidates(connection.id),
  })

  const discover = useMutation({
    mutationFn: () => discover_mailboxes(connection.id),
    onSuccess: (view) => {
      setSelected([])
      setResult(null)
      queryClient.setQueryData(['provider-candidates', connection.id], view)
    },
    onError: (error) => {
      toast({
        title: t('providers.mailboxes.discoverFailed', 'Discovery failed'),
        description: errorMessage(error),
        variant: 'destructive',
      })
    },
  })

  const importSelected = useMutation({
    mutationFn: () => import_accounts(connection.id, selected),
    onSuccess: (outcome) => {
      setResult(outcome)
      setSelected([])
      queryClient.invalidateQueries({
        queryKey: ['provider-candidates', connection.id],
      })
      queryClient.invalidateQueries({ queryKey: ['provider-connections'] })
      queryClient.invalidateQueries({
        queryKey: ['provider-account-status', connection.id],
      })
      toast({
        title: t('providers.mailboxes.importDone', 'Import finished'),
        description: t(
          'providers.mailboxes.importSummary',
          '{{created}} created, {{skipped}} skipped, {{failed}} failed',
          {
            created: outcome.created,
            skipped: outcome.skipped,
            failed: outcome.failed,
          }
        ),
      })
    },
    onError: (error) => {
      toast({
        title: t('providers.mailboxes.importFailed', 'Import failed'),
        description: errorMessage(error),
        variant: 'destructive',
      })
    },
  })

  const candidates = discovery?.candidates ?? []
  const visible = useMemo(() => {
    const needle = filter.trim().toLowerCase()
    if (!needle) return candidates
    return candidates.filter(
      (candidate) =>
        candidate.email.toLowerCase().includes(needle) ||
        (candidate.display_name ?? '').toLowerCase().includes(needle)
    )
  }, [candidates, filter])

  const visibleSelectable = visible.filter(selectable)
  const allVisibleSelected =
    visibleSelectable.length > 0 &&
    visibleSelectable.every((candidate) => selected.includes(candidate.email))

  const toggle = (email: string) =>
    setSelected((current) =>
      current.includes(email)
        ? current.filter((item) => item !== email)
        : [...current, email]
    )

  const toggleAll = () => {
    const emails = visibleSelectable.map((candidate) => candidate.email)
    setSelected((current) =>
      allVisibleSelected
        ? current.filter((email) => !emails.includes(email))
        : Array.from(new Set([...current, ...emails]))
    )
  }

  if (!discovery && !isLoading) {
    return (
      <div className='flex h-64 flex-col items-center justify-center rounded-md border border-dashed text-center'>
        <p className='text-sm font-medium'>
          {t('providers.mailboxes.needDiscover', 'No directory listing yet')}
        </p>
        <p className='mt-1 max-w-md text-sm text-muted-foreground'>
          {t(
            'providers.mailboxes.needDiscoverHint',
            'Discovery reads the tenant directory so you can pick the mailboxes to archive. Nothing is created until you import.'
          )}
        </p>
        <Button
          className='mt-4'
          onClick={() => discover.mutate()}
          disabled={discover.isPending}
        >
          <Search className='mr-1.5 h-4 w-4' />
          {discover.isPending
            ? t('providers.mailboxes.discovering', 'Discovering…')
            : t('providers.mailboxes.discover', 'Discover mailboxes')}
        </Button>
      </div>
    )
  }

  return (
    <div className='space-y-4'>
      <div className='flex flex-wrap items-center justify-between gap-2'>
        <div className='text-sm text-muted-foreground'>
          {t('providers.mailboxes.listed', '{{total}} addresses listed', {
            total: discovery?.total_seen ?? 0,
          })}
          {' · '}
          {t('providers.mailboxes.fetchedAt', 'fetched {{time}}', {
            time: formatTime(discovery?.fetched_at),
          })}
          {discovery?.stale && (
            <Badge variant='outline' className='ml-2'>
              {t('providers.mailboxes.stale', 'Listing is old')}
            </Badge>
          )}
        </div>
        <Button
          variant='outline'
          size='sm'
          onClick={() => discover.mutate()}
          disabled={discover.isPending || importSelected.isPending}
        >
          <RefreshCw
            className={`mr-1.5 h-4 w-4 ${discover.isPending ? 'animate-spin' : ''}`}
          />
          {t('providers.mailboxes.refresh', 'Refresh listing')}
        </Button>
      </div>

      <div className='flex flex-wrap items-center gap-2'>
        <div className='relative'>
          <Search className='absolute left-2 top-2.5 h-4 w-4 text-muted-foreground' />
          <Input
            className='w-64 pl-8'
            placeholder={t('providers.mailboxes.search', 'Filter by address')}
            value={filter}
            onChange={(event) => setFilter(event.target.value)}
          />
        </div>
        <Button
          variant='outline'
          size='sm'
          onClick={toggleAll}
          disabled={visibleSelectable.length === 0}
        >
          {allVisibleSelected
            ? t('common.deselectAll', 'Deselect all')
            : t('common.selectAll', 'Select all')}
        </Button>
        <span className='text-sm text-muted-foreground'>
          {t('providers.mailboxes.selected', '{{count}} selected', {
            count: selected.length,
          })}
        </span>
        <Button
          className='ml-auto'
          disabled={selected.length === 0 || importSelected.isPending}
          onClick={() => importSelected.mutate()}
        >
          <Upload className='mr-1.5 h-4 w-4' />
          {importSelected.isPending
            ? t('providers.mailboxes.importing', 'Importing…')
            : t('providers.mailboxes.import', 'Import selected')}
        </Button>
      </div>

      <div className='rounded-md border'>
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead className='w-10' />
              <TableHead>{t('accounts.email', 'Email')}</TableHead>
              <TableHead>{t('accounts.type', 'Type')}</TableHead>
              <TableHead>{t('accounts.status', 'Status')}</TableHead>
              <TableHead>{t('providers.mailboxes.note', 'Note')}</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {visible.length === 0 ? (
              <TableRow>
                <TableCell
                  colSpan={5}
                  className='h-24 text-center text-sm text-muted-foreground'
                >
                  {t('providers.mailboxes.empty', 'No addresses to show')}
                </TableCell>
              </TableRow>
            ) : (
              visible.map((candidate) => (
                <TableRow key={candidate.email}>
                  <TableCell>
                    <Checkbox
                      checked={selected.includes(candidate.email)}
                      disabled={!selectable(candidate)}
                      onCheckedChange={() => toggle(candidate.email)}
                    />
                  </TableCell>
                  <TableCell>
                    <div className='font-medium'>{candidate.email}</div>
                    {candidate.display_name && (
                      <div className='text-xs text-muted-foreground'>
                        {candidate.display_name}
                      </div>
                    )}
                  </TableCell>
                  <TableCell className='text-sm'>
                    {candidate.mailbox_type}
                  </TableCell>
                  <TableCell>
                    <div className='flex flex-wrap gap-1'>
                      {candidate.already_imported && (
                        <Badge variant='secondary'>
                          {t(
                            'providers.mailboxes.alreadyImported',
                            'Already archived'
                          )}
                        </Badge>
                      )}
                      {!candidate.archivable && (
                        <Badge variant='destructive'>
                          {t(
                            'providers.mailboxes.unarchivable',
                            'Not importable'
                          )}
                        </Badge>
                      )}
                      {candidate.suspended && (
                        <Badge variant='outline'>
                          {t('providers.mailboxes.suspended', 'Suspended')}
                        </Badge>
                      )}
                      {candidate.archived && (
                        <Badge variant='outline'>
                          {t('providers.mailboxes.archived', 'Archived user')}
                        </Badge>
                      )}
                    </div>
                  </TableCell>
                  <TableCell className='max-w-[16rem] truncate text-xs text-muted-foreground'>
                    {candidate.reason ?? ''}
                  </TableCell>
                </TableRow>
              ))
            )}
          </TableBody>
        </Table>
      </div>

      {result && (
        <Card>
          <CardHeader className='pb-2'>
            <CardTitle className='text-base'>
              {t('providers.mailboxes.results', 'Import result')}
            </CardTitle>
            <CardDescription>
              {t(
                'providers.mailboxes.importSummary',
                '{{created}} created, {{skipped}} skipped, {{failed}} failed',
                {
                  created: result.created,
                  skipped: result.skipped,
                  failed: result.failed,
                }
              )}
            </CardDescription>
          </CardHeader>
          <CardContent className='space-y-1'>
            {result.results.map((item) => (
              <div
                key={item.email}
                className='flex items-start justify-between gap-3 border-b py-1 text-sm last:border-b-0'
              >
                <span className='font-mono text-xs'>{item.email}</span>
                <span className='text-right'>
                  <Badge
                    variant={
                      item.status === 'created'
                        ? 'secondary'
                        : item.status === 'failed' ||
                            item.status === 'license_refused'
                          ? 'destructive'
                          : 'outline'
                    }
                  >
                    {t(
                      `providers.importStatus.${item.status}`,
                      item.status
                    )}
                  </Badge>
                  {item.detail && (
                    <span className='ml-2 text-xs text-muted-foreground'>
                      {item.detail}
                    </span>
                  )}
                </span>
              </div>
            ))}
          </CardContent>
        </Card>
      )}
    </div>
  )
}
