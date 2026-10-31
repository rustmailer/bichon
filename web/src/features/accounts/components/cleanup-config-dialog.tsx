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
import { useEffect, useMemo, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { useTranslation } from 'react-i18next'
import { z } from 'zod'
import { isValidCronExpression } from '@/lib/cron'
import { AlertTriangle, ChevronDown } from 'lucide-react'
import type { AccountModel } from '@/api/account/api'
import { update_account } from '@/api/account/api'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Switch } from '@/components/ui/switch'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from '@/components/ui/collapsible'
import { useToast } from '@/hooks/use-toast'

interface CleanupConfigDialogProps {
  account: AccountModel
  open: boolean
  onOpenChange: (open: boolean) => void
}

type Preset = 'manual' | 'daily' | 'weekly' | 'monthly'

const WEEKDAYS = ['SUN', 'MON', 'TUE', 'WED', 'THU', 'FRI', 'SAT']
const HOURS = Array.from({ length: 24 }, (_, hour) => hour)

function buildCron(
  preset: Preset,
  hour: number,
  weekday: string,
  monthDay: number
): string {
  switch (preset) {
    case 'manual':
      return ''
    case 'daily':
      return `0 0 ${hour} * * *`
    case 'weekly':
      return `0 0 ${hour} * * ${weekday}`
    case 'monthly':
      return `0 0 ${hour} ${monthDay} * *`
  }
}

/** Map a stored 6-field cron onto the friendly picker. Unrecognised values
 *  keep their raw string unless the user touches the schedule controls. */
function parseCron(cron: string | null | undefined): {
  preset: Preset
  hour: number
  weekday: string
  monthDay: number
} {
  const fallback = { preset: 'daily' as Preset, hour: 3, weekday: 'SUN', monthDay: 1 }
  if (!cron || !cron.trim()) return { ...fallback, preset: 'manual' }
  const fields = cron.trim().split(/\s+/)
  if (fields.length !== 6) return fallback
  const hour = Number(fields[2])
  if (!Number.isInteger(hour) || hour < 0 || hour > 23) return fallback
  if (fields[3] === '*' && fields[4] === '*' && fields[5] !== '*') {
    return { preset: 'weekly', hour, weekday: fields[5], monthDay: 1 }
  }
  if (fields[3] === '*' && fields[4] === '*' && fields[5] === '*') {
    return { preset: 'daily', hour, weekday: 'SUN', monthDay: 1 }
  }
  if (/^\d{1,2}$/.test(fields[3]) && fields[4] === '*' && fields[5] === '*') {
    return {
      preset: 'monthly',
      hour,
      weekday: 'SUN',
      monthDay: Number(fields[3]),
    }
  }
  return fallback
}

/**
 * Source-mailbox cleanup configuration. Deliberately **outside** the account
 * create/edit form: the feature is not needed at creation time and only adds
 * friction there. Saving here sends only `remote_cleanup` through the account
 * update API, leaving every other field untouched (backward compatible).
 */
