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
import { Loader2 } from 'lucide-react'
import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import {
  getJournalConfig,
  updateJournalConfig,
} from '@/api/journal/api'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Switch } from '@/components/ui/switch'
import { Textarea } from '@/components/ui/textarea'
import { useEdition } from '@/hooks/use-edition'
import { useCurrentUser } from '@/hooks/use-current-user'
import { useToast } from '@/hooks/use-toast'

/** The archive inlet: accept journal deliveries from the tenant's mail system.
 *
 *  A child of the `/settings` layout, so it renders neither `FixedHeader` nor
 *  `Main` — those come from `@/features/settings`. */
export function JournalSettings() {
  const { t } = useTranslation()
  const { toast } = useToast()
  const { isPro, isEnterprise } = useEdition()
  const { canGlobal } = useCurrentUser()

  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)

  const [enabled, setEnabled] = useState(false)
  const [target, setTarget] = useState('')
  // The CIDR list is kept as free text (one entry per line) so a typo is
  // visible before saving rather than silently dropped.
  const [cidrs, setCidrs] = useState('')

  const canManage =
    (isPro || isEnterprise) && canGlobal('system:root')

  useEffect(() => {
    let mounted = true
    getJournalConfig()
      .then((cfg) => {
        if (!mounted) return
        setEnabled(cfg.enabled)
        setTarget(cfg.target_address)
        setCidrs(cfg.allowed_cidrs.join('\n'))
      })
      .catch(() => {
        if (mounted) {
          toast({
            variant: 'destructive',
            title: t(
              'settings.journal.loadFailed',
              'Failed to load the SMTP journaling configuration',
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

  const handleSave = async () => {
    setSaving(true)
    try {
      const saved = await updateJournalConfig({
        enabled,
        target_address: target.trim(),
        // One per line in the box; the server trims and rejects anything that
        // is not a CIDR, so an empty list is sent as an empty array and the
        // server refuses only if the feature is being enabled.
        allowed_cidrs: cidrs
          .split('\n')
          .map((line) => line.trim())
          .filter((line) => line.length > 0),
      })
      setEnabled(saved.enabled)
      setTarget(saved.target_address)
      setCidrs(saved.allowed_cidrs.join('\n'))
      toast({
        title: t(
          'settings.journal.saved',
          'SMTP journaling configuration saved',
        ),
      })
    } catch (err) {
      const message =
        (err as { response?: { data?: { message?: string } } })?.response?.data
          ?.message || (err instanceof Error ? err.message : String(err))
      toast({
        variant: 'destructive',
        title: t(
          'settings.journal.saveFailed',
          'Failed to save the SMTP journaling configuration',
        ),
        description: message,
      })
    } finally {
      setSaving(false)
    }
  }

  if (!canManage) {
    return (
      <div className='w-full p-6 text-muted-foreground'>
        {t(
          'settings.journal.forbidden',
          'SMTP journaling requires the Pro or Enterprise edition and the system:root permission.',
        )}
      </div>
    )
  }

  if (loading) {
    return (
      <div className='flex h-64 items-center justify-center'>
        <Loader2 className='h-6 w-6 animate-spin' />
      </div>
    )
  }

  return (
    <div className='w-full max-w-7xl space-y-6 px-4'>
      <div className='space-y-2'>
        <h2 className='text-xl font-bold'>
          {t('settings.journal.title', 'SMTP journaling')}
        </h2>
        <p className='text-sm text-muted-foreground'>
          {t(
            'settings.journal.description',
            'Accept the copy of every message your mail system delivers to a journal address — before it reaches the recipient. This is the only way to archive a message an employee receives and deletes between two syncs. Deliveries arrive without SMTP authentication, so only the source network is accepted, and only over TLS.',
          )}
        </p>
      </div>

      <div className='space-y-6 rounded-lg border p-6'>
        <div className='flex items-center justify-between'>
          <div className='space-y-1'>
            <Label>{t('settings.journal.enabled', 'Journaling enabled')}</Label>
            <p className='text-xs text-muted-foreground'>
              {t(
                'settings.journal.enabledHint',
                'When off, the SMTP server behaves exactly as it did before: no peer is treated as a journal source.',
              )}
            </p>
          </div>
          <Switch checked={enabled} onCheckedChange={setEnabled} />
        </div>

        <div className='grid gap-4 sm:grid-cols-2'>
          <div className='space-y-2'>
            <Label htmlFor='journal-target'>
              {t('settings.journal.target', 'Journal target address')}
            </Label>
            <Input
              id='journal-target'
              value={target}
              onChange={(e) => setTarget(e.target.value)}
              placeholder='journal@firma.de'
            />
            <p className='text-xs text-muted-foreground'>
              {t(
                'settings.journal.targetHint',
                'The address your journaling rule delivers to. Saving this configuration creates the archive account for it (administrator permission required) — the address does not need to exist as a mailbox.',
              )}
            </p>
            <p className='text-xs text-muted-foreground'>
              {t(
                'settings.journal.changeHint',
                'Changing this address later starts a new journal account on the first delivery; the previous account keeps its mail. Nothing is moved or merged, and the old account must not be deleted — it is part of the archive.',
              )}
            </p>
          </div>

          <div className='space-y-2'>
            <Label htmlFor='journal-cidrs'>
              {t('settings.journal.cidrs', 'Allowed source networks (CIDR)')}
            </Label>
            <Textarea
              id='journal-cidrs'
              value={cidrs}
              onChange={(e) => setCidrs(e.target.value)}
              rows={4}
              placeholder={'203.0.113.0/24\n2001:db8::/32'}
            />
            <p className='text-xs text-muted-foreground'>
              {t(
                'settings.journal.cidrsHint',
                'One CIDR per line — the egress addresses of your mail system. Required to enable journaling; leave nothing here only if the feature is off.',
              )}
            </p>
          </div>
        </div>

        <p className='text-xs text-muted-foreground'>
          {t(
            'settings.journal.tlsHint',
            'The SMTP listener must be enabled (BICHON_ENABLE_SMTP) and configured for STARTTLS or TLS (BICHON_SMTP_ENCRYPTION); with encryption disabled every journal delivery is refused.',
          )}
        </p>

        <div className='flex items-center justify-end'>
          <Button type='button' onClick={handleSave} disabled={saving}>
            {saving && <Loader2 className='mr-2 h-4 w-4 animate-spin' />}
            {t('settings.journal.save', 'Save journaling')}
          </Button>
        </div>
      </div>
    </div>
  )
}
