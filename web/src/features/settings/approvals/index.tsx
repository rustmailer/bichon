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
// Dual control (two-person control) settings (Enterprise).
//
// Off by default. Turning it on does not change what any operation does — it
// inserts a second person between the request and the effect for the
// operations listed below.
import { useEffect, useState } from 'react'
import { Loader2 } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import {
  BREAK_GLASS_DISABLED,
  get_approval_config,
  update_approval_config,
  type ApprovalConfig,
} from '@/api/approvals/api'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Switch } from '@/components/ui/switch'
import { useEdition } from '@/hooks/use-edition'
import { useCurrentUser } from '@/hooks/use-current-user'
import { useToast } from '@/hooks/use-toast'

interface ApiErrorLike {
  response?: { data?: { message?: string } }
  message?: string
}

/** Extract a readable message from an axios/API error (the backend returns
 *  `{ message }` JSON; fall back to the transport message otherwise). */
const errorMessage = (e: unknown): string =>
  (e as ApiErrorLike).response?.data?.message ??
  (e as Error).message ??
  'Unknown error'

/** Human labels for the operation discriminators the server reports.
 *
 *  Every op in `available_ops` needs an entry. A missing one is not a crash —
 *  it renders as the raw discriminator (`account.delete`) — which is why this
 *  is easy to forget and worth a comment. Kept in sync with
 *  `features/approvals/target-description.tsx`, which renders the same labels
 *  on the queue. */
const OP_LABELS: Record<string, { key: string; fallback: string }> = {
  'legal_hold.release': {
    key: 'settings.approvals.opLegalHoldRelease',
    fallback: 'Release a legal hold',
  },
  'user.delete': {
    key: 'settings.approvals.opUserDelete',
    fallback: 'Delete a user',
  },
  'account.delete': {
    key: 'settings.approvals.opAccountDelete',
    fallback: 'Delete an account',
  },
  'mailbox.delete': {
    key: 'settings.approvals.opMailboxDelete',
    fallback: 'Delete a mailbox',
  },
  'message.delete': {
    key: 'settings.approvals.opMessageDelete',
    fallback: 'Delete messages',
  },
  'token.revoke': {
    key: 'settings.approvals.opTokenRevoke',
    fallback: 'Revoke an API token',
  },
  'export.batch': {
    key: 'settings.approvals.opExportBatch',
    fallback: 'Export a batch of archived mail',
  },
  'gdpr.erasure': {
    key: 'settings.approvals.opGdprErasure',
    fallback: 'Execute a GDPR erasure request',
  },
  'retention.update': {
    key: 'settings.approvals.opRetentionUpdate',
    fallback: 'Weaken a retention policy',
  },
}

/** The operations that delete data, as opposed to releasing protection. Shown
 *  with a warning because gating them changes day-to-day use: with these on,
 *  deleting a single spam message needs a second person. */
const DESTRUCTIVE_OPS = new Set([
  'user.delete',
  'account.delete',
  'mailbox.delete',
  'message.delete',
  // Not an immediate deletion, but it is the one way to shorten the window
  // that protects data, so it belongs on the warning path.
  'retention.update',
])

/** Operations whose blast radius is not obvious from the label.
 *
 *  `token.revoke` is not a deletion of data, so it is not in
 *  `DESTRUCTIVE_OPS` — but it does end an integration, and it is the one
 *  operation whose target is a *credential*. Worth saying out loud rather than
 *  letting an administrator discover it from a broken pipeline. */
const REVOCATION_OPS = new Set(['token.revoke'])

/** Operations whose *absence* from the policy is the consequential choice.
 *
 *  Every other op behaves as it always did when it is not listed. A GDPR
 *  erasure does not: the request is recorded and then executed on the
 *  submitter's signature alone, which is the opposite of what the workflow
 *  exists for. Saying so here is the only place an administrator can learn it
 *  — the erasure form itself warns the operator, but nobody sees that until
 *  they are already erasing. */
const ERASURE_OPS = new Set(['gdpr.erasure'])

