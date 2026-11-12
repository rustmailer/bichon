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



import { useRef } from 'react'
import { X, Trash2, Upload, TagIcon, ShieldAlert, ListChecks } from 'lucide-react'
import { cn } from '@/lib/utils'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Separator } from '@/components/ui/separator'
import {
    Tooltip,
    TooltipTrigger,
    TooltipContent,
} from '@/components/ui/tooltip'
import { useSearchContext } from './context'
import { useTranslation } from 'react-i18next'
import { toast } from '@/hooks/use-toast'
import { OP_GDPR_ERASURE, OP_MESSAGE_DELETE } from '@/api/approvals/api'
import { usePendingApprovals } from '@/features/approvals/use-pending-approvals'
import { useEdition } from '@/hooks/use-edition'
import { useCurrentUser } from '@/hooks/use-current-user'

type MailBulkActionsProps = {
    children?: React.ReactNode
    /** Total matching the current filter, when the page is fully selected and
     *  more results exist. Present means "offer to widen to all matches". */
    selectAllMatchingCount?: number
}

export function MailBulkActions({
    children,
    selectAllMatchingCount,
}: MailBulkActionsProps) {
    const {
        selected,
        setSelected,
        selectAllScope,
        setSelectAllScope,
        filter,
        setOpen,
        setToDelete,
        setCurrentEnvelope,
    } = useSearchContext()
    const toolbarRef = useRef<HTMLDivElement>(null)
    const { t, i18n } = useTranslation()
    const { countPendingMessages, hasPendingMessage } = usePendingApprovals()
    // GDPR erasure requests are an Enterprise feature — the record they leave
    // is part of the compliance suite. Hidden rather than shown-and-refused:
    // a button that only ever answers 403 teaches operators that the archive
    // is broken.
    const { isEnterprise } = useEdition()
    const erasureQueuedCount = countPendingMessages(OP_GDPR_ERASURE, selected)
    // Every selected message must be actionable: the toolbar acts on the whole
    // selection, so one account without the permission would make the request
    // fail. Judged per message's account, skipping the action otherwise.
    const { require_any_permission } = useCurrentUser()
    const scopeMode = !!selectAllScope
    const selectedAccountIds = Array.from(selected.keys())
    const canForAll = (perms: string[]) =>
        selectedAccountIds.length > 0 &&
        selectedAccountIds.every((id) => require_any_permission(perms, id))
    // A scope has no explicit account list on the client. Only deletion
    // supports it in v1; the server resolves and checks the accounts when the
    // job is created (and the preview returns a clear error otherwise).
    const canRestoreAll = !scopeMode && canForAll([
        'system:root',
        'data:export:batch',
        'data:export:batch:all',
    ])
    const canTagAll = !scopeMode && canForAll([
        'system:root',
        'data:manage',
        'data:manage:all',
    ])
    const canDeleteAll = scopeMode || canForAll([
        'system:root',
        'data:delete',
        'data:delete:all',
    ])
    const noPermission = t('common.noPermission', 'No permission')

    // In "all matching results" mode the count comes from the search total,
    // not from the explicit selection: there are no per-message ids on the
    // client, which is the whole point.
    const selectedCount = scopeMode
        ? selectAllScope.total
        : Array.from(selected.values()).reduce((sum, set) => sum + set.size, 0)
    // Locale-aware grouping (12,345 / 12.345 / 12 345). Paired with
    // `tabular-nums` below, the floating bar's width follows the digit count
    // instead of jittering as the displayed number changes.
    const formatCount = (value: number) =>
        new Intl.NumberFormat(i18n.language).format(value)

    // Part of the selection may already be queued. The server has no dedup —
    // it would happily queue a message twice — so those are filtered out
    // before the dialog opens, and this count explains the difference between
    // what was selected and what will actually be submitted.
    // Scope mode has no per-message ids to compare against the approval queue;
    // the server deduplicates nothing, so the dialog warns instead of showing
    // a precise skipped count.
    const queuedCount = scopeMode
        ? 0
        : countPendingMessages(OP_MESSAGE_DELETE, selected)

    const handleClearSelection = () => {
        setSelectAllScope(null)
        setSelected(new Map())
    }

    /** Scope mode: the delete dialog resolves the count and starts the job. */
    const handleDeleteScope = () => {
        setToDelete(new Map())
        setOpen('delete')
    }

    const handleDelete = () => {
        setToDelete(new Map())
        selected.forEach((ids, accountId) => {
            setToDelete(prev => {
                const next = new Map(prev)
                next.set(accountId, new Set(ids))
                return next
            })
        })
        setOpen('delete')
    }

    /** Open the delete dialog, leaving out anything already queued.
     *
     *  Skipping rather than submitting the lot: the server has no dedup, so a
     *  message that already awaits approval would be queued a second time and
     *  two approvers would each execute a deletion of the same message. */
    const handleDeleteSkippingQueued = () => {
        if (queuedCount === 0) {
            handleDelete()
            return
        }
        const remaining = new Map<number, Set<string>>()
        selected.forEach((ids, accountId) => {
            const keep = new Set(
                Array.from(ids).filter(
                    (id) => !hasPendingMessage(OP_MESSAGE_DELETE, accountId, id)
                )
            )
            if (keep.size > 0) remaining.set(accountId, keep)
        })
        setToDelete(remaining)
        if (remaining.size === 0) {
            toast({
                title: t(
                    'approvals.allQueuedTitle',
                    'Nothing to submit — every selected message already awaits approval'
                ),
                description: t(
                    'approvals.allQueuedDesc',
                    'A second person has to decide on those requests before they can be asked for again.'
                ),
            })
            return
        }
        setOpen('delete')
    }


    const handleRestore = () => {
        setCurrentEnvelope(undefined);
        setOpen('restore')
    }

    /** Open the GDPR erasure form for the current selection, leaving out
     *  anything that already has an erasure request waiting.
     *
     *  Same reasoning as `handleDeleteSkippingQueued`: the server has no
     *  dedup, so a message already awaiting an erasure decision would be
     *  recorded a second time and two approvers would each authorize erasing
     *  it — two records for one erasure, which is worse than an untidy queue
     *  because the records are the evidence.
     *
     *  Reuses `toDelete` for the same reason the delete dialog does — the
     *  dialog reads one "what was selected" slot — but nothing is erased until
     *  the request is decided, so the two never run at once. */
    const handleErasureRequest = () => {
        if (erasureQueuedCount === 0) {
            setToDelete(new Map(selected))
            setOpen('gdpr-erasure')
            return
        }
        const remaining = new Map<number, Set<string>>()
        selected.forEach((ids, accountId) => {
            const keep = new Set(
                Array.from(ids).filter(
                    (id) => !hasPendingMessage(OP_GDPR_ERASURE, accountId, id)
                )
            )
            if (keep.size > 0) remaining.set(accountId, keep)
        })
        if (remaining.size === 0) {
            toast({
                title: t(
                    'approvals.allQueuedTitle',
                    'Nothing to submit — every selected message already awaits approval'
                ),
                description: t(
                    'approvals.allQueuedDesc',
                    'A second person has to decide on those requests before they can be asked for again.'
                ),
            })
            return
        }
        setToDelete(remaining)
        setOpen('gdpr-erasure')
    }


    const handleUpdateTags = () => {
        setOpen('update-tags')
    }

    const handleKeyDown = (e: React.KeyboardEvent) => {
        const buttons = toolbarRef.current?.querySelectorAll('button')
        if (!buttons || buttons.length === 0) return

        const currentIndex = Array.from(buttons).findIndex(
            btn => btn === document.activeElement
        )

        switch (e.key) {
            case 'ArrowRight': {
                e.preventDefault()
                const next = (currentIndex + 1) % buttons.length
                buttons[next]?.focus()
                break
            }
            case 'ArrowLeft': {
                e.preventDefault()
                const prev = currentIndex === 0 ? buttons.length - 1 : currentIndex - 1
                buttons[prev]?.focus()
                break
            }
            case 'Home':
                e.preventDefault()
                buttons[0]?.focus()
                break
            case 'End':
                e.preventDefault()
                buttons[buttons.length - 1]?.focus()
                break
            case 'Escape': {
                const target = e.target as HTMLElement
                const active = document.activeElement as HTMLElement
                const isFromDropdown =
                    target.closest('[data-slot="dropdown-menu-trigger"]') ||
                    active.closest('[data-slot="dropdown-menu-trigger"]') ||
                    target.closest('[data-slot="dropdown-menu-content"]') ||
                    active.closest('[data-slot="dropdown-menu-content"]')

                if (!isFromDropdown) {
                    e.preventDefault()
                    handleClearSelection()
                }
                break
            }
        }
    }

    if (selectedCount === 0) return null

    return (
        <>
            <div
                ref={toolbarRef}
                role="toolbar"
                aria-label={t('search.bulkActions.ariaLabel', {
                    count: selectedCount,
                })}
                tabIndex={-1}
                onKeyDown={handleKeyDown}
                className={cn(
                    'fixed bottom-6 left-1/2 z-50 -translate-x-1/2 rounded-xl',
                    'transition-all delay-100 duration-300 ease-out hover:scale-105',
                    'focus-visible:ring-ring/50 focus-visible:ring-2 focus-visible:outline-none'
                )}
            >
                <div
                    className={cn(
                        'p-2 shadow-xl rounded-xl border',
                        'bg-background/95 supports-[backdrop-filter]:bg-background/60 backdrop-blur-lg',
                        'flex items-center gap-x-2'
                    )}
                >
                    <Tooltip>
                        <TooltipTrigger asChild>
<span className="inline-flex">
                            <Button
                                variant="outline"
                                size="icon"
                                onClick={handleClearSelection}
                                className="size-6 rounded-full"
                                aria-label={t('search.bulkActions.clear')}
                            >
                                <X className="h-3 w-3" />
                                <span className="sr-only">{t('search.bulkActions.clear')}</span>
                            </Button>
                        </span>
</TooltipTrigger>
                    </Tooltip>
                    <Separator orientation="vertical" className="h-5" />
                    <div className="flex items-center gap-x-1 text-sm">
                        <Badge
                            variant="default"
                            className="min-w-8 rounded-lg tabular-nums"
                        >
                            {formatCount(selectedCount)}
                        </Badge>{' '}
                        {scopeMode ? (
                            <span className="text-xs text-muted-foreground">
                                {t('search.batchDelete.scopeLabel', 'All matching messages')}
                            </span>
                        ) : selectAllMatchingCount !== undefined ? (
                            <Button
                                variant="outline"
                                size="sm"
                                className="h-7 gap-1 px-2 text-xs tabular-nums"
                                title={t('search.batchDelete.selectAllMatching', {
                                    formatted: formatCount(selectAllMatchingCount),
                                    defaultValue: 'Select all {{formatted}} matching results',
                                })}
                                onClick={() =>
                                    setSelectAllScope({
                                        filterJson: JSON.stringify(filter),
                                        total: selectAllMatchingCount,
                                    })
                                }
                            >
                                <ListChecks className="h-3.5 w-3.5" />
                                {t('search.batchDelete.selectAllMatchingShort', {
                                    formatted: formatCount(selectAllMatchingCount),
                                    defaultValue: 'All {{formatted}}',
                                })}
                            </Button>
                        ) : null}
                    </div>
                    <Separator orientation="vertical" className="h-5" />
                    <Tooltip>
                        <TooltipTrigger asChild>
<span className="inline-flex">
                            <Button
                                variant="secondary"
                                size="sm"
                                onClick={handleRestore}
                                disabled={!canRestoreAll}
                                className="gap-1"
                            >
                                <Upload className="h-3.5 w-3.5" />
                            </Button>
                        </span>
</TooltipTrigger>
                        <TooltipContent>
                            {canRestoreAll
                                ? t('restore_message.restore_to_imap', 'Restore Mail')
                                : noPermission}
                        </TooltipContent>
                    </Tooltip>
                    <Separator orientation="vertical" className="h-5" />
                    <Tooltip>
                        <TooltipTrigger asChild>
<span className="inline-flex">
                            <Button
                                variant="secondary"
                                size="sm"
                                onClick={handleUpdateTags}
                                disabled={!canTagAll}
                                className="gap-1"
                            >
                                <TagIcon className="h-3.5 w-3.5" />
                            </Button>
                        </span>
</TooltipTrigger>
                        <TooltipContent>
                            {canTagAll ? t('search.bulkActions.manageTags') : noPermission}
                        </TooltipContent>
                    </Tooltip>
                    <Separator orientation="vertical" className="h-5" />
                    {isEnterprise && (
                        <>
                            <Tooltip>
                                <TooltipTrigger asChild>
<span className="inline-flex">
                                    <Button
                                        variant="secondary"
                                        size="sm"
                                        onClick={handleErasureRequest}
                                        disabled={!canDeleteAll}
                                        className="gap-1"
                                    >
                                        <ShieldAlert className="h-3.5 w-3.5" />
                                    </Button>
                                </span>
</TooltipTrigger>
                                <TooltipContent>
                                    {!canDeleteAll
                                        ? noPermission
                                        : erasureQueuedCount > 0
                                        ? t(
                                              'approvals.bulkPartlyQueued',
                                              '{{queued}} of {{total}} selected message(s) already await approval and will be skipped',
                                              {
                                                  queued: erasureQueuedCount,
                                                  total: selectedCount,
                                              }
                                          )
                                        : t(
                                              'search.bulkActions.gdprErasure',
                                              'Record a GDPR erasure request'
                                          )}
                                </TooltipContent>
                            </Tooltip>
                            <Separator orientation="vertical" className="h-5" />
                        </>
                    )}
                    <Tooltip>
                        <TooltipTrigger asChild>
<span className="inline-flex">
                            <Button
                                variant="destructive"
                                size="sm"
                                onClick={scopeMode ? handleDeleteScope : handleDeleteSkippingQueued}
                                disabled={!canDeleteAll}
                                className="gap-1"
                            >
                                <Trash2 className="h-3.5 w-3.5" />
                                {!scopeMode && queuedCount > 0 && (
                                    <span className="text-[10px] leading-none">
                                        {selectedCount - queuedCount}
                                    </span>
                                )}
                            </Button>
                        </span>
</TooltipTrigger>
                        <TooltipContent>
                            {!canDeleteAll
                                ? noPermission
                                : queuedCount > 0
                                ? t(
                                      'approvals.bulkPartlyQueued',
                                      '{{queued}} of {{total}} selected message(s) already await approval and will be skipped',
                                      { queued: queuedCount, total: selectedCount }
                                  )
                                : t('search.bulkActions.deleteDesc')}
                        </TooltipContent>
                    </Tooltip>

                    {children}
                </div>
            </div>
        </>
    )
}

