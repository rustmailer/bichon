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
// Legal hold console (Enterprise). Accounts under a legal hold are frozen: the
// retention sweep skips them and bulk deletion refuses to purge them, so the
// data is preserved for litigation / regulatory holds. Placing and releasing a
// hold requires the global `legal:hold` permission.
import { useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Clock, Lock, LockOpen, Plus, Scale, Unlock } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { minimal_account_list } from '@/api/account/api'
import { list_pending_release_targets } from '@/api/approvals/api'
import {
  list_legal_holds,
  place_legal_holds_batch,
  release_legal_holds_batch,
  release_legal_hold,
  release_legal_hold_case,
  type BatchHoldResult,
  type HoldAccount,
} from '@/api/legal-hold/api'
import { cn } from '@/lib/utils'
import { useCurrentUser } from '@/hooks/use-current-user'
import { useEdition } from '@/hooks/use-edition'
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
import { Checkbox } from '@/components/ui/checkbox'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Label } from '@/components/ui/label'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import { Input } from '@/components/ui/input'
import { Textarea } from '@/components/ui/textarea'
import { FixedHeader } from '@/components/layout/fixed-header'
import { Main } from '@/components/layout/main'
import { TableSkeleton } from '@/components/table-skeleton'
import { DualControlNotice } from '@/features/approvals'

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

/** Toggle one id in a selection list. */
const toggleId = (
  list: number[],
  setter: (next: number[]) => void,
  id: number
): void =>
  setter(list.includes(id) ? list.filter((x) => x !== id) : [...list, id])

