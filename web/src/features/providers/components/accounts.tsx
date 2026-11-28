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
// Imported accounts and their provider runtime state. The status is derived
// server-side (pause, licence limit, connection health all override the last
// pass outcome); this table shows it and offers the three operator actions.
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useTranslation } from 'react-i18next'
import { Activity, Pause, Play } from 'lucide-react'
import { errorMessage } from '@/lib/error'
import { toast } from '@/hooks/use-toast'
import {
  list_account_status,
  pause_account,
  resume_account,
  start_gap_fill,
  type ProviderAccountStatusView,
  type ProviderConnectionView,
} from '@/api/providers/api'
import { Button } from '@/components/ui/button'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import { StatusBadge, useFormatTime } from '../status'

export function AccountsTab({
  connection,
}: {
  connection: ProviderConnectionView
}) {
  const { t } = useTranslation()
  const queryClient = useQueryClient()
  const formatTime = useFormatTime()

  const { data, isLoading } = useQuery<ProviderAccountStatusView[]>({
    queryKey: ['provider-account-status', connection.id],
    queryFn: () => list_account_status(connection.id),
    // A live pass updates progress per folder; poll while one is running.
    refetchInterval: (query) =>
      (query.state.data ?? []).some(
        (row) => row.status === 'syncing' || row.status === 'gap_fill'
      )
        ? 5_000
        : 15_000,
  })

  const invalidate = () => {
    queryClient.invalidateQueries({
      queryKey: ['provider-account-status', connection.id],
    })
    queryClient.invalidateQueries({ queryKey: ['provider-connections'] })
  }

  const pause = useMutation({
    mutationFn: pause_account,
    onSuccess: invalidate,
    onError: (error) =>
      toast({
        title: t('providers.accounts.pauseFailed', 'Could not pause the account'),
        description: errorMessage(error),
        variant: 'destructive',
      }),
  })

  const resume = useMutation({
    mutationFn: resume_account,
    onSuccess: invalidate,
    onError: (error) =>
      toast({
        title: t(
          'providers.accounts.resumeFailed',
          'Could not resume the account'
        ),
        description: errorMessage(error),
        variant: 'destructive',
      }),
  })

  const gapFill = useMutation({
    mutationFn: start_gap_fill,
    onSuccess: () => {
      toast({
        title: t(
          'providers.accounts.gapFillStarted',
          'Gap-fill started in the background'
        ),
      })
      invalidate()
    },
    onError: (error) =>
      toast({
        title: t('providers.accounts.gapFillFailed', 'Could not start gap-fill'),
        description: errorMessage(error),
        variant: 'destructive',
      }),
  })

  if (!isLoading && (data ?? []).length === 0) {
    return (
      <div className='flex h-48 items-center justify-center rounded-md border border-dashed text-sm text-muted-foreground'>
        {t(
          'providers.accounts.empty',
          'No mailboxes imported from this connection yet'
        )}
      </div>
    )
  }

  return (
    <div className='rounded-md border'>
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>{t('accounts.email', 'Email')}</TableHead>
            <TableHead>{t('accounts.status', 'Status')}</TableHead>
            <TableHead>{t('providers.accounts.progress', 'Progress')}</TableHead>
            <TableHead>{t('providers.lastOk', 'Last success')}</TableHead>
            <TableHead>
              {t('providers.accounts.lastPass', 'Last pass')}
            </TableHead>
            <TableHead className='text-right'>
              {t('common.actions', 'Actions')}
            </TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {(data ?? []).map((row) => {
            const busy = row.status === 'syncing' || row.status === 'gap_fill'
            return (
              <TableRow key={row.account_id}>
                <TableCell className='font-medium'>{row.email}</TableCell>
                <TableCell>
                  <StatusBadge status={row.status} />
                  {row.reason && (
                    <div className='mt-1 max-w-[22rem] text-xs text-muted-foreground'>
                      {row.reason_code
                        ? t(
                            `providers.reasons.${row.reason_code}`,
                            row.reason
                          )
                        : row.reason}
                    </div>
                  )}
                </TableCell>
                <TableCell className='text-xs text-muted-foreground'>
                  {busy && row.folders_total != null
                    ? `${row.folders_done}/${row.folders_total}`
                    : row.phase ?? ''}
                </TableCell>
                <TableCell className='text-xs'>
                  {formatTime(row.last_ok_at)}
                </TableCell>
                <TableCell className='text-xs text-muted-foreground'>
                  {t(
                    'providers.accounts.passCounts',
                    '{{archived}} archived · {{failed}} failed · {{suppressed}} suppressed',
                    {
                      archived: row.last_pass_archived,
                      failed: row.last_pass_failed,
                      suppressed: row.last_pass_suppressed,
                    }
                  )}
                </TableCell>
                <TableCell>
                  <div className='flex justify-end gap-1'>
                    {row.paused ? (
                      <Button
                        size='sm'
                        variant='outline'
                        disabled={resume.isPending}
                        onClick={() => resume.mutate(row.account_id)}
                      >
                        <Play className='mr-1 h-3.5 w-3.5' />
                        {t('providers.accounts.resume', 'Resume')}
                      </Button>
                    ) : (
                      <Button
                        size='sm'
                        variant='outline'
                        disabled={pause.isPending}
                        onClick={() => pause.mutate(row.account_id)}
                      >
                        <Pause className='mr-1 h-3.5 w-3.5' />
                        {t('providers.accounts.pause', 'Pause')}
                      </Button>
                    )}
                    <Button
                      size='sm'
                      variant='outline'
                      disabled={row.paused || busy || gapFill.isPending}
                      title={
                        row.paused
                          ? t(
                              'providers.accounts.gapFillPaused',
                              'Resume the account before running gap-fill'
                            )
                          : undefined
                      }
                      onClick={() => gapFill.mutate(row.account_id)}
                    >
                      <Activity className='mr-1 h-3.5 w-3.5' />
                      {t('providers.accounts.gapFill', 'Gap-fill')}
                    </Button>
                  </div>
                </TableCell>
              </TableRow>
            )
          })}
        </TableBody>
      </Table>
    </div>
  )
}
