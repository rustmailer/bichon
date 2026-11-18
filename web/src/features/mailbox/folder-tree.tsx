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

import * as React from 'react'
import { useTranslation } from 'react-i18next'
import Collapse from '@mui/material/Collapse'
import {
  TreeItemContent,
  TreeItemIcon,
  TreeItemIconContainer,
  TreeItemLabel,
  TreeItemProvider,
  TreeItemRoot,
  useTreeItemModel,
} from '@mui/x-tree-view'
import { RichTreeView } from '@mui/x-tree-view/RichTreeView'
import { useTreeItem, UseTreeItemParameters } from '@mui/x-tree-view/useTreeItem'
import { Clock, FolderIcon, Folders, Search, Trash2 } from 'lucide-react'

import type { MinimalAccount } from '@/api/account/api'
import type { MailboxData } from '@/api/mailbox/api'
import { OP_MAILBOX_DELETE } from '@/api/approvals/api'
import { buildTree, type ExtendedTreeItemProps } from '@/lib/build-tree'
import { ScrollArea } from '@/components/ui/scroll-area'
import { Input } from '@/components/ui/input'
import { usePendingApprovals } from '@/features/approvals/use-pending-approvals'
import { AccountPicker } from './account-picker'
import { cn } from '@/lib/utils'

interface Props {
  accounts: MinimalAccount[]
  accountId?: number
  mailboxes: MailboxData[]
  mailboxesLoading: boolean
  mailboxesError: boolean
  mailboxId?: number
  onSelectAccount: (accountId: number) => void
  onSelectMailbox: (mailboxId: number | undefined) => void
  onDeleteMailbox: (mailboxId: number) => void
  className?: string
}

interface CustomTreeItemProps
  extends Omit<UseTreeItemParameters, 'rootRef'>,
  Omit<React.HTMLAttributes<HTMLLIElement>, 'onFocus'> { }

/**
 * Account / folder navigation for the mailbox view.
 *
 * Deliberately owned by this feature: the search page's mailbox popover is a
 * multi-select *filter*, while this tree is single-account navigation. They
 * share the mailbox API, `buildTree` and the per-folder delete action, not
 * their state or UI.
 */