export function CleanupConfigDialog({
  account,
  open,
  onOpenChange,
}: CleanupConfigDialogProps) {
  const { t } = useTranslation()
  const { toast } = useToast()
  const queryClient = useQueryClient()

  const [enabled, setEnabled] = useState(false)
  const [preset, setPreset] = useState<Preset>('daily')
  const [hour, setHour] = useState(3)
  const [weekday, setWeekday] = useState('SUN')
  const [monthDay, setMonthDay] = useState(1)
  const [originalSchedule, setOriginalSchedule] = useState<string | null>(null)
  const [scheduleTouched, setScheduleTouched] = useState(false)
  const [foldersText, setFoldersText] = useState('')
  const [eligibleDays, setEligibleDays] = useState(30)
  const [minAgeText, setMinAgeText] = useState('')
  const [maxPerRun, setMaxPerRun] = useState(50)
  const [skipFlagged, setSkipFlagged] = useState(true)
  const [saving, setSaving] = useState(false)
  const [confirmRemove, setConfirmRemove] = useState(false)
  const [errors, setErrors] = useState<Record<string, string[] | undefined>>({})


  useEffect(() => {
    if (!open) return
    const config = account.remote_cleanup
    const parsed = parseCron(config?.schedule)
    setEnabled(config?.enabled ?? false)
    setPreset(parsed.preset)
    setHour(parsed.hour)
    setWeekday(parsed.weekday)
    setMonthDay(parsed.monthDay)
    setOriginalSchedule(config?.schedule ?? null)
    setScheduleTouched(false)
    setFoldersText(config?.folders?.join(', ') ?? '')
    setEligibleDays(config?.eligible_after_archive_days ?? 30)
    setMinAgeText(
      config?.min_message_age_days ? String(config.min_message_age_days) : ''
    )
    setMaxPerRun(config?.max_messages_per_run ?? 50)
    setSkipFlagged(config?.skip_flagged ?? true)
    setConfirmRemove(false)
    setErrors({})
  }, [open, account])

  const schedule = useMemo(
    () => buildCron(preset, hour, weekday, monthDay),
    [preset, hour, weekday, monthDay]
  )

  const markScheduleTouched = (fn: () => void) => {
    setScheduleTouched(true)
    fn()
  }

  const held = !!account.legal_hold

  const save = async () => {
    const folders = foldersText
      .split(',')
      .map((name) => name.trim())
      .filter(Boolean)
    const minAge = minAgeText.trim() === '' ? null : Number(minAgeText)
    const payload = {
      enabled,
      // Preserve an unrecognised stored cron unless the user edited it.
      schedule:
        !scheduleTouched && originalSchedule
          ? originalSchedule
          : preset === 'manual'
            ? null
            : schedule,
      folders: folders.length ? folders : null,
      eligible_after_archive_days: eligibleDays,
      min_message_age_days: minAge,
      max_messages_per_run: maxPerRun,
      skip_flagged: skipFlagged,
    }

    const schema = z
      .object({
        enabled: z.boolean(),
        schedule: z
          .string()
          .nullable()
          .refine((value) => !value || isValidCronExpression(value), {
            message: t(
              'validation.invalidCronExpression',
              'Invalid cron expression'
            ),
          }),
        folders: z
          .array(
            z
              .string()
              .min(
                1,
                t(
                  'accounts.cleanup.foldersInvalid',
                  'Folder names must not be empty.'
                )
              )
          )
          .nullable(),
        eligible_after_archive_days: z
          .number()
          .int()
          .min(
            0,
            t(
              'accounts.cleanup.graceRange',
              'Grace period must be between 0 and 3650 days.'
            )
          )
          .max(
            3650,
            t(
              'accounts.cleanup.graceRange',
              'Grace period must be between 0 and 3650 days.'
            )
          ),
        min_message_age_days: z
          .number()
          .int()
          .min(
            1,
            t(
              'accounts.cleanup.minAgeRange',
              'Message age must be between 1 and 36500 days.'
            )
          )
          .max(
            36500,
            t(
              'accounts.cleanup.minAgeRange',
              'Message age must be between 1 and 36500 days.'
            )
          )
          .nullable(),
        max_messages_per_run: z
          .number()
          .int()
          .min(
            1,
            t(
              'accounts.cleanup.maxPerRunRange',
              'Max messages per batch must be between 1 and 500.'
            )
          )
          .max(
            500,
            t(
              'accounts.cleanup.maxPerRunRange',
              'Max messages per batch must be between 1 and 500.'
            )
          ),
        skip_flagged: z.boolean(),
      })
      .superRefine((value, ctx) => {
        // Automatic runs need a schedule; manual-only configurations
        // (`enabled=false`) are valid and are what "Run now" uses.
        if (value.enabled && !value.schedule?.trim()) {
          ctx.addIssue({
            code: z.ZodIssueCode.custom,
            path: ['schedule'],
            message: t(
              'accounts.cleanup.scheduleRequired',
              'Choose a schedule, or leave scheduled moves disabled.'
            ),
          })
        }
      })

    const parsed = schema.safeParse(payload)
    if (!parsed.success) {
      setErrors(parsed.error.flatten().fieldErrors)
      return
    }
    setErrors({})
    setSaving(true)
    try {
      await update_account(account.id, { remote_cleanup: parsed.data })
      await queryClient.invalidateQueries({ queryKey: ['account-list'] })
      toast({ title: t('accounts.cleanup.saved', 'Cleanup settings saved') })
      onOpenChange(false)
    } catch (error: any) {
      toast({
        variant: 'destructive',
        title: t('accounts.cleanup.saveFailed', 'Failed to save cleanup settings'),
        description:
          error?.response?.data?.message ?? error?.message ?? String(error),
      })
    } finally {
      setSaving(false)
    }
  }

  const remove = async () => {
    setSaving(true)
    try {
      await update_account(account.id, { clear_remote_cleanup: true })
      await queryClient.invalidateQueries({ queryKey: ['account-list'] })
      toast({ title: t('accounts.cleanup.saved', 'Cleanup settings saved') })
      onOpenChange(false)
    } catch (error: any) {
      toast({
        variant: 'destructive',
        title: t('accounts.cleanup.saveFailed', 'Failed to save cleanup settings'),
        description:
          error?.response?.data?.message ?? error?.message ?? String(error),
      })
    } finally {
      setSaving(false)
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className='max-w-4xl max-h-[90vh] [&_button]:text-xs [&_h4]:text-xs [&_input]:text-xs [&_label]:text-xs [&_[role=combobox]]:text-xs'>
        <DialogHeader>
          <DialogTitle>
            {t('accounts.cleanup.dialogTitle', 'Source mailbox cleanup')}
          </DialogTitle>
          <DialogDescription>
            {t(
              'accounts.cleanup.dialogDesc',
              'Move archived mail into bichon_auto_created_trash, a mailbox on your IMAP server, after a grace period. Moving is reversible; permanently deleting from that server mailbox is a separate, confirmed action in the account menu.'
            )}
          </DialogDescription>
        </DialogHeader>

        {/* Native scroll container: Radix's ScrollArea needs a definite height,
            a max-height alone leaves it unable to scroll (see reports dialog). */}
        <div className='max-h-[60vh] overflow-y-auto pr-4'>
          <div className='space-y-6 py-2 text-xs'>
            {held && (
              <div className='flex items-start gap-2 rounded-md border border-amber-500/40 bg-amber-500/10 p-3 text-xs text-amber-600'>
                <AlertTriangle className='mt-0.5 h-4 w-4 shrink-0' />
                <span>
                  {t(
                    'accounts.cleanup.held',
                    'This account is on legal hold. Cleanup runs will be refused.'
                  )}
                </span>
              </div>
            )}
            {account.retention_days !== undefined &&
              account.retention_days !== null &&
              account.retention_days > 0 &&
              account.retention_days <= eligibleDays && (
                <div className='flex items-start gap-2 rounded-md border border-amber-500/40 bg-amber-500/10 p-3 text-xs text-amber-600'>
                  <AlertTriangle className='mt-0.5 h-4 w-4 shrink-0' />
                  <span>
                    {t('accounts.cleanup.retentionWarning', {
                      retention: account.retention_days,
                      grace: eligibleDays,
                      defaultValue:
                        'Local mail is kept for {{retention}} days, which is not longer than the cleanup wait ({{grace}} days). Bichon may delete the local copy before cleanup can move it; consider keeping local mail longer.',
                    })}
                  </span>
                </div>
              )}
            <div className='flex items-start justify-between gap-4 rounded-md border p-3'>
              <div className='space-y-1'>
                <Label>{t('accounts.cleanup.enabled', 'Enable scheduled moves')}</Label>
                <p className='text-xs text-muted-foreground'>
                  {t(
                    'accounts.cleanup.enabledDesc',
                    'Also choose a frequency below. Nothing runs automatically without this switch.'
                  )}
                </p>
              </div>
              <Switch checked={enabled} onCheckedChange={setEnabled} />
            </div>

            <div className='space-y-2'>
              <Label>{t('accounts.cleanup.frequency', 'Frequency')}</Label>
              <div className='flex flex-wrap items-center gap-2'>
                <Select
                  value={preset}
                  onValueChange={(value) =>
                    markScheduleTouched(() => setPreset(value as Preset))
                  }
                >
                  <SelectTrigger className='w-[200px]'>
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem className='text-xs' value='manual'>
                      {t('backup.target.freqManual', 'Manual only')}
                    </SelectItem>
                    <SelectItem className='text-xs' value='daily'>
                      {t('backup.target.freqDaily', 'Daily')}
                    </SelectItem>
                    <SelectItem className='text-xs' value='weekly'>
                      {t('backup.target.freqWeekly', 'Weekly')}
                    </SelectItem>
                    <SelectItem className='text-xs' value='monthly'>
                      {t('accounts.cleanup.freqMonthly', 'Monthly')}
                    </SelectItem>
                  </SelectContent>
                </Select>

                {preset === 'daily' && (
                  <span className='flex items-center gap-2 text-sm text-muted-foreground'>
                    {t('backup.target.execAt', 'at')}
                    <HourSelect
                      value={hour}
                      onChange={(value) =>
                        markScheduleTouched(() => setHour(value))
                      }
                    />
                  </span>
                )}
                {preset === 'weekly' && (
                  <span className='flex items-center gap-2 text-sm text-muted-foreground'>
                    {t('backup.target.execOn', 'on')}
                    <Select
                      value={weekday}
                      onValueChange={(value) =>
                        markScheduleTouched(() => setWeekday(value))
                      }
                    >
                      <SelectTrigger className='w-[110px]'>
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        {WEEKDAYS.map((day) => (
                          <SelectItem className='text-xs' key={day} value={day}>
                            {t(`backup.weekday.${day}`, day)}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                    {t('backup.target.execAt', 'at')}
                    <HourSelect
                      value={hour}
                      onChange={(value) =>
                        markScheduleTouched(() => setHour(value))
                      }
                    />
                  </span>
                )}
                {preset === 'monthly' && (
                  <span className='flex items-center gap-2 text-sm text-muted-foreground'>
                    {t('accounts.cleanup.monthDay', 'on day')}
                    <Select
                      value={String(monthDay)}
                      onValueChange={(value) =>
                        markScheduleTouched(() => setMonthDay(Number(value)))
                      }
                    >
                      <SelectTrigger className='w-[80px]'>
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent className='max-h-64'>
                        {Array.from({ length: 28 }, (_, index) => index + 1).map(
                          (day) => (
                            <SelectItem className='text-xs' key={day} value={String(day)}>
                              {day}
                            </SelectItem>
                          )
                        )}
                      </SelectContent>
                    </Select>
                    {t('backup.target.execAt', 'at')}
                    <HourSelect
                      value={hour}
                      onChange={(value) =>
                        markScheduleTouched(() => setHour(value))
                      }
                    />
                  </span>
                )}
              </div>
              <p className='text-xs text-muted-foreground'>
                {t(
                  'accounts.cleanup.scheduleDesc',
                  'Only one run at a time; runs are intentionally slow.'
                )}
              </p>
              {errors.schedule?.[0] && (
                <p className='text-xs text-destructive'>{errors.schedule[0]}</p>
              )}
              <Collapsible>
                <CollapsibleTrigger className='group flex items-center gap-1 text-xs text-muted-foreground hover:text-foreground'>
                  <ChevronDown className='h-3.5 w-3.5 transition-transform group-data-[state=open]:rotate-180' />
                  {t('backup.target.advancedCron', 'Advanced: cron expression')}
                </CollapsibleTrigger>
                <CollapsibleContent className='pt-2'>
                  <Input
                    value={
                      preset === 'manual'
                        ? ''
                        : scheduleTouched || !originalSchedule
                          ? schedule
                          : originalSchedule
                    }
                    readOnly
                    className='w-[240px] bg-muted font-mono text-xs'
                  />
                  <p className='mt-1.5 text-xs text-muted-foreground'>
                    {t(
                      'backup.target.cronHint',
                      'Six fields: second minute hour day month weekday, evaluated in UTC.'
                    )}
                  </p>
                </CollapsibleContent>
              </Collapsible>
            </div>

            <div className='grid gap-5 sm:grid-cols-2'>
              <div className='space-y-1.5'>
                <Label>
                  {t('accounts.cleanup.grace', 'Archive age before moving (days)')}
                </Label>
                <Input
                  type='number'
                  min={0}
                  max={3650}
                  value={eligibleDays}
                  onChange={(event) => setEligibleDays(Number(event.target.value))}
                />
                <p className='text-xs text-muted-foreground'>
                  {t(
                    'accounts.cleanup.graceDesc',
                    'Counted from the moment Bichon saved the message locally. 0 moves messages on the next run; default 30.'
                  )}
                </p>
                {eligibleDays < 7 && (
                  <p className='flex items-start gap-1.5 rounded-md bg-muted/40 p-2 text-xs text-amber-600'>
                    <AlertTriangle className='mt-0.5 h-3.5 w-3.5 shrink-0' />
                    <span>
                      {t(
                        'accounts.cleanup.shortGraceWarning',
                        'Under 7 days a message can be moved right after it is archived, before any backup captures it. The move is reversible, but the local archive is then the only verified copy.'
                      )}
                    </span>
                  </p>
                )}
                {errors.eligible_after_archive_days?.[0] && (
                  <p className='text-xs text-destructive'>
                    {errors.eligible_after_archive_days[0]}
                  </p>
                )}
              </div>
              <div className='space-y-1.5'>
                <Label>
                  {t('accounts.cleanup.minAge', 'Only move messages older than (days, optional)')}
                </Label>
                <Input
                  type='number'
                  min={1}
                  value={minAgeText}
                  onChange={(event) => setMinAgeText(event.target.value)}
                  placeholder={t('accounts.cleanup.minAgePlaceholder', 'No extra floor')}
                />
                <p className='text-xs text-muted-foreground'>
                  {t(
                    'accounts.cleanup.minAgeDesc',
                    'Compares the date in the email with the date your mail server received it and uses the newer one, so a message moves only when both are older than this. Leave empty if you already limit archiving by date.'
                  )}
                </p>
                {errors.min_message_age_days?.[0] && (
                  <p className='text-xs text-destructive'>
                    {errors.min_message_age_days[0]}
                  </p>
                )}
              </div>
              <div className='space-y-1.5'>
                <Label>{t('accounts.cleanup.maxPerRun', 'Max messages per batch')}</Label>
                <Input
                  type='number'
                  min={1}
                  max={500}
                  value={maxPerRun}
                  onChange={(event) => setMaxPerRun(Number(event.target.value))}
                />
                <p className='text-xs text-muted-foreground'>
                  {t(
                    'accounts.cleanup.maxPerRunDesc',
                    'Messages moved per batch. A scheduled run repeats batches until nothing eligible is left; a very large backlog continues on the next scheduled run.'
                  )}
                </p>
                {errors.max_messages_per_run?.[0] && (
                  <p className='text-xs text-destructive'>
                    {errors.max_messages_per_run[0]}
                  </p>
                )}
              </div>
              <div className='space-y-1.5'>
                <Label>{t('accounts.cleanup.folders', 'Source folders (optional)')}</Label>
                <Input
                  value={foldersText}
                  onChange={(event) => setFoldersText(event.target.value)}
                  placeholder={t(
                    'accounts.cleanup.foldersPlaceholder',
                    'Default: download folders'
                  )}
                />
                <p className='text-xs text-muted-foreground'>
                  {t(
                    'accounts.cleanup.foldersDesc',
                    "Comma-separated folder names. Leave empty to use the account's download folders. System folders (Trash, Junk, Drafts, Sent) and the destination folder are always skipped."
                  )}
                </p>
                {errors.folders?.[0] && (
                  <p className='text-xs text-destructive'>{errors.folders[0]}</p>
                )}
              </div>
            </div>

            {minAgeText.trim() !== '' && Number.isFinite(Number(minAgeText)) && (
              <p className='flex items-start gap-1.5 rounded-md bg-muted/40 p-3 text-xs text-muted-foreground'>
                <AlertTriangle className='mt-0.5 h-3.5 w-3.5 shrink-0' />
                <span>
                  {t('accounts.cleanup.ageBothHint', {
                    grace: eligibleDays,
                    minAge: Number(minAgeText),
                    defaultValue:
                      'Both conditions must be met: Bichon must have archived the message at least {{grace}} days ago AND the message itself must be older than {{minAge}} days. Mail archived more recently is skipped as too_recently_archived even when the email is old.',
                  })}
                </span>
              </p>
            )}

            <div className='space-y-3 rounded-md border p-3'>
              <label className='flex items-start gap-3'>
                <Checkbox
                  checked={skipFlagged}
                  onCheckedChange={(value) => setSkipFlagged(value === true)}
                />
                <span className='space-y-1'>
                  <Label>{t('accounts.cleanup.skipFlagged', 'Keep starred messages')}</Label>
                  <p className='text-xs text-muted-foreground'>
                    {t(
                      'accounts.cleanup.skipFlaggedDesc',
                      'Messages you starred stay in the source mailbox, even when they are old enough.'
                    )}
                  </p>
                </span>
              </label>
            </div>

            {account.remote_cleanup && (
              <div className='flex flex-wrap items-center justify-between gap-2 rounded-md border border-destructive/30 p-3'>
                <p className='text-xs text-muted-foreground'>
                  {t(
                    'accounts.cleanup.removeDesc',
                    'Removing this configuration stops scheduled moves. Messages already in bichon_auto_created_trash stay there.'
                  )}
                </p>
                <Button
                  type='button'
                  variant={confirmRemove ? 'destructive' : 'outline'}
                  size='sm'
                  disabled={saving}
                  onClick={() => {
                    if (!confirmRemove) {
                      setConfirmRemove(true)
                      return
                    }
                    remove()
                  }}
                >
                  {confirmRemove
                    ? t('accounts.cleanup.removeConfirm', 'Click again to confirm removal')
                    : t('accounts.cleanup.remove', 'Remove cleanup configuration')}
                </Button>
              </div>
            )}
          </div>
        </div>

        <DialogFooter>
          <Button
            type='button'
            variant='outline'
            onClick={() => onOpenChange(false)}
            disabled={saving}
          >
            {t('common.cancel', 'Cancel')}
          </Button>
          <Button type='button' onClick={save} disabled={saving}>
            {saving ? t('common.loading', 'Loading') : t('common.save', 'Save')}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

function HourSelect({
  value,
  onChange,
}: {
  value: number
  onChange: (value: number) => void
}) {
  return (
    <Select value={String(value)} onValueChange={(next) => onChange(Number(next))}>
      <SelectTrigger className='w-[88px]'>
        <SelectValue />
      </SelectTrigger>
      <SelectContent className='max-h-64'>
        {HOURS.map((hour) => (
          <SelectItem className='text-xs' key={hour} value={String(hour)}>
            {String(hour).padStart(2, '0')}:00
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  )
}