function formatTime(ts: number): string {
  const d = new Date(ts)
  const pad = (n: number) => String(n).padStart(2, '0')
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`
}

export default function LegalHoldPage() {
  const { t } = useTranslation()
  const queryClient = useQueryClient()
  const { isEnterprise, approvalEnabled } = useEdition()
  const { require_any_permission } = useCurrentUser()

  // ---- gating ---------------------------------------------------------
  // Read access includes compliance officers (`compliance:audit`,
  // separation of duties); placing/releasing stays manage-only. An approver
  // holding only `approval:decide` must be able to reach the page too, or the
  // queue they are supposed to work would be invisible to them.
  const canView =
    isEnterprise &&
    require_any_permission([
      'legal:hold',
      'system:root',
      'compliance:audit',
      'approval:decide',
    ])
  const canManage =
    isEnterprise && require_any_permission(['legal:hold', 'system:root'])

  // ---- data -----------------------------------------------------------
  const { data: holds, isLoading } = useQuery({
    queryKey: ['legal-hold'],
    queryFn: list_legal_holds,
    enabled: canView,
  })

  // Accounts with a release already in the queue. Without this the operator
  // would queue the same release again every time they looked at the page.
  const { data: pendingReleaseTargets } = useQuery({
    queryKey: ['approvals', 'pending-accounts'],
    queryFn: list_pending_release_targets,
    enabled: canView && approvalEnabled,
  })
  const queuedAccounts = new Set(pendingReleaseTargets?.account_ids ?? [])
  const accountReleaseQueued = new Set(
    pendingReleaseTargets?.account_release_ids ?? []
  )
  const caseReleaseQueued = new Set(pendingReleaseTargets?.hold_ids ?? [])
  /** Whether *this case* is already spoken for. An account-level release
   *  blocks every case on the account; a case-level release blocks only the
   *  case it names, so another matter stays actionable. */
  const isQueued = (hold: HoldAccount): boolean =>
    accountReleaseQueued.has(hold.id) ||
    (hold.hold_id
      ? caseReleaseQueued.has(hold.hold_id)
      : queuedAccounts.has(hold.id))

  const { data: accounts } = useQuery({
    queryKey: ['legal-hold-accounts'],
    queryFn: minimal_account_list,
    enabled: canManage,
    staleTime: 60_000,
  })

  const heldIds = new Set((holds ?? []).map((h) => h.id))
  const placeable = (accounts ?? []).filter((a) => !heldIds.has(a.id))

  // ---- place hold (batch) --------------------------------------------
  const [placeOpen, setPlaceOpen] = useState(false)
  const [selectedIds, setSelectedIds] = useState<number[]>([])
  const [placeReason, setPlaceReason] = useState('')
  const [placeCaseRef, setPlaceCaseRef] = useState('')
  const [releasing, setReleasing] = useState<HoldAccount | null>(null)
  const [releaseReason, setReleaseReason] = useState('')

  // ---- batch release ---------------------------------------------------
  const [releaseBatchOpen, setReleaseBatchOpen] = useState(false)
  const [heldSelected, setHeldSelected] = useState<number[]>([])
  const [releaseBatchReason, setReleaseBatchReason] = useState('')

  const allPlaceableSelected =
    placeable.length > 0 && placeable.every((a) => selectedIds.includes(a.id))
  const toggleAllPlaceable = () =>
    setSelectedIds(allPlaceableSelected ? [] : placeable.map((a) => a.id))

  const allHeldSelected =
    (holds ?? []).length > 0 &&
    (holds ?? []).every((h) => heldSelected.includes(h.id))
  const toggleAllHeld = () =>
    setHeldSelected(
      allHeldSelected ? [] : (holds ?? []).map((h) => h.id)
    )

  /** Toast summarizing a batch result list (which accounts failed and why). */
  const batchToast = (
    okKey: string,
    okFallback: string,
    partialKey: string,
    partialFallback: string,
    results: BatchHoldResult[]
  ) => {
    const failed = results.filter((r) => !r.ok)
    if (failed.length === 0) {
      toast({
        title: t(okKey, okFallback, { count: results.length }),
      })
    } else {
      toast({
        title: t(partialKey, partialFallback, {
          ok: results.length - failed.length,
          total: results.length,
        }),
        description: failed
          .map((r) => `${r.email || r.account_id}: ${r.error ?? 'unknown'}`)
          .join('; '),
        variant: 'destructive',
      })
    }
  }

  /**
   * Toast for a batch release. Three buckets, not two: with dual control on, an
   * account can be accepted-and-queued rather than released, and calling that
   * "released" would tell the operator a hold is gone when it is not. `pending`
   * is checked before `ok` because a queued row carries both.
   */
  const batchReleaseToast = (results: BatchHoldResult[]) => {
    const queued = results.filter((r) => r.pending)
    const released = results.filter((r) => r.ok && !r.pending)
    const failed = results.filter((r) => !r.ok && !r.pending)

    if (queued.length > 0 && released.length === 0 && failed.length === 0) {
      toast({
        title: t(
          'legalHold.batchQueued',
          '{{count}} release request(s) submitted for approval',
          { count: queued.length }
        ),
        description: t(
          'legalHold.batchQueuedHint',
          'Nothing has been released yet — a second person must approve the request.'
        ),
      })
      return
    }

    const parts: string[] = []
    if (released.length > 0) {
      parts.push(
        t('legalHold.batchReleased', 'Released {{count}} account(s)', {
          count: released.length,
        })
      )
    }
    if (queued.length > 0) {
      parts.push(
        t(
          'legalHold.batchQueuedCount',
          '{{count}} submitted for approval',
          { count: queued.length }
        )
      )
    }
    if (failed.length > 0) {
      parts.push(
        t('legalHold.batchFailedCount', '{{count}} failed', {
          count: failed.length,
        })
      )
    }

    toast({
      title: parts.join(' · '),
      description:
        failed.length > 0
          ? failed
              .map((r) => `${r.email || r.account_id}: ${r.error ?? 'unknown'}`)
              .join('; ')
          : undefined,
      variant: failed.length > 0 ? 'destructive' : undefined,
    })
  }

  const placeMutation = useMutation({
    mutationFn: (vars: {
      account_ids: number[]
      reason: string
      case_ref?: string
    }) =>
      place_legal_holds_batch(vars.account_ids, vars.reason, vars.case_ref),
    onSuccess: (results) => {
      batchToast(
        'legalHold.batchPlaced',
        'Legal hold placed on {{count}} account(s)',
        'legalHold.batchPlacedPartial',
        'Placed {{ok}} of {{total}} holds',
        results
      )
      queryClient.invalidateQueries({ queryKey: ['legal-hold'] })
      queryClient.invalidateQueries({ queryKey: ['account-list'] })
      setPlaceOpen(false)
      setSelectedIds([])
      setPlaceReason('')
      setPlaceCaseRef('')
    },
    onError: (e) => {
      toast({
        title: t('legalHold.placeFailed', 'Failed to place legal hold'),
        description: errorMessage(e),
        variant: 'destructive',
      })
    },
  })

  const releaseMutation = useMutation({
    // The per-row action releases the case that row shows. Accounts with two
    // matters keep the other one active. Older rows without a case id fall
    // back to the account-level release.
    mutationFn: (vars: { hold_id?: string; account_id: number; reason?: string }) =>
      vars.hold_id
        ? release_legal_hold_case(vars.hold_id, vars.reason)
        : release_legal_hold(vars.account_id, vars.reason),
    onSuccess: (result) => {
      if (result.pending || result.status === 'pending') {
        // Queued, not released. The account is still on hold and the operator
        // must not walk away believing otherwise.
        toast({
          title: t(
            'legalHold.releaseQueued',
            'Release submitted for approval'
          ),
          description: t(
            'legalHold.releaseQueuedHint',
            'The hold is still in place. A second person must approve the request before the account is released.'
          ),
        })
        // The hold itself did not change, but the pending badges did.
        queryClient.invalidateQueries({ queryKey: ['approvals'] })
      } else {
        toast({ title: t('legalHold.released', 'Legal hold released') })
        queryClient.invalidateQueries({ queryKey: ['legal-hold'] })
        queryClient.invalidateQueries({ queryKey: ['account-list'] })
      }
      setReleasing(null)
      setReleaseReason('')
    },
    onError: (e) => {
      toast({
        title: t('legalHold.releaseFailed', 'Failed to release legal hold'),
        description: errorMessage(e),
        variant: 'destructive',
      })
    },
  })

  const releaseBatchMutation = useMutation({
    mutationFn: (vars: { account_ids: number[]; reason?: string }) =>
      release_legal_holds_batch(vars.account_ids, vars.reason),
    onSuccess: (results) => {
      batchReleaseToast(results)
      queryClient.invalidateQueries({ queryKey: ['legal-hold'] })
      queryClient.invalidateQueries({ queryKey: ['account-list'] })
      queryClient.invalidateQueries({ queryKey: ['approvals'] })
      setReleaseBatchOpen(false)
      setHeldSelected([])
      setReleaseBatchReason('')
    },
    onError: (e) => {
      toast({
        title: t('legalHold.releaseFailed', 'Failed to release legal hold'),
        description: errorMessage(e),
        variant: 'destructive',
      })
    },
  })

  if (!canView) {
    return (
      <>
        <FixedHeader />
        <Main>
          <div className='mx-auto w-full max-w-7xl px-4 py-16 text-center text-muted-foreground'>
            {t(
              'legalHold.forbidden',
              'Legal Hold is available in the Enterprise edition with the legal:hold permission.'
            )}
          </div>
        </Main>
      </>
    )
  }

  return (
    <>
      <FixedHeader />
      <Main>
        <div className='mx-auto w-full max-w-7xl space-y-6 text-xs'>
          <div className='flex flex-wrap items-start justify-between gap-4'>
            <div>
              <h1 className='flex items-center gap-2 text-lg font-semibold'>
                <Scale className='h-5 w-5 text-primary' />
                {t('legalHold.title', 'Legal hold')}
              </h1>
              <p className='text-xs text-muted-foreground'>
                {t(
                  'legalHold.subtitle',
                  'Freeze accounts so retention and bulk deletion never purge their messages. Placing a hold suspends the retention sweep for that account; releasing it resumes normal retention.'
                )}
              </p>
            </div>
            <div className='flex items-center gap-2'>
              {canManage && heldSelected.length > 0 && (
                <Button
                  variant='outline'
                  onClick={() => setReleaseBatchOpen(true)}
                >
                  <LockOpen className='mr-1 h-4 w-4' />
                  {t('legalHold.releaseSelected', 'Release selected', {
                    count: heldSelected.length,
                  })}
                </Button>
              )}
              {canManage && (
                <Button
                  onClick={() => setPlaceOpen(true)}
                  disabled={placeable.length === 0}
                >
                  <Plus className='mr-1 h-4 w-4' />
                  {t('legalHold.place', 'Place hold')}
                </Button>
              )}
            </div>
          </div>

          <Card>
            <CardHeader>
              <CardTitle className='flex items-center gap-2'>
                <Lock className='h-4 w-4 text-amber-500' />
                {t('legalHold.heldAccounts', 'Accounts under legal hold')}
              </CardTitle>
              <CardDescription>
                {t(
                  'legalHold.heldAccountsHint',
                  'These accounts are excluded from automatic retention cleanup until the hold is released.'
                )}
              </CardDescription>
            </CardHeader>
            <CardContent>
              {isLoading && !holds ? (
                <TableSkeleton rows={5} />
              ) : (holds ?? []).length === 0 ? (
                <div className='flex items-center gap-2 rounded border p-6 text-center text-muted-foreground'>
                  <Unlock className='h-4 w-4' />
                  {t(
                    'legalHold.noHolds',
                    'No accounts are currently under a legal hold.'
                  )}
                </div>
              ) : (
                <Table className='text-xs'>
                  <TableHeader>
                    <TableRow>
                      {canManage && (
                        <TableHead className='w-8'>
                          <Checkbox
                            checked={allHeldSelected}
                            onCheckedChange={toggleAllHeld}
                            aria-label={t(
                              'legalHold.selectAll',
                              'Select all accounts'
                            )}
                          />
                        </TableHead>
                      )}
                      <TableHead>{t('legalHold.account', 'Account')}</TableHead>
                      <TableHead>{t('legalHold.caseRef', 'Case')}</TableHead>
                      <TableHead>{t('legalHold.reason', 'Reason')}</TableHead>
                      <TableHead>
                        {t('legalHold.placedBy', 'Placed by')}
                      </TableHead>
                      <TableHead>
                        {t('legalHold.placedAt', 'Placed at')}
                      </TableHead>
                      {canManage && (
                        <TableHead className='text-right'>
                          {t('legalHold.actions', 'Actions')}
                        </TableHead>
                      )}
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    {(holds ?? []).map((hold) => (
                      <TableRow key={hold.hold_id ?? hold.id}>
                        {canManage && (
                          <TableCell className='w-8'>
                            <Checkbox
                              checked={heldSelected.includes(hold.id)}
                              onCheckedChange={() =>
                                toggleId(heldSelected, setHeldSelected, hold.id)
                              }
                              aria-label={hold.email}
                            />
                          </TableCell>
                        )}
                        <TableCell className='font-medium'>
                          <div className='flex items-center gap-2'>
                            <Badge
                              variant='outline'
                              className='border-amber-500/40 bg-amber-500/10 text-amber-600'
                            >
                              <Lock className='mr-1 h-3 w-3' />
                              {t('legalHold.badge', 'Held')}
                            </Badge>
                            {queuedAccounts.has(hold.id) && (
                              <Badge
                                variant='outline'
                                className='border-sky-500/40 bg-sky-500/10 text-sky-600'
                              >
                                <Clock className='mr-1 h-3 w-3' />
                                {t(
                                  'legalHold.awaitingApproval',
                                  'Awaiting approval'
                                )}
                              </Badge>
                            )}
                            <span>{hold.email}</span>
                          </div>
                        </TableCell>
                        <TableCell className='font-mono text-xs'>
                          {hold.case_ref || '—'}
                        </TableCell>
                        <TableCell className='max-w-md'>
                          <span className='line-clamp-2 text-muted-foreground'>
                            {hold.reason || '—'}
                          </span>
                        </TableCell>
                        <TableCell>
                          {hold.placed_by_name ??
                            (hold.placed_by ? String(hold.placed_by) : '—')}
                        </TableCell>
                        <TableCell>
                          {hold.placed_at ? formatTime(hold.placed_at) : '—'}
                        </TableCell>
                        {canManage && (
                          <TableCell className='text-right'>
                            <Button
                              variant='outline'
                              size='sm'
                              disabled={isQueued(hold)}
                              title={
                                isQueued(hold)
                                  ? t(
                                      'legalHold.alreadyQueued',
                                      'A release request for this account is already awaiting approval.'
                                    )
                                  : undefined
                              }
                              onClick={() => setReleasing(hold)}
                            >
                              <LockOpen className='mr-1 h-3.5 w-3.5' />
                              {t('legalHold.release', 'Release')}
                            </Button>
                          </TableCell>
                        )}
                      </TableRow>
                    ))}
                  </TableBody>
                </Table>
              )}
            </CardContent>
          </Card>
        </div>

        {/* ---- Place hold dialog ---- */}
        <Dialog open={placeOpen} onOpenChange={setPlaceOpen}>
          <DialogContent className='sm:max-w-md'>
            <DialogHeader>
              <DialogTitle>
                {t('legalHold.placeTitle', 'Place a legal hold')}
              </DialogTitle>
              <DialogDescription>
                {t(
                  'legalHold.placeDesc',
                  'Select an account to freeze. While held, the retention sweep skips this account and no messages are purged.'
                )}
              </DialogDescription>
            </DialogHeader>
            <div className='space-y-4'>
              <div className='space-y-2'>
                <Label>
                  {t('legalHold.selectAccounts', 'Accounts')}
                  <span className='text-red-500 align-super text-xs'>*</span>
                </Label>
                <div className='max-h-56 space-y-1 overflow-y-auto rounded border p-2'>
                  <label className='flex cursor-pointer items-center gap-2 rounded px-2 py-1.5 text-xs hover:bg-accent'>
                    <Checkbox
                      checked={allPlaceableSelected}
                      onCheckedChange={toggleAllPlaceable}
                    />
                    <span className='text-muted-foreground'>
                      {t('legalHold.selectAll', 'Select all')} (
                      {placeable.length})
                    </span>
                  </label>
                  {placeable.length === 0 ? (
                    <div className='px-2 py-3 text-center text-xs text-muted-foreground'>
                      {t(
                        'legalHold.noPlaceable',
                        'All accounts are already under a legal hold.'
                      )}
                    </div>
                  ) : (
                    placeable.map((a) => (
                      <label
                        key={a.id}
                        className='flex cursor-pointer items-center gap-2 rounded px-2 py-1.5 text-xs hover:bg-accent'
                      >
                        <Checkbox
                          checked={selectedIds.includes(a.id)}
                          onCheckedChange={() =>
                            toggleId(selectedIds, setSelectedIds, a.id)
                          }
                        />
                        <span className='truncate'>{a.email}</span>
                      </label>
                    ))
                  )}
                </div>
              </div>
              <div className='space-y-2'>
                <Label>{t('legalHold.caseRef', 'Case')}</Label>
                <Input
                  value={placeCaseRef}
                  onChange={(e) => setPlaceCaseRef(e.target.value)}
                  placeholder={t(
                    'legalHold.caseRefPlaceholder',
                    'e.g. 2026-0417'
                  )}
                />
                <p className='text-xs text-muted-foreground'>
                  {t(
                    'legalHold.caseRefDesc',
                    'Optional matter reference. An account can carry more than one active hold.'
                  )}
                </p>
              </div>
              <div className='space-y-2'>
                <Label>
                  {t('legalHold.reason', 'Reason')}
                  <span className='text-red-500 align-super text-xs'>*</span>
                </Label>
                <Textarea
                  value={placeReason}
                  onChange={(e) => setPlaceReason(e.target.value)}
                  placeholder={t(
                    'legalHold.reasonPlaceholder',
                    'e.g. Pending litigation — case #2026-0417'
                  )}
                  rows={3}
                />
              </div>
            </div>
            <DialogFooter>
              <Button variant='outline' onClick={() => setPlaceOpen(false)}>
                {t('common.cancel')}
              </Button>
              <Button
                disabled={
                  selectedIds.length === 0 ||
                  !placeReason.trim() ||
                  placeMutation.isPending
                }
                onClick={() =>
                  placeMutation.mutate({
                    account_ids: selectedIds,
                    reason: placeReason.trim(),
                    case_ref: placeCaseRef.trim() || undefined,
                  })
                }
              >
                <Lock className='mr-1 h-4 w-4' />
                {placeMutation.isPending
                  ? t('legalHold.placing', 'Placing…')
                  : t('legalHold.confirmPlace', 'Place hold', {
                      count: selectedIds.length,
                    })}
              </Button>
            </DialogFooter>
          </DialogContent>
        </Dialog>

        {/* ---- Release hold dialog ---- */}
        <AlertDialog
          open={releasing !== null}
          onOpenChange={(open) => !open && setReleasing(null)}
        >
          <AlertDialogContent>
            <AlertDialogHeader>
              <AlertDialogTitle>
                {approvalEnabled
                  ? t(
                      'legalHold.releaseTitleApproval',
                      'Submit this release for approval?'
                    )
                  : t('legalHold.releaseTitle', 'Release legal hold?')}
              </AlertDialogTitle>
              <AlertDialogDescription>
                {approvalEnabled
                  ? t(
                      'legalHold.releaseDescApproval',
                      'Dual control is on. Requesting the release of {email} does NOT release it — a second person must approve the request first, and the account stays on hold until they do.',
                      { email: releasing?.email ?? '' }
                    )
                  : t(
                      'legalHold.releaseDesc',
                      'Releasing the hold on {email} resumes the retention sweep for this account. Older messages may be purged on the next sweep.',
                      { email: releasing?.email ?? '' }
                    )}
              </AlertDialogDescription>
            </AlertDialogHeader>
            {releasing?.case_ref && (
              <div className='text-xs text-muted-foreground'>
                {t('legalHold.caseRef', 'Case')}: {releasing.case_ref}
              </div>
            )}
            <div className='space-y-2'>
              <Label className='text-xs'>
                {t('legalHold.releaseReason', 'Reason (optional)')}
              </Label>
              <Textarea
                value={releaseReason}
                onChange={(e) => setReleaseReason(e.target.value)}
                placeholder={t(
                  'legalHold.releaseReasonPlaceholder',
                  'e.g. Case closed'
                )}
                rows={2}
              />
            </div>
            <AlertDialogFooter>
              <AlertDialogCancel>{t('common.cancel')}</AlertDialogCancel>
              <AlertDialogAction
                className={cn('bg-destructive text-destructive-foreground')}
                disabled={releaseMutation.isPending}
                onClick={() =>
                  releasing &&
                  releaseMutation.mutate({
                    hold_id: releasing.hold_id,
                    account_id: releasing.id,
                    reason: releaseReason.trim() || undefined,
                  })
                }
              >
                <LockOpen className='mr-1 h-4 w-4' />
                {releaseMutation.isPending
                  ? t('legalHold.releasing', 'Releasing…')
                  : approvalEnabled
                    ? t('legalHold.confirmRequestRelease', 'Submit for approval')
                    : t('legalHold.confirmRelease', 'Release hold')}
              </AlertDialogAction>
            </AlertDialogFooter>
          </AlertDialogContent>
        </AlertDialog>

        {/* ---- Batch release hold dialog ---- */}
        <AlertDialog
          open={releaseBatchOpen}
          onOpenChange={setReleaseBatchOpen}
        >
          <AlertDialogContent>
            <AlertDialogHeader>
              <AlertDialogTitle>
                {approvalEnabled
                  ? t(
                      'legalHold.releaseBatchTitleApproval',
                      'Submit these releases for approval?'
                    )
                  : t(
                      'legalHold.releaseBatchTitle',
                      'Release selected holds?'
                    )}
              </AlertDialogTitle>
              <AlertDialogDescription>
                {approvalEnabled
                  ? t(
                      'legalHold.releaseBatchDescApproval',
                      'Dual control is on. Requesting the release of {{count}} account(s) does NOT release them — a second person must approve the request first.',
                      { count: heldSelected.length }
                    )
                  : t(
                      'legalHold.releaseBatchDesc',
                      'Releasing {{count}} account(s) resumes the retention sweep for each of them. Older messages may be purged on the next sweep.',
                      { count: heldSelected.length }
                    )}
              </AlertDialogDescription>
            </AlertDialogHeader>
            <div className='space-y-2'>
              <Label className='text-xs'>
                {t('legalHold.releaseReason', 'Reason (optional)')}
              </Label>
              <Textarea
                value={releaseBatchReason}
                onChange={(e) => setReleaseBatchReason(e.target.value)}
                placeholder={t(
                  'legalHold.releaseReasonPlaceholder',
                  'e.g. Case closed'
                )}
                rows={2}
              />
            </div>
            <AlertDialogFooter>
              <AlertDialogCancel>{t('common.cancel')}</AlertDialogCancel>
              <AlertDialogAction
                className={cn('bg-destructive text-destructive-foreground')}
                disabled={releaseBatchMutation.isPending}
                onClick={() =>
                  releaseBatchMutation.mutate({
                    account_ids: heldSelected,
                    reason: releaseBatchReason.trim() || undefined,
                  })
                }
              >
                <LockOpen className='mr-1 h-4 w-4' />
                {releaseBatchMutation.isPending
                  ? t('legalHold.releasing', 'Releasing…')
                  : approvalEnabled
                    ? t(
                        'legalHold.confirmRequestRelease',
                        'Submit for approval'
                      )
                    : t('legalHold.releaseSelected', 'Release selected', {
                        count: heldSelected.length,
                      })}
              </AlertDialogAction>
            </AlertDialogFooter>
          </AlertDialogContent>
        </AlertDialog>

        {/* ---- Dual control moved to its own page ---- */}
        {/* The queue used to be rendered here. It now covers five operations,
            not just hold releases, so it lives on /approvals and this console
            only points at it. */}
        {approvalEnabled && (
          <div className='mx-auto w-full max-w-7xl'>
            <DualControlNotice />
          </div>
        )}
      </Main>
    </>
  )
}
