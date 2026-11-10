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
import { useFormContext } from 'react-hook-form'
import { Lock } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import type { AccountModel } from '@/api/account/api'
import { useCurrentUser } from '@/hooks/use-current-user'
import {
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
  FormControl,
  FormDescription,
} from '@/components/ui/form'
import { Input } from '@/components/ui/input'
import { Button } from '@/components/ui/button'
import { AccountFormValues } from './schema'

interface TabRetentionProps {
  account?: AccountModel
}

/** One-click retention presets. Day counts are exact (365-day years, no leap
 *  correction) and stay within the schema's 0–3650 bound (10 years = 3650). */
const PRESETS: { days: number; key: string; fallback: string }[] = [
  { days: 30, key: 'accounts.retentionPreset1m', fallback: '1 month' },
  { days: 90, key: 'accounts.retentionPreset3m', fallback: '3 months' },
  { days: 365, key: 'accounts.retentionPreset1y', fallback: '1 year' },
  { days: 730, key: 'accounts.retentionPreset2y', fallback: '2 years' },
  { days: 1095, key: 'accounts.retentionPreset3y', fallback: '3 years' },
  { days: 1825, key: 'accounts.retentionPreset5y', fallback: '5 years' },
  { days: 3650, key: 'accounts.retentionPreset10y', fallback: '10 years' },
]

/**
 * Retention window (free / community feature). A background sweep purges
 * messages older than this many days from the account. 0 (or empty) keeps
 * everything. While an account is under a legal hold (Enterprise) the sweep is
 * suspended for it, so this setting is inert until the hold is released.
 */
export function TabRetention({ account }: TabRetentionProps) {
  const { t } = useTranslation()
  const { control } = useFormContext<AccountFormValues>()
  const { require_any_permission } = useCurrentUser()
  // Minimum retention is a compliance control: only system:root may set it.
  const canManageLock = require_any_permission(['system:root'])

  const held = !!account?.legal_hold

  return (
    <div className='space-y-6'>
      {held && (
        <div className='flex items-start gap-2 rounded-md border border-amber-500/40 bg-amber-500/10 p-3 text-xs text-amber-600'>
          <Lock className='mt-0.5 h-4 w-4 shrink-0' />
          <span>
            {t(
              'accounts.retentionHeldNotice',
              'This account is under a legal hold. Retention is suspended and no messages are purged until the hold is released.'
            )}
          </span>
        </div>
      )}

      <FormField
        control={control}
        name='retention_days'
        render={({ field }) => (
          <FormItem>
            <FormLabel>
              {t('accounts.retentionDays', 'Retention window (days)')}
            </FormLabel>
            <FormControl>
              <Input
                type='number'
                min={0}
                max={3650}
                disabled={!canManageLock}
                value={field.value ?? ''}
                onChange={(e) => {
                  const raw = e.target.value
                  field.onChange(raw === '' ? undefined : Number(raw))
                }}
                placeholder={t(
                  'accounts.retentionDaysPlaceholder',
                  'Keep everything'
                )}
              />
            </FormControl>
            <div className='flex flex-wrap items-center gap-2'>
              {PRESETS.map((preset) => (
                <Button
                  key={preset.days}
                  type='button'
                  disabled={!canManageLock}
                  variant={field.value === preset.days ? 'default' : 'outline'}
                  size='sm'
                  className='h-7 px-2.5 text-xs'
                  onClick={() => field.onChange(preset.days)}
                >
                  {t(preset.key, preset.fallback)}
                </Button>
              ))}
              <Button
                type='button'
                variant='ghost'
                size='sm'
                disabled={!canManageLock}
                className='h-7 px-2.5 text-xs text-muted-foreground'
                onClick={() => field.onChange(undefined)}
              >
                {t('accounts.retentionDaysPlaceholder', 'Keep everything')}
              </Button>
            </div>
            <FormDescription>
              {t(
                'accounts.retentionDaysDesc',
                'Messages older than this many days are automatically purged. Leave empty (or 0) to keep everything. Age is measured from the older of the message Date header and the server receive time.'
              )}
            </FormDescription>
            <FormMessage />
          </FormItem>
        )}
      />

      <FormField
        control={control}
        name='min_retention_days'
        render={({ field }) => (
          <FormItem>
            <FormLabel>
              {t(
                'accounts.minRetentionDays',
                'Minimum retention / deletion lock (days)'
              )}
            </FormLabel>
            <FormControl>
              <Input
                type='number'
                min={0}
                max={3650}
                disabled={!canManageLock}
                value={field.value ?? ''}
                onChange={(e) => {
                  const raw = e.target.value
                  field.onChange(raw === '' ? undefined : Number(raw))
                }}
                placeholder={t(
                  'accounts.minRetentionDaysPlaceholder',
                  'No deletion lock'
                )}
              />
            </FormControl>
            <div className='flex flex-wrap items-center gap-2'>
              {PRESETS.map((preset) => (
                <Button
                  key={preset.days}
                  type='button'
                  disabled={!canManageLock}
                  variant={field.value === preset.days ? 'default' : 'outline'}
                  size='sm'
                  className='h-7 px-2.5 text-xs'
                  onClick={() => field.onChange(preset.days)}
                >
                  {t(preset.key, preset.fallback)}
                </Button>
              ))}
              <Button
                type='button'
                variant='ghost'
                size='sm'
                disabled={!canManageLock}
                className='h-7 px-2.5 text-xs text-muted-foreground'
                onClick={() => field.onChange(undefined)}
              >
                {t('accounts.minRetentionDaysPlaceholder', 'No deletion lock')}
              </Button>
            </div>
            <FormDescription>
              {t(
                'accounts.minRetentionDaysDesc',
                'Messages younger than this window cannot be deleted by any path, including batch deletion and mailbox or account deletion. The retention sweep also keeps them. Only a system administrator can change this.'
              )}
            </FormDescription>
            <FormMessage />
          </FormItem>
        )}
      />
    </div>
  )
}
