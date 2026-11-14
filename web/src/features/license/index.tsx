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

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { AxiosError } from 'axios'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Copy, FileUp, Loader2 } from 'lucide-react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from '@/components/ui/card'
import { Label } from '@/components/ui/label'
import { Textarea } from '@/components/ui/textarea'
import { FixedHeader } from '@/components/layout/fixed-header'
import { Main } from '@/components/layout/main'
import { useEdition } from '@/hooks/use-edition'
import { useCurrentUser } from '@/hooks/use-current-user'
import { useToast } from '@/hooks/use-toast'
import { copyToClipboard } from '@/lib/clipboard'
import {
  get_license_status,
  upload_license,
  type LicenseStatusResponse,
} from '@/api/license/api'

function formatEpoch(ts?: string | null): string {
  if (!ts) return '—'
  const n = Number(ts)
  if (!Number.isFinite(n)) return ts
  return new Date(n * 1000).toLocaleDateString()
}

function formatEdition(edition?: string | null): string {
  if (!edition) return ''
  return edition.charAt(0).toUpperCase() + edition.slice(1)
}

function InfoRow({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className='flex items-start justify-between gap-4 py-2'>
      <span className='text-sm text-muted-foreground'>{label}</span>
      <div className='flex flex-wrap items-center justify-end gap-1.5 text-right text-sm'>
        {children}
      </div>
    </div>
  )
}

