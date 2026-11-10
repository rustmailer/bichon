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
// The dual-control approval queue (Enterprise).
//
// Moved here from the legal-hold console when the gate grew from one operation
// to five. The queue is now the whole of the two-person rule's second half —
// requests for holds, users, accounts, mailboxes and messages all land in it —
// so it has its own page rather than living inside one operation's console.
import { useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Check, Clock, Loader2, ShieldCheck, TriangleAlert, X } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import {
  approve_request,
  break_glass_request,
  is_outcome_item,
  list_approvals,
  OP_GDPR_ERASURE,
  reject_request,
  type ApprovalRequest,
  type ApprovalResultItem,
} from '@/api/approvals/api'
import { useCurrentUser } from '@/hooks/use-current-user'
import { toast } from '@/hooks/use-toast'
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
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import { Textarea } from '@/components/ui/textarea'
import { TableSkeleton } from '@/components/table-skeleton'
import { OP_LABELS } from './op-labels'
import { TargetDescription } from './target-description'

interface ApiErrorLike {
  response?: { data?: { message?: string } }
  message?: string
}

const errorMessage = (e: unknown): string =>
  (e as ApiErrorLike).response?.data?.message ??
  (e as Error).message ??
  'Unknown error'

function formatTime(ts: number): string {
  const d = new Date(ts)
  const pad = (n: number) => String(n).padStart(2, '0')
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`
}

/** How long until a request expires, as a short human string. */
function formatRemaining(expires_at: number, now: number): string {
  const ms = expires_at - now
  if (ms <= 0) return '0m'
  const hours = Math.floor(ms / 3_600_000)
  const minutes = Math.floor((ms % 3_600_000) / 60_000)
  return hours > 0 ? `${hours}h ${minutes}m` : `${minutes}m`
}

/** A one-line description of a per-item result, for the failure list.
 *
 *  The two result shapes are different on the wire — `ReleaseItem` has no
 *  discriminator, the general shape does — so this narrows rather than
 *  guessing at fields that may not exist. */
function describe_result_item(item: ApprovalResultItem): string {
  if (is_outcome_item(item)) {
    switch (item.kind) {
      case 'user':
        return `${item.username || `#${item.user_id}`}: ${item.error ?? 'unknown'}`
      case 'account':
        return `${item.email || `#${item.account_id}`}: ${item.error ?? 'unknown'}`
      case 'mailbox':
        return `mailbox #${item.mailbox_id}: ${item.error ?? 'unknown'}`
      case 'messages':
        return `account #${item.account_id}: ${item.error ?? 'unknown'}`
      case 'token':
        return `${item.username || `#${item.user_id}`}: ${item.error ?? 'unknown'}`
      case 'export':
        return `${item.saved_search_name || item.export_id || 'export'}: ${
          item.error ?? 'unknown'
        }`
      case 'erasure':
        return `${item.subject || `#${item.account_id}`}: ${
          item.error ?? 'unknown'
        }`
      case 'retention':
        return `account #${item.account_id}: ${item.error ?? 'unknown'}`
    }
  }
  return `${item.email || `#${item.account_id}`}: ${item.error ?? 'unknown'}`
}

/** Whether an item is a failure. Both result shapes carry `ok`, so no
 *  narrowing is needed here — only for reading the per-shape detail fields. */
const item_failed = (item: ApprovalResultItem): boolean => !item.ok

