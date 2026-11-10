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
import { useEffect, useMemo, useRef, useState } from 'react'
import {
  AlertTriangle,
  Check,
  ChevronDown,
  Loader2,
  RotateCcw,
  Trash2,
} from 'lucide-react'
import { useTranslation } from 'react-i18next'
import {
  getBackupTargetConfig,
  updateBackupTargetConfig,
  type BackupConfigUpdate,
  type BackupConfigView,
  type BackupTargetType,
} from '@/api/backup/api'
import { useCurrentUser } from '@/hooks/use-current-user'
import { useToast } from '@/hooks/use-toast'
import { isValidCronExpression } from '@/lib/cron'
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from '@/components/ui/alert-dialog'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from '@/components/ui/collapsible'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { Switch } from '@/components/ui/switch'
import { PasswordInput } from '@/components/password-input'

/** Debounce before an edit batch is auto-saved. */
const AUTOSAVE_DELAY_MS = 1500

/** Friendly schedule presets → 6-field cron (sec min hour dom mon dow, UTC —
 *  the `cron` crate format the server parses). Day-of-week uses names because
 *  the crate's numeric convention (1 = Sunday) is easy to get wrong. */
type SchedulePreset =
  | 'manual'
  | 'hourly'
  | 'sixHours'
  | 'daily'
  | 'weekly'

const WEEKDAYS = ['SUN', 'MON', 'TUE', 'WED', 'THU', 'FRI', 'SAT']

const buildCron = (
  preset: SchedulePreset,
  dailyHour: number,
  weeklyDay: string,
  weeklyHour: number
) => {
  switch (preset) {
    case 'manual':
      return ''
    case 'hourly':
      return '0 0 * * * *'
    case 'sixHours':
      return '0 0 */6 * * *'
    case 'daily':
      return `0 0 ${dailyHour} * * *`
    case 'weekly':
      return `0 0 ${weeklyHour} * * ${weeklyDay}`
  }
}

/** Reverse of `buildCron`: map a stored cron back onto the friendly picker.
 *  Cron expressions a preset can't express (a hand-edited or older custom
 *  value) are *salvaged*, not discarded: a weekly day-of-week wins, else a
 *  numeric hour becomes the daily time, else the default daily 02:00. */
const parseCron = (
  cron: string
): {
  preset: SchedulePreset
  dailyHour: number
  weeklyDay: string
  weeklyHour: number
} => {
  const fallback = {
    dailyHour: 2,
    weeklyDay: 'MON',
    weeklyHour: 3,
  }
  if (!cron.trim()) return { preset: 'manual', ...fallback }
  if (cron === '0 0 * * * *') return { preset: 'hourly', ...fallback }
  if (cron === '0 0 */6 * * *') return { preset: 'sixHours', ...fallback }
  const daily = cron.match(/^0 0 (\d{1,2}) \* \* \*$/)
  if (daily && Number(daily[1]) <= 23)
    return { preset: 'daily', ...fallback, dailyHour: Number(daily[1]) }
  const weekly = cron.match(
    /^0 0 (\d{1,2}) \* \* (SUN|MON|TUE|WED|THU|FRI|SAT)$/
  )
  if (weekly && Number(weekly[1]) <= 23)
    return {
      preset: 'weekly',
      ...fallback,
      weeklyDay: weekly[2],
      weeklyHour: Number(weekly[1]),
    }
  // Unrecognized. Salvage the hour for a daily-style mapping; a weekday in
  // the day-of-week field upgrades that to weekly.
  const fields = cron.trim().split(/\s+/)
  const hour =
    fields.length === 6 &&
    /^\d{1,2}$/.test(fields[2]) &&
    Number(fields[2]) <= 23
      ? Number(fields[2])
      : null
  if (hour !== null && fields[3] === '*' && /^(SUN|MON|TUE|WED|THU|FRI|SAT)$/.test(fields[5]))
    return { preset: 'weekly', ...fallback, weeklyDay: fields[5], weeklyHour: hour }
  if (hour !== null) return { preset: 'daily', ...fallback, dailyHour: hour }
  return { preset: 'daily', ...fallback }
}

