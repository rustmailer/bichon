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
import { Loader2 } from 'lucide-react'
import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import {
  getComplianceConfig,
  updateComplianceConfig,
  type ComplianceConfigView,
} from '@/api/compliance/api'
import { list_accounts } from '@/api/account/api'
import { useQuery } from '@tanstack/react-query'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { useEdition } from '@/hooks/use-edition'
import { useCurrentUser } from '@/hooks/use-current-user'
import { useToast } from '@/hooks/use-toast'
import { FixedHeader } from '@/components/layout/fixed-header'
import { Main } from '@/components/layout/main'

/** Pro/Enterprise compliance configuration. Each field is an override on top
 *  of the env/CLI default: leave it empty to keep the default, type a value
 *  to override, and clear it (empty again) to return to the default. */
export default function ComplianceConfigPage() {
  const { t } = useTranslation()
  const { toast } = useToast()
  const { isPro, isEnterprise } = useEdition()
  const { require_any_permission } = useCurrentUser()

  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)
  const [current, setCurrent] = useState<ComplianceConfigView | null>(null)

  const [auditRetentionDays, setAuditRetentionDays] = useState('')
  const [integrityScheduleHours, setIntegrityScheduleHours] = useState('')
  const [integrityRetentionDays, setIntegrityRetentionDays] = useState('')
  const [tspIntervalHours, setTspIntervalHours] = useState('')

  const canManage =
    (isPro || isEnterprise) && require_any_permission(['system:root'])

  // Read only to answer "how long is mail kept?" for the warning below. The
  // page works without it — a failed read just falls back to the generic
  // GoBD tripwire.
  const { data: accounts } = useQuery({
    queryKey: ['account-list'],
    queryFn: list_accounts,
    enabled: canManage,
  })

  useEffect(() => {
    let mounted = true
    getComplianceConfig()
      .then((cfg) => {
        if (!mounted) return
        setCurrent(cfg)
        setAuditRetentionDays(cfg.audit_retention_days?.toString() ?? '')
        // The schedule interval has no env/CLI default any more: the server
        // falls back to 24h when nothing is stored, so the field shows that
        // number instead of an empty box with an invisible default.
        setIntegrityScheduleHours(
          cfg.integrity_schedule_hours?.toString() ?? '24'
        )
        setIntegrityRetentionDays(
          cfg.integrity_retention_days?.toString() ?? ''
        )
        setTspIntervalHours(cfg.tsp_interval_hours?.toString() ?? '')
      })
      .catch(() => {
        if (mounted) {
          toast({
            variant: 'destructive',
            title: t(
              'settings.compliance.loadFailed',
              'Failed to load compliance configuration',
            ),
          })
        }
      })
      .finally(() => {
        if (mounted) setLoading(false)
      })
    return () => {
      mounted = false
    }
  }, [t, toast])

  if (!canManage) {
    return (
      <>
        <FixedHeader />
        <Main>
          <div className='mx-auto w-full max-w-7xl px-4 py-16 text-center text-muted-foreground'>
            {t(
              'settings.compliance.forbidden',
              'Compliance configuration requires the Pro or Enterprise edition and the system:root permission.',
            )}
          </div>
        </Main>
      </>
    )
  }

  if (loading) {
    return (
      <>
        <FixedHeader />
        <Main>
          <div className='flex h-64 items-center justify-center'>
            <Loader2 className='h-6 w-6 animate-spin' />
          </div>
        </Main>
      </>
    )
  }

  const numOrEmpty = (value: string) => (value.trim() ? value.trim() : '')

  const handleSave = async () => {
    setSaving(true)
    try {
      // Always send all four fields: a filled input sets the override, an
      // empty one clears it back to the env/CLI default.
      const payload = {
        audit_retention_days: numOrEmpty(auditRetentionDays),
        integrity_schedule_hours: numOrEmpty(integrityScheduleHours),
        integrity_retention_days: numOrEmpty(integrityRetentionDays),
        tsp_interval_hours: numOrEmpty(tspIntervalHours),
      }
      const saved = await updateComplianceConfig(payload)
      setCurrent(saved)
      toast({
        title: t('settings.compliance.saved', 'Compliance configuration saved'),
      })
    } catch (err: unknown) {
      const message =
        (err as { response?: { data?: { message?: string } } })?.response?.data
          ?.message || (err instanceof Error ? err.message : String(err))
      toast({
        variant: 'destructive',
        title: t(
          'settings.compliance.saveFailed',
          'Failed to save compliance configuration',
        ),
        description: message,
      })
    } finally {
      setSaving(false)
    }
  }

  /** The window actually in force: an empty box means "keep the server
   *  default", so the effective value is the stored one (0 when nothing has
   *  ever been set, which now means "keep indefinitely"). */
  const auditRetentionEffective = (() => {
    const typed = auditRetentionDays.trim()
    if (typed !== '') return Number(typed)
    return current?.audit_retention_days ?? 0
  })()

  /** How long mail is kept, across all accounts. This is the number the audit
   *  window has to cover: the trail is the evidence that the archive was never
   *  tampered with, and evidence that is deleted before the records it attests
   *  to expire proves nothing. `0` on an account means it keeps mail
   *  indefinitely, which is the binding case rather than the maximum. */
  const accountRetention = (accounts?.items ?? []).reduce(
    (acc, a) => ({
      unlimited: acc.unlimited || (a.retention_days ?? 0) === 0,
      max: Math.max(acc.max, a.retention_days ?? 0),
    }),
    { unlimited: false, max: 0 }
  )

  /** The warning under the field, in descending order of specificity. The
   *  server refuses nothing here — a short window is a legitimate choice — but
   *  it warns at startup, and the operator deserves to see the same warning
   *  where the choice is made rather than in a log file nobody reads. */
  const auditRetentionWarning = (() => {
    if (auditRetentionEffective <= 0) return null
    if (accountRetention.unlimited) {
      return t(
        'settings.compliance.auditRetentionVsAccountUnlimited',
        'This purges the audit trail while at least one account is configured to keep mail indefinitely. The trail will be gone long before the records it attests to. Set 0 to keep it.'
      )
    }
    if (accountRetention.max > auditRetentionEffective) {
      return t(
        'settings.compliance.auditRetentionVsAccount',
        'This purges the audit trail after {{audit}} day(s), while mail is kept for up to {{account}} day(s). The trail will be gone before the records it attests to — set 0, or at least match the account retention.',
        {
          audit: auditRetentionEffective,
          account: accountRetention.max,
        }
      )
    }
    // Same threshold as `MIN_RECOMMENDED_AUDIT_RETENTION_DAYS` on the server:
    // below a year is clearly indefensible, above it is a deliberate window.
    if (auditRetentionEffective < 365) {
      return t(
        'settings.compliance.auditRetentionDaysWarning',
        'This purges the evidence that the archive was never tampered with, long before the records it attests to expire. GoBD and comparable regimes require 5–8 years. Set 0 to keep the audit trail indefinitely.'
      )
    }
    return null
  })()

  const numField = (
    id: string,
    value: string,
    onChange: (v: string) => void,
    key: string,
    fallback: string,
    hintKey: string,
    warning?: string | null
  ) => (
    <div className='space-y-2'>
      <Label htmlFor={id}>{t(`settings.compliance.${key}`, key)}</Label>
      <Input
        id={id}
        type='number'
        min={0}
        value={value}
        placeholder={fallback}
        onChange={(e) => onChange(e.target.value)}
      />
      <p className='text-xs text-muted-foreground'>
        {t(`settings.compliance.${hintKey}`, '')}
      </p>
      {warning && (
        <p className='text-xs text-amber-600 dark:text-amber-400'>{warning}</p>
      )}
    </div>
  )

  return (
    <>
      <FixedHeader />
      <Main>
        <div className='mx-auto w-full max-w-7xl space-y-6 px-4'>
          <div className='space-y-2'>
            <h1 className='text-lg font-semibold'>
              {t('settings.compliance.title', 'Compliance configuration')}
            </h1>
            <p className='text-xs text-muted-foreground'>
              {t(
                'settings.compliance.description',
                'Retention windows and schedule intervals for the audit trail, integrity checks and timestamp anchoring. Empty fields keep the server defaults; type a value to override.',
              )}
            </p>
          </div>

          <div className='space-y-6 rounded-lg border p-6'>
            <div className='grid gap-4 sm:grid-cols-2'>
              {numField(
                'compliance-audit-retention',
                auditRetentionDays,
                setAuditRetentionDays,
                'auditRetentionDays',
                current?.audit_retention_days?.toString() ?? '',
                'auditRetentionDaysHint',
                auditRetentionWarning,
              )}
              {numField(
                'compliance-integrity-schedule',
                integrityScheduleHours,
                setIntegrityScheduleHours,
                'integrityScheduleHours',
                current?.integrity_schedule_hours?.toString() ?? '24',
                'integrityScheduleHoursHint',
              )}
              {numField(
                'compliance-integrity-retention',
                integrityRetentionDays,
                setIntegrityRetentionDays,
                'integrityRetentionDays',
                current?.integrity_retention_days?.toString() ?? '',
                'integrityRetentionDaysHint',
              )}
              {numField(
                'compliance-tsp-interval',
                tspIntervalHours,
                setTspIntervalHours,
                'tspIntervalHours',
                current?.tsp_interval_hours?.toString() ?? '',
                'tspIntervalHoursHint',
              )}
            </div>

            <div className='flex items-center justify-end'>
              <Button type='button' onClick={handleSave} disabled={saving}>
                {saving && <Loader2 className='mr-2 h-4 w-4 animate-spin' />}
                {t('settings.compliance.save', 'Save configuration')}
              </Button>
            </div>
          </div>
        </div>
      </Main>
    </>
  )
}