export default function ApprovalsPage() {
  const { t } = useTranslation()
  const queryClient = useQueryClient()
  const { require_any_permission } = useCurrentUser()

  // A compliance officer without `approval:decide` may still read the queue:
  // the separation-of-duties permission grants sight of it, never the power to
  // move it.
  const canView = require_any_permission([
    'approval:decide',
    'compliance:audit',
    // The built-in administrator holds system:root, and both the sidebar and
    // the server's read gate accept it. Omitting it here showed the admin the
    // entry and then rendered the forbidden state on the page.
    'system:root',
  ])
  const canDecide = require_any_permission(['approval:decide'])
  // The break-glass override is narrower than deciding: the server requires
  // `system:root` for it, and only a root admin may waive the second
  // signature. A compliance officer holding `approval:decide` must not see the
  // button at all — showing it and letting the server refuse would suggest the
  // power is theirs to take.
  const canBreakGlass = require_any_permission(['system:root'])

  const [approving, setApproving] = useState<ApprovalRequest | null>(null)
  const [rejecting, setRejecting] = useState<ApprovalRequest | null>(null)
  const [breakingGlass, setBreakingGlass] = useState<ApprovalRequest | null>(
    null
  )
  const [note, setNote] = useState('')

  const { data: pending, isLoading } = useQuery({
    queryKey: ['approvals', 'pending'],
    queryFn: () => list_approvals('pending'),
    enabled: canView,
    // A background operation moves from `executing` to `executed` without the
    // approver doing anything, so the queue is polled while it is open. Without
    // this the page would sit on a stale "in progress" forever.
    refetchInterval: 5_000,
  })

  const settle = () => {
    queryClient.invalidateQueries({ queryKey: ['approvals'] })
    queryClient.invalidateQueries({ queryKey: ['legal-hold'] })
    queryClient.invalidateQueries({ queryKey: ['account-list'] })
    setApproving(null)
    setRejecting(null)
    setBreakingGlass(null)
    setNote('')
  }

  /** Report what an approval actually did.
   *
   *  Shared by the normal path and the break-glass override rather than
   *  duplicated: the two produce the same result shapes, and a second copy
   *  would drift — most likely on the `executing` case, where reading the
   *  absent result as an empty one fabricates a "0 processed" outcome for work
   *  that has not started.
   *
   *  The status decides what can honestly be said, and it must be checked
   *  BEFORE looking at `result.result`. A background operation comes back as
   *  `executing` with no result at all.
   *
   *  `viaBreakGlass` selects a whole parallel set of sentences rather than
   *  prefixing the normal ones. Composing "prefix — suffix" out of translated
   *  fragments reads acceptably in English and badly in most of the other 17
   *  locales: the em-dash join and the clause order do not survive Japanese,
   *  Korean, or Arabic. A translator needs a complete sentence per case. */
  const report_approval_outcome = (
    result: ApprovalRequest,
    viaBreakGlass = false
  ) => {
    if (result.status === 'executing' || result.status === 'approved') {
      toast({
        title: t(
          viaBreakGlass
            ? 'approvals.breakGlassApprovedRunning'
            : 'approvals.approvedRunning',
          viaBreakGlass
            ? 'Approved alone (break-glass) — the operation is now running'
            : 'Request approved — the operation is now running'
        ),
        description: t(
          'approvals.approvedRunningDesc',
          'It runs in the background and may take a while. The outcome will appear in this queue; nothing has finished yet.'
        ),
      })
      settle()
      return
    }

    if (result.status === 'interrupted') {
      toast({
        variant: 'destructive',
        title: t(
          viaBreakGlass
            ? 'approvals.breakGlassApprovedInterrupted'
            : 'approvals.approvedInterrupted',
          viaBreakGlass
            ? 'Approved alone (break-glass) — but the execution was interrupted'
            : 'Request approved — but the execution was interrupted'
        ),
        description: t(
          'approvals.approvedInterruptedDesc',
          'The operation was cut short and its target may be partially modified. Inspect it and issue a fresh request if needed.'
        ),
      })
      settle()
      return
    }

    const items = result.result ?? []
    const failed = items.filter(item_failed)
    if (failed.length === 0) {
      toast({
        title: t(
          viaBreakGlass
            ? 'approvals.breakGlassApproved'
            : 'approvals.approved',
          viaBreakGlass
            ? 'Approved alone (break-glass) — {{count}} item(s) processed'
            : 'Request approved — {{count}} item(s) processed',
          { count: items.length }
        ),
      })
    } else {
      // Approval is about the decision; individual targets can still fail (a
      // hold was placed, the target is gone). Say so rather than reporting a
      // clean success.
      toast({
        title: t(
          viaBreakGlass
            ? 'approvals.breakGlassApprovedPartial'
            : 'approvals.approvedPartial',
          viaBreakGlass
            ? 'Approved alone (break-glass) — succeeded for {{ok}} of {{total}}'
            : 'Request approved — succeeded for {{ok}} of {{total}}',
          { ok: items.length - failed.length, total: items.length }
        ),
        description: failed.map(describe_result_item).join('; '),
        variant: 'destructive',
      })
    }
    settle()
  }

  const approveMutation = useMutation({
    mutationFn: (vars: { id: string; note?: string }) =>
      approve_request(vars.id, vars.note),
    onSuccess: (result) => report_approval_outcome(result),
    onError: (e) => {
      toast({
        title: t('approvals.approveFailed', 'Failed to approve request'),
        description: errorMessage(e),
        variant: 'destructive',
      })
    },
  })

  const breakGlassMutation = useMutation({
    mutationFn: (vars: { id: string; note?: string }) =>
      break_glass_request(vars.id, vars.note),
    onSuccess: (result) => report_approval_outcome(result, true),
    onError: (e) => {
      // The server refuses with 403 both when the wait has not elapsed and when
      // the override is disabled, and its message says which — so it is shown
      // verbatim rather than replaced with a generic failure.
      toast({
        title: t(
          'approvals.breakGlassFailed',
          'Failed to apply the break-glass override'
        ),
        description: errorMessage(e),
        variant: 'destructive',
      })
    },
  })

  const rejectMutation = useMutation({
    mutationFn: (vars: { id: string; note: string }) =>
      reject_request(vars.id, vars.note),
    onSuccess: () => {
      toast({ title: t('approvals.rejected', 'Request rejected') })
      settle()
    },
    onError: (e) => {
      toast({
        title: t('approvals.rejectFailed', 'Failed to reject request'),
        description: errorMessage(e),
        variant: 'destructive',
      })
    },
  })

  if (!canView) {
    return (
      <div className='w-full p-6 text-muted-foreground'>
        {t(
          'approvals.forbidden',
          'The approval queue requires the Enterprise edition and the approval:decide or compliance:audit permission.'
        )}
      </div>
    )
  }

  const rows = pending ?? []
  const busy =
    approveMutation.isPending ||
    rejectMutation.isPending ||
    breakGlassMutation.isPending
  const now = Date.now()

  return (
    <div className='w-full space-y-6 px-4'>
      <div className='space-y-2'>
        <h2 className='flex items-center gap-2 text-xl font-bold'>
          <ShieldCheck className='h-5 w-5 text-primary' />
          {t('approvals.title', 'Approvals awaiting a second person')}
          {rows.length > 0 && <Badge variant='secondary'>{rows.length}</Badge>}
        </h2>
        <p className='text-sm text-muted-foreground'>
          {t(
            'approvals.description',
            'Dual control is on: these operations do not take effect until someone other than the requester approves them. Nothing has happened to their targets yet.'
          )}
        </p>
      </div>

      <Card>
        <CardHeader>
          <CardTitle>{t('approvals.pendingTitle', 'Pending requests')}</CardTitle>
          <CardDescription>
            {t(
              'approvals.pendingHint',
              'The requester cannot approve their own request — not even an administrator. The break-glass override, available to a system:root administrator after the configured wait, is the one exception.'
            )}
          </CardDescription>
        </CardHeader>
        <CardContent>
          {isLoading && !pending ? (
            <TableSkeleton rows={3} />
          ) : rows.length === 0 ? (
            <div className='flex items-center justify-center gap-2 rounded border p-6 text-muted-foreground'>
              <Check className='h-4 w-4' />
              {t('approvals.noPending', 'Nothing is awaiting approval.')}
            </div>
          ) : (
            <Table className='text-xs'>
              <TableHeader>
                <TableRow>
                  <TableHead>{t('approvals.operation', 'Operation')}</TableHead>
                  <TableHead>{t('approvals.target', 'Target')}</TableHead>
                  <TableHead>{t('approvals.reason', 'Reason')}</TableHead>
                  <TableHead>
                    {t('approvals.requestedBy', 'Requested by')}
                  </TableHead>
                  <TableHead>{t('approvals.expires', 'Expires in')}</TableHead>
                  {canDecide && (
                    <TableHead className='text-right'>
                      {t('approvals.actions', 'Actions')}
                    </TableHead>
                  )}
                </TableRow>
              </TableHeader>
              <TableBody>
                {rows.map((req) => (
                  <TableRow key={req.id}>
                    <TableCell className='whitespace-nowrap font-medium'>
                      {OP_LABELS[req.op]
                        ? t(OP_LABELS[req.op].key, OP_LABELS[req.op].fallback)
                        : req.op}
                    </TableCell>
                    <TableCell className='max-w-xs'>
                      <TargetDescription target={req.target} />
                    </TableCell>
                    <TableCell className='max-w-md'>
                      <span className='line-clamp-2 text-muted-foreground'>
                        {req.reason || '—'}
                      </span>
                    </TableCell>
                    <TableCell>
                      <div>{req.requested_by_name}</div>
                      <div className='text-muted-foreground'>
                        {formatTime(req.requested_at)}
                      </div>
                    </TableCell>
                    <TableCell>
                      {req.expires_at ? (
                        <span className='flex items-center gap-1 text-muted-foreground'>
                          <Clock className='h-3 w-3' />
                          {formatRemaining(req.expires_at, now)}
                        </span>
                      ) : (
                        '—'
                      )}
                    </TableCell>
                    {canDecide && (
                      <TableCell className='text-right'>
                        <div className='flex justify-end gap-2'>
                          <Button
                            variant='outline'
                            size='sm'
                            disabled={busy}
                            onClick={() => setRejecting(req)}
                          >
                            <X className='mr-1 h-3.5 w-3.5' />
                            {t('approvals.reject', 'Reject')}
                          </Button>
                          <Button
                            size='sm'
                            disabled={busy}
                            onClick={() => setApproving(req)}
                          >
                            {approveMutation.isPending ? (
                              <Loader2 className='mr-1 h-3.5 w-3.5 animate-spin' />
                            ) : (
                              <Check className='mr-1 h-3.5 w-3.5' />
                            )}
                            {t('approvals.approve', 'Approve')}
                          </Button>
                          {canBreakGlass && (
                            <Button
                              variant='destructive'
                              size='sm'
                              disabled={busy}
                              title={t(
                                'approvals.breakGlassHint',
                                'Approve alone, without a second person. Only available after the configured wait, and recorded as a break-glass override.'
                              )}
                              onClick={() => setBreakingGlass(req)}
                            >
                              <TriangleAlert className='mr-1 h-3.5 w-3.5' />
                              {t('approvals.breakGlass', 'Break-glass')}
                            </Button>
                          )}
                        </div>
                      </TableCell>
                    )}
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          )}
        </CardContent>
      </Card>

      {/* ---- Approve ---- */}
      <AlertDialog
        open={approving !== null}
        onOpenChange={(open) => !open && setApproving(null)}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              {approving && OP_LABELS[approving.op]
                ? t('approvals.approveTitle', 'Approve "{{op}}"?', {
                    op: t(
                      OP_LABELS[approving.op].key,
                      OP_LABELS[approving.op].fallback
                    ),
                  })
                : t('approvals.approveTitle', 'Approve this request?')}
            </AlertDialogTitle>
            <AlertDialogDescription>
              {t(
                'approvals.approveDesc',
                'The operation runs immediately and is irreversible. It is recorded against your name.'
              )}
            </AlertDialogDescription>
          </AlertDialogHeader>
          {approving && (
            <div className='rounded-md border bg-muted/40 p-3'>
              <TargetDescription target={approving.target} />
            </div>
          )}
          <div className='space-y-2'>
            <Label className='text-xs'>
              {approving?.op === OP_GDPR_ERASURE
                ? t(
                    'approvals.approveNoteRequired',
                    'Note (required — state the basis)'
                  )
                : t('approvals.approveNote', 'Note (optional)')}
            </Label>
            <Textarea
              value={note}
              onChange={(e) => setNote(e.target.value)}
              placeholder={t(
                'approvals.approveNotePlaceholder',
                'e.g. Confirmed with legal'
              )}
              rows={2}
            />
            {/* The server refuses an unexplained erasure decision. Saying so
                here is the difference between "the button is broken" and "I
                have to write down why". */}
            {approving?.op === OP_GDPR_ERASURE && (
              <p className='text-xs text-muted-foreground'>
                {t(
                  'approvals.approveNoteRequiredHint',
                  'This is recorded as the basis on which the erasure is lawful. It cannot be left empty.'
                )}
              </p>
            )}
          </div>
          <AlertDialogFooter>
            <AlertDialogCancel>{t('common.cancel')}</AlertDialogCancel>
            <AlertDialogAction
              disabled={
                approveMutation.isPending ||
                (approving?.op === OP_GDPR_ERASURE && !note.trim())
              }
              onClick={() =>
                approving &&
                approveMutation.mutate({
                  id: approving.id,
                  note: note.trim() || undefined,
                })
              }
            >
              <Check className='mr-1 h-4 w-4' />
              {approveMutation.isPending
                ? t('approvals.approving', 'Approving…')
                : t('approvals.confirmApprove', 'Approve and execute')}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      {/* ---- Reject ---- */}
      <AlertDialog
        open={rejecting !== null}
        onOpenChange={(open) => !open && setRejecting(null)}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              {t('approvals.rejectTitle', 'Reject this request?')}
            </AlertDialogTitle>
            <AlertDialogDescription>
              {t(
                'approvals.rejectDesc',
                'Nothing happens to the target and the requester is told why. A reason is required — whoever has to act next needs to know what to fix.'
              )}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <div className='space-y-2'>
            <Label className='text-xs'>
              {t('approvals.rejectNote', 'Reason')}
              <span className='align-super text-xs text-red-500'>*</span>
            </Label>
            <Textarea
              value={note}
              onChange={(e) => setNote(e.target.value)}
              placeholder={t(
                'approvals.rejectNotePlaceholder',
                'e.g. This data is still required for case #2026-0417'
              )}
              rows={2}
            />
          </div>
          <AlertDialogFooter>
            <AlertDialogCancel>{t('common.cancel')}</AlertDialogCancel>
            <AlertDialogAction
              className='bg-destructive text-destructive-foreground'
              disabled={rejectMutation.isPending || !note.trim()}
              onClick={() =>
                rejecting &&
                rejectMutation.mutate({ id: rejecting.id, note: note.trim() })
              }
            >
              <X className='mr-1 h-4 w-4' />
              {rejectMutation.isPending
                ? t('approvals.rejecting', 'Rejecting…')
                : t('approvals.confirmReject', 'Reject request')}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      {/* ---- Break-glass ---- */}
      <AlertDialog
        open={breakingGlass !== null}
        onOpenChange={(open) => !open && setBreakingGlass(null)}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              {t(
                'approvals.breakGlassTitle',
                'Approve alone, without a second person?'
              )}
            </AlertDialogTitle>
            <AlertDialogDescription>
              {t(
                'approvals.breakGlassDesc',
                'This waives the second signature. The operation runs immediately and is irreversible, and the audit log records it as a break-glass override — one person, acting alone. If the approver can be reached, use the normal path instead.'
              )}
            </AlertDialogDescription>
          </AlertDialogHeader>
          {breakingGlass && (
            <div className='rounded-md border bg-muted/40 p-3'>
              <TargetDescription target={breakingGlass.target} />
            </div>
          )}
          <div className='space-y-2'>
            <Label className='text-xs'>
              {breakingGlass?.op === OP_GDPR_ERASURE
                ? t(
                    'approvals.approveNoteRequired',
                    'Note (required — state the basis)'
                  )
                : t(
                    'approvals.breakGlassNote',
                    'Reason (optional, but recorded)'
                  )}
            </Label>
            <Textarea
              value={note}
              onChange={(e) => setNote(e.target.value)}
              placeholder={t(
                'approvals.breakGlassNotePlaceholder',
                'e.g. Approver unreachable, release required by court deadline'
              )}
              rows={2}
            />
          </div>
          <AlertDialogFooter>
            <AlertDialogCancel>{t('common.cancel')}</AlertDialogCancel>
            <AlertDialogAction
              className='bg-destructive text-destructive-foreground'
              disabled={
                breakGlassMutation.isPending ||
                (breakingGlass?.op === OP_GDPR_ERASURE && !note.trim())
              }
              onClick={() =>
                breakingGlass &&
                breakGlassMutation.mutate({
                  id: breakingGlass.id,
                  note: note.trim() || undefined,
                })
              }
            >
              <TriangleAlert className='mr-1 h-4 w-4' />
              {breakGlassMutation.isPending
                ? t('approvals.breakingGlass', 'Overriding…')
                : t('approvals.confirmBreakGlass', 'Override and execute')}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  )
}

/** A banner shown on the legal-hold console while dual control is on. */
export function DualControlNotice() {
  const { t } = useTranslation()
  return (
    <div className='flex items-start gap-2 rounded-md border border-amber-500/40 bg-amber-500/10 p-3 text-xs text-amber-700 dark:text-amber-400'>
      <TriangleAlert className='mt-0.5 h-4 w-4 shrink-0' />
      {t(
        'approvals.legalHoldNotice',
        'Dual control is enabled: releasing a hold now requires a second person. Requests appear on the Approvals page.'
      )}
    </div>
  )
}