const HOUR_OPTIONS = Array.from({ length: 24 }, (_, h) => h)

type SaveState = 'idle' | 'pending' | 'saving' | 'saved' | 'error'

/** WebUI-configured S3-only backup target (design doc §9). Everything
 *  auto-saves after a short debounce; cron hides behind a friendly frequency
 *  picker, retention is a single "keep the most recent N" count. A failed
 *  backup never triggers cleanup, so recent restore points are always safe.
 *  Credentials never round-trip: `*_set` flags render placeholders, a Clear
 *  button sends `''` to drop the page override back to env. Changing the
 *  prefix starts a fresh backup chain, so that one edit asks before it
 *  auto-saves. */
export function BackupConfigForm({ onSaved }: { onSaved?: () => void }) {
  const { t } = useTranslation()
  const { toast } = useToast()
  const { require_any_permission } = useCurrentUser()

  const [loading, setLoading] = useState(true)
  const [current, setCurrent] = useState<BackupConfigView | null>(null)

  const [enabled, setEnabled] = useState(false)
  const [prefix, setPrefix] = useState('')
  const [targetType, setTargetType] = useState<BackupTargetType>('s3')
  const [localDir, setLocalDir] = useState('')

  // Schedule: friendly picker fields are the source of truth; `schedule` is
  // derived. A stored cron a preset can't express is only rewritten once the
  // user actually touches the schedule fields (`scheduleTouched`) — loading
  // must never silently "normalize" it away.
  const [preset, setPreset] = useState<SchedulePreset>('daily')
  const [dailyHour, setDailyHour] = useState(2)
  const [weeklyDay, setWeeklyDay] = useState('MON')
  const [weeklyHour, setWeeklyHour] = useState(3)
  const [scheduleTouched, setScheduleTouched] = useState(false)
  const schedule = useMemo(
    () => buildCron(preset, dailyHour, weeklyDay, weeklyHour),
    [preset, dailyHour, weeklyDay, weeklyHour]
  )

  // Retention: one count only — the restore points recent enough to matter.
  const [keepLast, setKeepLast] = useState(7)

  const [s3Endpoint, setS3Endpoint] = useState('')
  const [s3Region, setS3Region] = useState('')
  const [s3Bucket, setS3Bucket] = useState('')
  const [s3AccessKey, setS3AccessKey] = useState('')
  const [s3SecretKey, setS3SecretKey] = useState('')
  const [clearAccessKey, setClearAccessKey] = useState(false)
  const [clearSecretKey, setClearSecretKey] = useState(false)

  const [saveState, setSaveState] = useState<SaveState>('idle')
  const [saveError, setSaveError] = useState<string | null>(null)
  const [savedAt, setSavedAt] = useState<Date | null>(null)
  const [retryNonce, setRetryNonce] = useState(0)
  // Changing the target or the prefix starts a fresh backup chain, so that
  // edit asks before it auto-saves.
  const [confirmOpen, setConfirmOpen] = useState(false)
  const pendingConfirmPayload = useRef<BackupConfigUpdate | null>(null)

  const savingRef = useRef(false)
  const queuedRef = useRef(false)
  const lastFailedRef = useRef<string | null>(null)

  const canManage = require_any_permission(['backup:manage', 'system:root'])

  useEffect(() => {
    let mounted = true
    getBackupTargetConfig()
      .then((cfg) => {
        if (!mounted) return
        setCurrent(cfg)
        setEnabled(cfg.enabled)
        setPrefix(cfg.prefix)
        const s = parseCron(cfg.schedule)
        setPreset(s.preset)
        setDailyHour(s.dailyHour)
        setWeeklyDay(s.weeklyDay)
        setWeeklyHour(s.weeklyHour)
        setScheduleTouched(false)
        setKeepLast(cfg.retention.keep_last)
        setTargetType(cfg.target_type)
        setLocalDir(cfg.local_dir ?? '')
        setS3Endpoint(cfg.s3_endpoint ?? '')
        setS3Region(cfg.s3_region ?? '')
        setS3Bucket(cfg.s3_bucket ?? '')
      })
      .catch(() => {
        if (mounted) {
          toast({
            variant: 'destructive',
            title: t(
              'backup.target.loadFailed',
              'Failed to load backup configuration'
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
    // Load once: depending on `t` would reset the form (discarding unsaved
    // edits) whenever the language — and with it `t`'s identity — changes.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  const buildPayload = (): BackupConfigUpdate | null => {
    if (!current) return null
    const payload: BackupConfigUpdate = {}
    const dirty = (loaded: string | null | undefined, value: string) =>
      value !== (loaded ?? '')
    if ((current.enabled ? '1' : '0') !== (enabled ? '1' : '0'))
      payload.enabled = enabled
    // Only user-edited schedule fields produce a schedule diff: a stored cron
    // the presets can't express maps to a *different* friendly value, and
    // auto-saving that would silently rewrite the user's cron.
    if (scheduleTouched && dirty(current.schedule, schedule))
      payload.schedule = schedule
    if (dirty(current.prefix, prefix)) payload.prefix = prefix
    if (current.retention.keep_last !== keepLast) {
      // The form manages only the recent-count tier; saving zeroes the daily/
      // weekly/monthly tiers so the visible number is the whole truth.
      payload.retention = {
        keep_last: keepLast,
        keep_daily: 0,
        keep_weekly: 0,
        keep_monthly: 0,
      }
    }
    // Switching the target carries the directory in the same request: the
    // autosave sends only diffs, so a lone `target_type: 'local'` would be
    // rejected as an incomplete target before the path edit is even typed.
    // The switch is also held back until the new target's own field is
    // filled in — an incomplete target is not something to persist, and
    // saving it would only produce a spurious error on the way.
    const targetComplete =
      targetType === 's3' ? !!s3Bucket.trim() : !!localDir.trim()
    if (current.target_type !== targetType && targetComplete) {
      payload.target_type = targetType
      if (targetType === 'local') payload.local_dir = localDir
    }
    if (dirty(current.local_dir, localDir)) payload.local_dir = localDir
    if (dirty(current.s3_endpoint, s3Endpoint)) payload.s3_endpoint = s3Endpoint
    if (dirty(current.s3_region, s3Region)) payload.s3_region = s3Region
    if (dirty(current.s3_bucket, s3Bucket)) payload.s3_bucket = s3Bucket
    // Secrets: omit keeps the stored value, '' clears it.
    if (clearAccessKey) payload.s3_access_key = ''
    else if (s3AccessKey.trim()) payload.s3_access_key = s3AccessKey
    if (clearSecretKey) payload.s3_secret_key = ''
    else if (s3SecretKey.trim()) payload.s3_secret_key = s3SecretKey
    return Object.keys(payload).length > 0 ? payload : null
  }

  const doSave = async (payload: BackupConfigUpdate) => {
    if (savingRef.current) {
      // A save is in flight; run once more afterwards with freshly
      // recomputed state instead of racing two requests.
      queuedRef.current = true
      return
    }
    savingRef.current = true
    setSaveState('saving')
    try {
      const saved = await updateBackupTargetConfig(payload)
      setCurrent(saved)
      setS3AccessKey('')
      setS3SecretKey('')
      setClearAccessKey(false)
      setClearSecretKey(false)
      setSavedAt(new Date())
      setSaveError(null)
      setSaveState('saved')
      lastFailedRef.current = null
      onSaved?.()
    } catch (err: unknown) {
      const message =
        (err as { response?: { data?: { message?: string } } })?.response?.data
          ?.message || (err instanceof Error ? err.message : String(err))
      lastFailedRef.current = JSON.stringify(payload)
      setSaveError(message)
      setSaveState('error')
    } finally {
      savingRef.current = false
      if (queuedRef.current) {
        queuedRef.current = false
        setRetryNonce((n) => n + 1)
      }
    }
  }

  const payloadDeps = [
    enabled,
    schedule,
    prefix,
    keepLast,
    targetType,
    localDir,
    s3Endpoint,
    s3Region,
    s3Bucket,
    s3AccessKey,
    s3SecretKey,
    clearAccessKey,
    clearSecretKey,
  ]

  useEffect(() => {
    if (loading || !current) return
    const payload = buildPayload()
    if (!payload) {
      // Keep a just-saved/error indicator visible; only clear a stale
      // "pending" dot once nothing is dirty anymore.
      setSaveState((s) => (s === 'pending' ? 'idle' : s))
      return
    }
    // Same structural check the cleanup dialog uses: the server's Rust cron
    // parser is authoritative, but a bad schedule should not leave the form.
    if (
      payload.schedule !== undefined &&
      payload.schedule.trim() !== '' &&
      !isValidCronExpression(payload.schedule)
    ) {
      setSaveError(t('validation.invalidCronExpression', 'Invalid cron expression'))
      setSaveState('error')
      return
    }
    const sig = JSON.stringify(payload)
    // Don't auto-retry a payload the server just rejected — that would loop.
    // The retry button (or any further edit) clears the block.
    if (sig === lastFailedRef.current) return
    // A target or prefix change starts a fresh backup chain in a new
    // location — confirm before applying. Both must be checked: a target
    // switch does not change the prefix, so the prefix test alone would let
    // the user switch and then wonder where their restore points went.
    const switchingTarget =
      payload.target_type !== undefined &&
      payload.target_type !== current.target_type
    const changingPrefix =
      payload.prefix !== undefined && (current.prefix ?? '') !== ''
    if (switchingTarget || changingPrefix) {
      pendingConfirmPayload.current = payload
      setConfirmOpen(true)
      return
    }
    setSaveState('pending')
    const timer = setTimeout(() => void doSave(payload), AUTOSAVE_DELAY_MS)
    return () => clearTimeout(timer)
    // `buildPayload`/`doSave` close over the same states listed here.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [...payloadDeps, retryNonce, current, loading])

  const handleRetry = () => {
    lastFailedRef.current = null
    setRetryNonce((n) => n + 1)
  }

  const confirmChainChange = () => {
    const payload = pendingConfirmPayload.current
    setConfirmOpen(false)
    pendingConfirmPayload.current = null
    if (payload) void doSave(payload)
  }

  const cancelChainChange = () => {
    setConfirmOpen(false)
    pendingConfirmPayload.current = null
    // Revert the edits; the effect then re-runs and saves the rest.
    setPrefix(current?.prefix ?? '')
    setTargetType(current?.target_type ?? 's3')
  }

  if (!canManage) {
    return (
      <div className='w-full p-6 text-muted-foreground'>
        {t(
          'backup.target.forbidden',
          'Configuring the backup target requires the backup:manage or system:root permission.'
        )}
      </div>
    )
  }

  if (loading) {
    return (
      <div className='flex h-32 items-center justify-center'>
        <Loader2 className='h-5 w-5 animate-spin text-muted-foreground' />
      </div>
    )
  }

  const bucketMissing = !s3Bucket.trim()
  const localDirMissing = !localDir.trim()
  const targetMissing =
    targetType === 's3' ? bucketMissing : localDirMissing
  // Pure recompute for the header hint: only nag about the missing target
  // field while an edit batch is actually waiting on it, not on a pristine
  // form. A freshly switched target counts as waiting even though
  // `buildPayload` holds the switch back until the field is filled in —
  // otherwise the form would look idle while visibly missing the one thing
  // it needs.
  const hasDirtyEdits =
    buildPayload() !== null ||
    (current?.target_type !== targetType && targetMissing)
  const secretPlaceholder = t(
    'backup.target.secretPlaceholder',
    '******** (unchanged)'
  )

  const saveStatus = (() => {
    if (targetMissing && (hasDirtyEdits || saveState === 'error')) {
      return (
        <span className='flex items-center gap-1.5 text-xs text-destructive'>
          <AlertTriangle className='h-3.5 w-3.5' />
          {targetType === 's3'
            ? t('backup.target.bucketRequired', 'Bucket is required')
            : t('backup.target.localDirRequired', 'A directory is required')}
        </span>
      )
    }
    switch (saveState) {
      case 'pending':
        return (
          <span className='flex items-center gap-1.5 text-xs text-muted-foreground'>
            <span className='h-1.5 w-1.5 animate-pulse rounded-full bg-muted-foreground' />
            {t('backup.target.autoSavePending', 'Unsaved changes…')}
          </span>
        )
      case 'saving':
        return (
          <span className='flex items-center gap-1.5 text-xs text-muted-foreground'>
            <Loader2 className='h-3.5 w-3.5 animate-spin' />
            {t('backup.target.autoSaveSaving', 'Saving…')}
          </span>
        )
      case 'saved':
        return (
          <span className='flex items-center gap-1.5 text-xs text-emerald-600 dark:text-emerald-400'>
            <Check className='h-3.5 w-3.5' />
            {t('backup.target.autoSaveSaved', 'Saved automatically')}
            {savedAt &&
              ` ${String(savedAt.getHours()).padStart(2, '0')}:${String(
                savedAt.getMinutes()
              ).padStart(2, '0')}`}
          </span>
        )
      case 'error':
        return (
          <span className='flex items-center gap-1.5 text-xs text-destructive'>
            <AlertTriangle className='h-3.5 w-3.5 shrink-0' />
            <span className='max-w-[280px] truncate' title={saveError ?? ''}>
              {t('backup.target.autoSaveFailed', 'Auto-save failed')}
              {saveError ? `: ${saveError}` : ''}
            </span>
            <Button
              variant='ghost'
              size='sm'
              className='h-6 gap-1 px-2 text-xs'
              onClick={handleRetry}
            >
              <RotateCcw className='h-3 w-3' />
              {t('backup.target.retry', 'Retry')}
            </Button>
          </span>
        )
      default:
        return null
    }
  })()

  return (
    <Card>
      <CardHeader className='flex flex-row items-center justify-between space-y-0 pb-3'>
        <CardTitle className='text-xs font-medium'>
          {t('backup.target.title', 'Backup target & schedule')}
        </CardTitle>
        {saveStatus}
      </CardHeader>
      <CardContent className='space-y-6'>
        {/* ── Enable ─────────────────────────────────────────────── */}
        <div className='flex items-center justify-between rounded-md border px-4 py-3'>
          <div className='space-y-0.5'>
            <Label className='text-xs'>
              {t('backup.target.enabled', 'Backups enabled')}
            </Label>
            <p className='text-xs text-muted-foreground'>
              {t(
                'backup.target.enabledHint',
                'When enabled, scheduled backups run on the configured schedule.'
              )}
            </p>
          </div>
          <Switch checked={enabled} onCheckedChange={setEnabled} />
        </div>

        {/* ── Target ─────────────────────────────────────────────── */}
        <div className='space-y-4'>
          <div className='space-y-2'>
            <Label htmlFor='backup-target-type' className='text-xs'>
              {t('backup.target.targetType', 'Backup target')}
            </Label>
            <Select
              value={targetType}
              onValueChange={(v) => setTargetType(v as BackupTargetType)}
            >
              <SelectTrigger
                id='backup-target-type'
                className='w-[240px] text-xs'
              >
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value='s3' className='text-xs'>
                  {t('backup.target.targetTypeS3', 'S3-compatible object store')}
                </SelectItem>
                <SelectItem value='local' className='text-xs'>
                  {t('backup.target.targetTypeLocal', 'Local directory')}
                </SelectItem>
              </SelectContent>
            </Select>
          </div>

          {targetType === 's3' && (
            <>
          <div className='grid gap-4 sm:grid-cols-2'>
            <div className='space-y-2'>
              <Label htmlFor='backup-s3-endpoint' className='text-xs'>
                {t('backup.target.s3Endpoint', 'Endpoint')}
              </Label>
              <Input
                id='backup-s3-endpoint'
                value={s3Endpoint}
                placeholder='s3.amazonaws.com'
                onChange={(e) => setS3Endpoint(e.target.value)}
                className='text-xs md:text-xs'
              />
            </div>
            <div className='space-y-2'>
              <Label htmlFor='backup-s3-region' className='text-xs'>
                {t('backup.target.s3Region', 'Region')}
              </Label>
              <Input
                id='backup-s3-region'
                value={s3Region}
                placeholder='us-east-1'
                onChange={(e) => setS3Region(e.target.value)}
                className='text-xs md:text-xs'
              />
              <p className='text-xs text-muted-foreground'>
                {t(
                  'backup.target.s3RegionHint',
                  'Local S3 servers (MinIO, SeaweedFS…) work with the default us-east-1; Cloudflare R2 needs "auto"; Garage must match its configured region.'
                )}
              </p>
            </div>
          </div>
          <div className='grid gap-4 sm:grid-cols-2'>
            <div className='space-y-2'>
              <Label htmlFor='backup-s3-bucket' className='text-xs'>
                {t('backup.target.s3Bucket', 'Bucket')}
              </Label>
              <Input
                id='backup-s3-bucket'
                value={s3Bucket}
                placeholder='bichon-backups'
                onChange={(e) => setS3Bucket(e.target.value)}
                className='text-xs md:text-xs'
                aria-invalid={bucketMissing}
              />
            </div>
          </div>
          <p className='text-xs text-muted-foreground'>
            {t(
              'backup.target.s3EndpointHint',
              'For a custom backend type the endpoint with a scheme (e.g. http://localhost:9000 for MinIO); a bare host (s3.amazonaws.com) uses AWS signature/region resolution.'
            )}
          </p>
          <div className='grid gap-4 sm:grid-cols-2'>
            <div className='space-y-2'>
              <Label htmlFor='backup-s3-access-key' className='text-xs'>
                {t('backup.target.s3AccessKey', 'Access key')}
              </Label>
              <PasswordInput
                id='backup-s3-access-key'
                value={clearAccessKey ? '' : s3AccessKey}
                placeholder={
                  current?.s3_access_key_set ? secretPlaceholder : undefined
                }
                className='[&_input]:text-xs'
                onChange={(e) => {
                  // Typing after Clear cancels the clear: otherwise the input
                  // would stay locked empty and the retyped key silently
                  // discarded (and the stored credential dropped) on save.
                  setClearAccessKey(false)
                  setS3AccessKey(e.target.value)
                }}
              />
              {current?.s3_access_key_set && (
                <Button
                  type='button'
                  variant='ghost'
                  size='sm'
                  className='h-6 px-2 text-xs'
                  onClick={() => setClearAccessKey((v) => !v)}
                >
                  <Trash2 className='mr-1 h-3 w-3' />
                  {t('backup.target.clear', 'Clear')}
                </Button>
              )}
            </div>
            <div className='space-y-2'>
              <Label htmlFor='backup-s3-secret-key' className='text-xs'>
                {t('backup.target.s3SecretKey', 'Secret key')}
              </Label>
              <PasswordInput
                id='backup-s3-secret-key'
                value={clearSecretKey ? '' : s3SecretKey}
                placeholder={
                  current?.s3_secret_key_set ? secretPlaceholder : undefined
                }
                className='[&_input]:text-xs'
                onChange={(e) => {
                  setClearSecretKey(false)
                  setS3SecretKey(e.target.value)
                }}
              />
              {current?.s3_secret_key_set && (
                <Button
                  type='button'
                  variant='ghost'
                  size='sm'
                  className='h-6 px-2 text-xs'
                  onClick={() => setClearSecretKey((v) => !v)}
                >
                  <Trash2 className='mr-1 h-3 w-3' />
                  {t('backup.target.clear', 'Clear')}
                </Button>
              )}
            </div>
          </div>
            </>
          )}

          {targetType === 'local' && (
            <div className='space-y-2'>
              <Label htmlFor='backup-local-dir' className='text-xs'>
                {t('backup.target.localDir', 'Directory')}
              </Label>
              <Input
                id='backup-local-dir'
                value={localDir}
                placeholder='/mnt/bichon-backup'
                onChange={(e) => setLocalDir(e.target.value)}
                className='font-mono text-xs md:text-xs'
                aria-invalid={localDirMissing}
              />
              <p className='text-xs text-muted-foreground'>
                {t(
                  'backup.target.localDirHint',
                  'An absolute path that must already exist — create it and mount the volume before configuring it. Bichon never creates it, so a missing mount fails the run instead of quietly filling the underlying filesystem.'
                )}
              </p>
              <p className='text-xs text-muted-foreground'>
                {t(
                  'backup.target.localMountHint',
                  'Point this at a dedicated volume (USB disk, NFS/SMB share, second disk) — a directory on the same disk as the data does not survive that disk failing.'
                )}
              </p>
            </div>
          )}

          {/* Prefix applies to both targets: the key prefix under the bucket,
              or the subdirectory of the local directory. */}
          <div className='space-y-2'>
            <Label htmlFor='backup-prefix' className='text-xs'>
              {t('backup.target.prefix', 'Prefix')}
            </Label>
            <Input
              id='backup-prefix'
              value={prefix}
              placeholder='bichon-backup'
              onChange={(e) => setPrefix(e.target.value)}
              className='font-mono text-xs md:text-xs'
            />
            <p className='text-xs text-muted-foreground'>
              {t(
                'backup.target.prefixHint',
                'Sub-location of the target the backups live under. Changing it starts a fresh backup chain in a new location.'
              )}
            </p>
          </div>
        </div>

        {/* ── Schedule ───────────────────────────────────────────── */}
        <div className='space-y-2'>
          <Label className='text-xs'>
            {t('backup.target.freqLabel', 'Backup frequency')}
          </Label>
          <div className='flex flex-wrap items-center gap-2'>
            <Select
              value={preset}
              onValueChange={(v) => {
                setPreset(v as SchedulePreset)
                setScheduleTouched(true)
              }}
            >
              <SelectTrigger className='w-[200px] text-xs'>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value='manual' className='text-xs'>
                  {t('backup.target.freqManual', 'Manual only')}
                </SelectItem>
                <SelectItem value='hourly' className='text-xs'>
                  {t('backup.target.freqHourly', 'Hourly')}
                </SelectItem>
                <SelectItem value='sixHours' className='text-xs'>
                  {t('backup.target.freqSixHours', 'Every 6 hours')}
                </SelectItem>
                <SelectItem value='daily' className='text-xs'>
                  {t('backup.target.freqDaily', 'Daily')}
                </SelectItem>
                <SelectItem value='weekly' className='text-xs'>
                  {t('backup.target.freqWeekly', 'Weekly')}
                </SelectItem>
              </SelectContent>
            </Select>
            {preset === 'daily' && (
              <span className='flex items-center gap-2 text-xs text-muted-foreground'>
                {t('backup.target.execAt', 'at')}
                <Select
                  value={String(dailyHour)}
                  onValueChange={(v) => {
                    setDailyHour(Number(v))
                    setScheduleTouched(true)
                  }}
                >
                  <SelectTrigger className='w-[88px] text-xs'>
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent className='max-h-64'>
                    {HOUR_OPTIONS.map((h) => (
                      <SelectItem key={h} value={String(h)} className='text-xs'>
                        {String(h).padStart(2, '0')}:00
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </span>
            )}
            {preset === 'weekly' && (
              <span className='flex items-center gap-2 text-xs text-muted-foreground'>
                {t('backup.target.execOn', 'on')}
                <Select
                  value={weeklyDay}
                  onValueChange={(v) => {
                    setWeeklyDay(v)
                    setScheduleTouched(true)
                  }}
                >
                  <SelectTrigger className='w-[110px] text-xs'>
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {WEEKDAYS.map((d) => (
                      <SelectItem key={d} value={d} className='text-xs'>
                        {t(`backup.weekday.${d}`, d)}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
                {t('backup.target.execAt', 'at')}
                <Select
                  value={String(weeklyHour)}
                  onValueChange={(v) => {
                    setWeeklyHour(Number(v))
                    setScheduleTouched(true)
                  }}
                >
                  <SelectTrigger className='w-[88px] text-xs'>
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent className='max-h-64'>
                    {HOUR_OPTIONS.map((h) => (
                      <SelectItem key={h} value={String(h)} className='text-xs'>
                        {String(h).padStart(2, '0')}:00
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </span>
            )}
          </div>
          <Collapsible>
            <CollapsibleTrigger className='group flex items-center gap-1 text-xs text-muted-foreground hover:text-foreground'>
              <ChevronDown className='h-3.5 w-3.5 transition-transform group-data-[state=open]:rotate-180' />
              {t('backup.target.advancedCron', 'Advanced: cron expression')}
            </CollapsibleTrigger>
            <CollapsibleContent className='pt-2'>
              <Input
                value={schedule}
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

        {/* ── Retention ──────────────────────────────────────────── */}
        <div className='space-y-2'>
          <Label className='text-xs'>
            {t('backup.target.retention', 'Retention')}
          </Label>
          <div className='flex flex-wrap items-center gap-2 text-xs'>
            <span className='text-muted-foreground'>
              {t('backup.target.keepLastPrefix', 'Keep the most recent')}
            </span>
            <Input
              id='backup-retention-keep-last'
              type='number'
              min={1}
              value={keepLast}
              onChange={(e) =>
                setKeepLast(Math.max(1, Number(e.target.value) || 1))
              }
              className='w-20 font-mono text-xs md:text-xs'
            />
            <span className='text-muted-foreground'>
              {t('backup.target.keepLastSuffix', 'restore points')}
            </span>
          </div>
          <p className='text-xs text-muted-foreground'>
            {t(
              'backup.target.retentionHint',
              'Older restore points are deleted from the target after each successful backup. A failed backup never triggers cleanup — your existing restore points always stay intact.'
            )}
          </p>
        </div>
      </CardContent>

      {/* Prefix changes start a fresh backup chain — confirm first. */}
      <AlertDialog
        open={confirmOpen}
        onOpenChange={(open) => {
          if (!open) cancelChainChange()
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              {t('backup.target.prefixConfirmTitle', 'Change the prefix?')}
            </AlertDialogTitle>
            <AlertDialogDescription>
              {t(
                'backup.target.prefixConfirmBody',
                'Changing the target or the prefix starts a fresh backup chain at a new location. Existing restore points stay where they are, but this page only ever browses the currently configured target. Apply the change?'
              )}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>
              {t('backup.cancel', 'Cancel')}
            </AlertDialogCancel>
            <AlertDialogAction onClick={confirmChainChange}>
              {t('backup.target.prefixConfirmAction', 'Apply change')}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </Card>
  )
}