export function FolderTree({
  accounts,
  accountId,
  mailboxes,
  mailboxesLoading,
  mailboxesError,
  mailboxId,
  onSelectAccount,
  onSelectMailbox,
  onDeleteMailbox,
  className,
}: Props) {
  const { t } = useTranslation()
  const { hasPending } = usePendingApprovals()
  const [search, setSearch] = React.useState('')

  const treeData = React.useMemo(() => {
    const filtered = search.trim()
      ? mailboxes.filter((mailbox) =>
        mailbox.name.toLowerCase().includes(search.toLowerCase())
      )
      : mailboxes
    return buildTree(filtered)
  }, [mailboxes, search])

  const handleSelect = (itemId: string | null) => {
    if (itemId === null) return
    const numericId = Number(itemId)
    // Parent nodes that are not real mailboxes carry their full path as the
    // id; only numeric ids name a mailbox in the index.
    if (!Number.isFinite(numericId)) return
    onSelectMailbox(numericId)
  }

  const CustomTreeItem = React.forwardRef(function CustomTreeItem(
    props: CustomTreeItemProps,
    ref: React.Ref<HTMLLIElement>,
  ) {
    const { id, itemId, label, disabled, children, ...other } = props
    const {
      getContextProviderProps,
      getRootProps,
      getContentProps,
      getLabelProps,
      getIconContainerProps,
      getGroupTransitionProps,
      status,
    } = useTreeItem({ id, itemId, children, label, disabled, rootRef: ref })
    const item = useTreeItemModel<ExtendedTreeItemProps>(itemId)

    const numericId = item?.id !== undefined ? Number(item.id) : NaN
    const deletable = Number.isFinite(numericId)
    const deleteQueued =
      accountId !== undefined &&
      deletable &&
      hasPending(OP_MAILBOX_DELETE, `mailbox:${accountId}:${item!.id}`)

    return (
      <TreeItemProvider {...getContextProviderProps()}>
        <TreeItemRoot {...getRootProps(other)} className="group">
          <TreeItemContent {...getContentProps()} sx={{ paddingY: '2px' }}>
            <TreeItemIconContainer {...getIconContainerProps()}>
              <TreeItemIcon status={status} />
            </TreeItemIconContainer>
            <TreeItemLabel
              {...getLabelProps()}
              sx={{ display: 'flex', alignItems: 'center', minWidth: 0 }}
            >
              <FolderIcon className="mr-2 h-3.5 w-3.5 shrink-0" />
              <span className="truncate font-medium text-xs text-inherit">
                {label}
              </span>
              {deletable && (
                <span className="ml-auto pl-2 opacity-0 transition-opacity group-hover:opacity-100 focus-within:opacity-100">
                  {/* Plain button instead of <Button size="icon">: the Button
                      base style's `[&_svg]:size-4` outranks the icon's own
                      size classes and inflates it inside this 12px row. */}
                  <button
                    type="button"
                    className="flex h-4 w-4 items-center justify-center rounded-sm text-muted-foreground transition-colors hover:bg-accent hover:text-destructive disabled:pointer-events-none disabled:opacity-50"
                    disabled={deleteQueued}
                    title={
                      deleteQueued
                        ? t('approvals.pendingBadge', 'Awaiting approval')
                        : t('common.delete')
                    }
                    onMouseDown={(event) => event.stopPropagation()}
                    onClick={(event) => {
                      event.stopPropagation()
                      event.preventDefault()
                      onDeleteMailbox(numericId)
                    }}
                  >
                    {deleteQueued ? (
                      <Clock className="h-3 w-3" />
                    ) : (
                      <Trash2 className="h-3 w-3" />
                    )}
                  </button>
                </span>
              )}
            </TreeItemLabel>
          </TreeItemContent>
          {children && <Collapse {...getGroupTransitionProps()} />}
        </TreeItemRoot>
      </TreeItemProvider>
    )
  })

  return (
    <aside
      className={cn(
        'flex min-h-0 flex-col rounded-md border bg-background',
        className
      )}
    >
      <div className="space-y-2 border-b p-2">
        <div className="flex items-center gap-1.5 text-xs font-medium text-muted-foreground">
          <Folders className="h-3.5 w-3.5" />
          {t('mailboxView.folders')}
        </div>
        <AccountPicker
          accounts={accounts}
          value={accountId}
          onChange={onSelectAccount}
          className="h-8 w-full text-xs font-normal"
        />
        <div className="relative">
          <Search className="absolute left-2 top-2 h-3.5 w-3.5 text-muted-foreground" />
          <Input
            value={search}
            onChange={(event) => setSearch(event.target.value)}
            placeholder={t('mailboxView.searchFolders')}
            // `md:text-xs` overrides the Input's built-in `md:text-sm`, which
            // would otherwise win at desktop widths.
            className="h-8 pl-7 text-xs md:text-xs"
          />
        </div>
      </div>

      <button
        type="button"
        onClick={() => onSelectMailbox(undefined)}
        className={cn(
          'mx-2 mt-2 flex items-center rounded px-2 py-1 text-left text-xs transition-colors hover:bg-accent',
          mailboxId === undefined && 'bg-primary/10 text-primary'
        )}
      >
        {t('mailboxView.allFolders')}
      </button>

      {/* Both axes: accounts can have hundreds of folders (vertical) and deep,
          long names (horizontal). */}
      <ScrollArea className="mt-1 min-h-0 flex-1" orientation="both">
        <div className="w-max min-w-full px-1 pb-2">
          {mailboxesLoading ? (
            <div className="space-y-1 p-2">
              {Array.from({ length: 8 }).map((_, index) => (
                <div
                  key={index}
                  className="h-4 animate-pulse rounded bg-muted"
                />
              ))}
            </div>
          ) : mailboxesError ? (
            <p className="p-2 text-xs text-muted-foreground">
              {t('mailboxView.loadError')}
            </p>
          ) : accounts.length === 0 ? (
            <p className="p-2 text-xs text-muted-foreground">
              {t('mailboxView.noAccounts')}
            </p>
          ) : treeData.length === 0 ? (
            <p className="p-2 text-xs text-muted-foreground">
              {t('mailboxView.empty')}
            </p>
          ) : (
            <RichTreeView
              items={treeData}
              selectedItems={mailboxId !== undefined ? String(mailboxId) : null}
              onSelectedItemsChange={(_event, itemId) =>
                handleSelect(itemId as string | null)
              }
              expansionTrigger="iconContainer"
              slots={{ item: CustomTreeItem }}
              sx={{
                // Grow with the widest visible row so the horizontal
                // scrollbar has something to scroll.
                width: 'max-content',
                minWidth: '100%',
                '& .MuiTreeItem-label': {
                  fontSize: '0.75rem',
                  paddingLeft: '2px',
                  // Keep long folder names on one line so the horizontal
                  // scrollbar can reveal them instead of wrapping/cutting.
                  whiteSpace: 'nowrap',
                },
                '& .MuiTreeItem-content': {
                  paddingY: '2px',
                  borderRadius: '0.25rem',
                },
                '& .MuiTreeItem-iconContainer': {
                  width: '18px',
                },
              }}
            />
          )}
        </div>
      </ScrollArea>
    </aside>
  )
}