export default function LicensePage() {
  const { t } = useTranslation()
  const { toast } = useToast()
  const queryClient = useQueryClient()
  const { isPro } = useEdition()
  const { require_any_permission } = useCurrentUser()
  const [licenseText, setLicenseText] = useState('')

  const { data, isLoading, error } = useQuery<LicenseStatusResponse, AxiosError>({
    queryKey: ['license-status'],
    queryFn: get_license_status,
    retry: false,
  })

  const upload = useMutation({
    mutationFn: upload_license,
    onSuccess: () => {
      toast({ title: t('license.uploadSuccess') })
      setLicenseText('')
      queryClient.invalidateQueries({ queryKey: ['license-status'] })
      // The sidebar and every edition gate read `/features` (cached for the
      // life of the page). Without this, Enterprise items — LDAP, SIEM,
      // compliance — stay hidden until a full reload after an upgrade.
      queryClient.invalidateQueries({ queryKey: ['edition'] })
    },
    onError: (err: AxiosError<{ error?: string }>) => {
      toast({
        title: t('license.uploadFailed'),
        description: err.response?.data?.error ?? t('license.uploadFailedDesc'),
        variant: 'destructive',
      })
    },
  })

  const copyMachineId = async () => {
    if (!data?.machine_id) return
    try {
      await copyToClipboard(data.machine_id)
      toast({ title: t('license.copied') })
    } catch {
      toast({ title: t('license.copyFailed'), variant: 'destructive' })
    }
  }

  const onFilePicked = (e: React.ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0]
    if (!file) return
    const reader = new FileReader()
    reader.onload = () => setLicenseText(String(reader.result ?? '').trim())
    reader.onerror = () => toast({ title: t('license.readFileFailed'), variant: 'destructive' })
    reader.readAsText(file)
    e.target.value = ''
  }

  const canView = isPro && require_any_permission(['system:root', 'user:manage'])

  if (!canView) {
    return (
      <>
        <FixedHeader />
        <Main>
          <div className='mx-auto w-full max-w-7xl px-4 py-16 text-center text-muted-foreground'>
            {t('license.forbidden', 'License management is available in the Pro edition only.')}
          </div>
        </Main>
      </>
    )
  }

  const statusLabels: Record<string, string> = {
    valid: t('license.statusValid'),
    trial: t('license.statusTrial'),
    trial_expired: t('license.statusTrialExpired'),
    update_expired: t('license.statusUpdateExpired'),
    machine_mismatch: t('license.statusMachineMismatch'),
    invalid_signature: t('license.statusInvalid'),
    error: t('license.statusError'),
  }

  const statusVariant: Record<string, 'default' | 'secondary' | 'destructive' | 'outline'> = {
    valid: 'default',
    trial: 'secondary',
  }

  const status = data?.status ?? ''

  return (
    <>
      <FixedHeader />
      <Main>
        <div className='mx-auto w-full max-w-7xl px-4'>
          <h1 className='mb-1 text-lg font-semibold'>{t('license.title')}</h1>
          <p className='mb-6 text-xs text-muted-foreground'>{t('license.description')}</p>

          {isLoading && (
            <div className='flex h-40 items-center justify-center'>
              <Loader2 className='h-6 w-6 animate-spin' />
            </div>
          )}

          {error && !isLoading && (
            <div className='mb-6 rounded-md border border-destructive/50 p-4 text-sm text-destructive'>
              {t('license.loadFailed')}
            </div>
          )}

          {data && (
            <div className='grid gap-6 lg:grid-cols-2'>
              <Card>
                <CardHeader>
                  <CardTitle>{t('license.statusTitle')}</CardTitle>
                  <CardDescription>{t('license.statusDesc')}</CardDescription>
                </CardHeader>
                <CardContent className='divide-y'>
                  <InfoRow label={t('license.status')}>
                    <Badge variant={statusVariant[status] ?? 'destructive'}>
                      {statusLabels[status] ?? status}
                    </Badge>
                  </InfoRow>
                  <InfoRow label={t('license.edition')}>
                    {data.edition ? (
                      <Badge variant='outline'>{formatEdition(data.edition)}</Badge>
                    ) : (
                      t('license.notAvailable')
                    )}
                  </InfoRow>
                  <InfoRow label={t('license.licensee')}>
                    {data.email ?? t('license.notAvailable')}
                  </InfoRow>
                  <InfoRow label={t('license.updatesUntil')}>
                    {formatEpoch(data.updates_until)}
                  </InfoRow>
                  {data.days_remaining !== null && data.days_remaining !== undefined && (
                    <InfoRow label={t('license.trialDays')}>
                      {t('license.trialDaysRemaining', { days: data.days_remaining })}
                    </InfoRow>
                  )}
                  <InfoRow label={t('license.accounts')}>
                    {data.account_limit
                      ? t('license.accountsUsed', {
                          used: data.accounts_used,
                          limit: data.account_limit,
                        })
                      : t('license.notAvailable')}
                  </InfoRow>
                </CardContent>
              </Card>

              <div className='flex flex-col gap-6'>
                <Card>
                  <CardHeader>
                    <CardTitle>{t('license.machineIdTitle')}</CardTitle>
                    <CardDescription>{t('license.machineIdDesc')}</CardDescription>
                  </CardHeader>
                  <CardContent>
                    <div className='flex items-center gap-2'>
                      <code className='min-w-0 flex-1 break-all rounded-md bg-muted px-3 py-2 font-mono text-xs'>
                        {data.machine_id || t('license.notAvailable')}
                      </code>
                      <Button
                        variant='outline'
                        size='icon'
                        onClick={copyMachineId}
                        title={t('license.copyMachineId')}
                        disabled={!data.machine_id}
                      >
                        <Copy className='h-4 w-4' />
                      </Button>
                    </div>
                  </CardContent>
                </Card>

                <Card>
                  <CardHeader>
                    <CardTitle>{t('license.uploadTitle')}</CardTitle>
                    <CardDescription>{t('license.uploadDesc')}</CardDescription>
                  </CardHeader>
                  <CardContent className='flex flex-col gap-3'>
                    <div className='flex items-center gap-2'>
                      <Label
                        htmlFor='license-file'
                        className='inline-flex h-9 cursor-pointer items-center gap-2 rounded-md border border-input bg-background px-3 text-sm font-medium shadow-sm transition-colors hover:bg-accent'
                      >
                        <FileUp className='h-4 w-4' />
                        {t('license.chooseFile')}
                        <input
                          id='license-file'
                          type='file'
                          accept='.jwt,.txt,text/plain,application/json'
                          className='hidden'
                          onChange={onFilePicked}
                        />
                      </Label>
                    </div>
                    <Textarea
                      value={licenseText}
                      onChange={(e) => setLicenseText(e.target.value)}
                      placeholder={t('license.pasteHere')}
                      rows={5}
                      className='font-mono text-xs'
                    />
                    <Button
                      onClick={() => upload.mutate(licenseText.trim())}
                      disabled={!licenseText.trim() || upload.isPending}
                      className='self-start'
                    >
                      {upload.isPending && <Loader2 className='mr-2 h-4 w-4 animate-spin' />}
                      {upload.isPending ? t('license.uploading') : t('license.upload')}
                    </Button>
                  </CardContent>
                </Card>
              </div>
            </div>
          )}
        </div>
      </Main>
    </>
  )
}