export function ApprovalSettings() {
  const { t } = useTranslation()
  const { toast } = useToast()
  const { isEnterprise } = useEdition()
  const { require_any_permission } = useCurrentUser()

  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)
  const [config, setConfig] = useState<ApprovalConfig | null>(null)

  const [enabled, setEnabled] = useState(false)
  const [ops, setOps] = useState<string[]>([])
  const [expiresHours, setExpiresHours] = useState('72')
  const [retentionDays, setRetentionDays] = useState('365')
  // `-1` is the "override disabled" sentinel, not a wait, so it is kept as a
  // separate flag rather than folded into the number. Folding it in is how the
  // UI would end up sending `-1` to a field the server reads as hours.
  const [breakGlassDisabled, setBreakGlassDisabled] = useState(false)
  const [breakGlassHours, setBreakGlassHours] = useState('1')

  const canManage =
    isEnterprise && require_any_permission(['system:root', 'user:manage'])

  useEffect(() => {
    if (!canManage) return
    let mounted = true
    get_approval_config()
      .then((cfg) => {
        if (!mounted) return
        setConfig(cfg)
        setEnabled(cfg.enabled)
        setOps(cfg.ops)
        setExpiresHours(String(cfg.expires_after_hours))
        setRetentionDays(String(cfg.retention_days))
        setBreakGlassDisabled(cfg.break_glass_min_hours === BREAK_GLASS_DISABLED)
        setBreakGlassHours(
          cfg.break_glass_min_hours === BREAK_GLASS_DISABLED
            ? '1'
            : String(cfg.break_glass_min_hours)
        )
      })
      .catch(() => {
        if (mounted) {
          toast({
            variant: 'destructive',
            title: t(
              'settings.approvals.loadFailed',
              'Failed to load dual-control configuration'
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
  }, [canManage, t, toast])

  const toggleOp = (op: string) =>
    setOps((prev) =>
      prev.includes(op) ? prev.filter((o) => o !== op) : [...prev, op]
    )

  const handleSave = async () => {
    setSaving(true)
    try {
      const saved = await update_approval_config({
        enabled,
        ops,
        // 0 means "use the server default", so an empty or invalid box is sent
        // as 0 rather than as a number the server would have to second-guess.
        expires_after_hours: Number(expiresHours) || 0,
        retention_days: Number(retentionDays) || 0,
        // The sentinel wins over the box: when the override is switched off,
        // whatever number is still showing is not a wait and must not be sent
        // as one.
        break_glass_min_hours: breakGlassDisabled
          ? BREAK_GLASS_DISABLED
          : Number(breakGlassHours) || 0,
      })
      setConfig(saved)
      setEnabled(saved.enabled)
      setOps(saved.ops)
      setExpiresHours(String(saved.expires_after_hours))
      setRetentionDays(String(saved.retention_days))
      setBreakGlassDisabled(
        saved.break_glass_min_hours === BREAK_GLASS_DISABLED
      )
      setBreakGlassHours(
        saved.break_glass_min_hours === BREAK_GLASS_DISABLED
          ? breakGlassHours
          : String(saved.break_glass_min_hours)
      )
      toast({
        title: t(
          'settings.approvals.saved',
          'Dual-control configuration saved'
        ),
      })
    } catch (err) {
      toast({
        variant: 'destructive',
        title: t(
          'settings.approvals.saveFailed',
          'Failed to save dual-control configuration'
        ),
        description: errorMessage(err),
      })
    } finally {
      setSaving(false)
    }
  }

  if (!canManage) {
    return (
      <div className='w-full p-6 text-muted-foreground'>
        {t(
          'settings.approvals.forbidden',
          'Dual control requires the Enterprise edition and system:root or user:manage permission.'
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
          {t('settings.approvals.title', 'Dual control')}
        </h2>
        <p className='text-sm text-muted-foreground'>
          {t(
            'settings.approvals.description',
            'Require two people for irreversible operations: one requests, a different person approves, and only then does it happen. Off by default.'
          )}
        </p>
      </div>

      <div className='space-y-6 rounded-lg border p-6'>
        <div className='flex items-center justify-between'>
          <div className='space-y-1'>
            <Label>{t('settings.approvals.enabled', 'Dual control enabled')}</Label>
            <p className='text-xs text-muted-foreground'>
              {t(
                'settings.approvals.enabledHint',
                'When off, every operation below behaves exactly as it did before. Nothing is queued.'
              )}
            </p>
          </div>
          <Switch checked={enabled} onCheckedChange={setEnabled} />
        </div>

        <div className='space-y-2'>
          <Label>
            {t('settings.approvals.operations', 'Operations requiring approval')}
          </Label>
          <div className='grid gap-3 sm:grid-cols-2'>
            {(config?.available_ops ?? []).map((op) => {
              const label = OP_LABELS[op]
              const destructive = DESTRUCTIVE_OPS.has(op)
              return (
                <label
                  key={op}
                  className='flex items-start gap-2 rounded-md border p-3'
                >
                  <Checkbox
                    checked={ops.includes(op)}
                    onCheckedChange={() => toggleOp(op)}
                    disabled={!enabled}
                  />
                  <div className='space-y-0.5'>
                    <p className='text-sm font-medium'>
                      {label ? t(label.key, label.fallback) : op}
                    </p>
                    <p className='font-mono text-xs text-muted-foreground'>
                      {op}
                    </p>
                    {destructive && (
                      <p className='text-xs text-amber-600 dark:text-amber-400'>
                        {t(
                          'settings.approvals.destructiveHint',
                          'Gating this makes every such deletion need a second person — including deleting a single message.'
                        )}
                      </p>
                    )}
                    {REVOCATION_OPS.has(op) && (
                      <p className='text-xs text-amber-600 dark:text-amber-400'>
                        {t(
                          'settings.approvals.revocationHint',
                          'Gating this means a token cannot be revoked until a second person approves — including a user revoking their own leaked token.'
                        )}
                      </p>
                    )}
                    {ERASURE_OPS.has(op) && !ops.includes(op) && (
                      <p className='text-xs text-amber-600 dark:text-amber-400'>
                        {t(
                          'settings.approvals.erasureHint',
                          'Not listed: an erasure request is still recorded, but it runs on the submitter’s signature alone — nobody else signs it.'
                        )}
                      </p>
                    )}
                  </div>
                </label>
              )
            })}
          </div>
          <p className='text-xs text-muted-foreground'>
            {t(
              'settings.approvals.operationsHint',
              'Only listed operations are gated. By default the requester cannot approve their own request — not even an administrator. The break-glass override below is the one exception.'
            )}
          </p>
        </div>

        <div className='grid gap-4 sm:grid-cols-2'>
          <div className='space-y-2'>
            <Label htmlFor='approval-expires'>
              {t(
                'settings.approvals.expiresAfterHours',
                'Requests expire after (hours)'
              )}
            </Label>
            <Input
              id='approval-expires'
              type='number'
              min={1}
              value={expiresHours}
              onChange={(e) => setExpiresHours(e.target.value)}
            />
            <p className='text-xs text-muted-foreground'>
              {t(
                'settings.approvals.expiresHint',
                'A stale request that is approved months later would silently unfreeze data. 0 uses the default of 72 hours.'
              )}
            </p>
          </div>
          <div className='space-y-2'>
            <Label htmlFor='approval-retention'>
              {t(
                'settings.approvals.retentionDays',
                'Keep decided requests for (days)'
              )}
            </Label>
            <Input
              id='approval-retention'
              type='number'
              min={1}
              value={retentionDays}
              onChange={(e) => setRetentionDays(e.target.value)}
            />
            <p className='text-xs text-muted-foreground'>
              {t(
                'settings.approvals.retentionHint',
                'The permanent record is the audit log; this only bounds the queue history. 0 uses the default of 365 days.'
              )}
            </p>
          </div>
        </div>

        <div className='space-y-4 rounded-md border p-4'>
          <div className='flex items-center justify-between'>
            <div className='space-y-1'>
              <Label>
                {t(
                  'settings.approvals.breakGlass',
                  'Break-glass override'
                )}
              </Label>
              <p className='text-xs text-muted-foreground'>
                {t(
                  'settings.approvals.breakGlassHint',
                  'What happens when the approver cannot be reached: after this wait, a system:root administrator may approve a request alone. The override is recorded as such in the audit log.'
                )}
              </p>
            </div>
            <div className='flex items-center gap-2'>
              <Label
                htmlFor='break-glass-disabled'
                className='text-xs text-muted-foreground'
              >
                {t('settings.approvals.breakGlassDisable', 'Disable')}
              </Label>
              <Switch
                id='break-glass-disabled'
                checked={breakGlassDisabled}
                onCheckedChange={setBreakGlassDisabled}
              />
            </div>
          </div>

          {breakGlassDisabled ? (
            <p className='text-xs text-amber-600 dark:text-amber-400'>
              {t(
                'settings.approvals.breakGlassDisabledHint',
                'The override is switched off. A request whose approver is unreachable can only be decided by a second person.'
              )}
            </p>
          ) : (
            <div className='space-y-2'>
              <Label htmlFor='break-glass-hours'>
                {t(
                  'settings.approvals.breakGlassMinHours',
                  'Override available after (hours pending)'
                )}
              </Label>
              <Input
                id='break-glass-hours'
                type='number'
                min={1}
                value={breakGlassHours}
                onChange={(e) => setBreakGlassHours(e.target.value)}
                className='max-w-40'
              />
              <p className='text-xs text-muted-foreground'>
                {t(
                  'settings.approvals.breakGlassMinHoursHint',
                  'The delay is the control, not an obstacle to it: it guarantees the normal two-person path had its chance first. 0 uses the default of 1 hour.'
                )}
              </p>
            </div>
          )}
        </div>

        <div className='flex items-center justify-end'>
          <Button type='button' onClick={handleSave} disabled={saving}>
            {saving && <Loader2 className='mr-2 h-4 w-4 animate-spin' />}
            {t('settings.approvals.save', 'Save configuration')}
          </Button>
        </div>
      </div>
    </div>
  )
}
