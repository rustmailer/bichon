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


import { cn, dateFnsLocaleMap, formatBytes, formatSender } from "@/lib/utils"
import { format, formatDistanceToNow } from "date-fns"
import { MessageSquareText, Paperclip } from "lucide-react"
import { Skeleton } from "@/components/ui/skeleton"
import { Checkbox } from "@/components/ui/checkbox"
import { EmailEnvelope } from "@/api"
import { useSearchContext } from "./context"
import { MailBulkActions } from "./bulk-actions"
import { useTranslation } from 'react-i18next'
import { enUS } from "date-fns/locale"
import { ColumnDef } from "@tanstack/react-table"
import LongText from "@/components/long-text"
import { DataTableColumnHeader } from "./table/data-table-column-header"
import { SearchTable } from "./table/table"
import { DataTableRowActions } from "./table/data-table-row-actions"
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { DataTableToolbar } from "./table/toolbar"
import { HoverCard, HoverCardContent, HoverCardTrigger } from "@/components/ui/hover-card"
import { useSearchMessages } from "@/hooks/use-search-messages"
import useMinimalAccountList from "@/hooks/use-minimal-account-list"

interface MailListProps {
  items: EmailEnvelope[]
  isLoading: boolean
  onEnvelopeChanged: (envelope: EmailEnvelope) => void
  setSortBy: (sortBy: "DATE" | "SIZE" | "RELEVANCE") => void
  setSortOrder: (value: "desc" | "asc") => void
}

export function MailListTable({
  items,
  isLoading,
  onEnvelopeChanged,
  setSortBy,
  setSortOrder
}: MailListProps) {
  const { t, i18n } = useTranslation()

  const locale = dateFnsLocaleMap[i18n.language.toLowerCase()] ?? enUS
  const { selected, setSelected, dateDisplay } = useSearchContext()
  const { minimalList = [] } = useMinimalAccountList()

  // Which accounts this user may name in a filter. A view is not one of them:
  // the `account_id` on a result row is the *source* account (the row is the
  // source's message), and a viewer holds no grant on it — the server drops an
  // account the caller cannot read, so clicking through would empty the list
  // instead of narrowing it. Only accounts the account picker offers are
  // clickable.
  const filterableAccounts = new Set(minimalList.map((account) => account.id))

  const columns: ColumnDef<EmailEnvelope>[] = [
    {
      accessorKey: "id",
      header: () => (
        <Checkbox
          checked={
            totalSelected === items.length && items.length > 0
              ? true
              : totalSelected > 0
                ? "indeterminate"
                : false
          }
          onCheckedChange={handleToggleAll}
          className="h-4 w-4"
        />
      ),
      cell: ({ row }) => (
        <Checkbox
          checked={hasSelected(row.original.account_id, row.original.id)}
          onCheckedChange={() => toggleSelected(row.original.account_id, row.original.id)}
          onClick={(e) => e.stopPropagation()}
          className="h-4 w-4 shrink-0"
        />
      ),
      meta: { className: 'text-left text-sm' },
      minSize: 25,
      maxSize: 25,
    },
    {
      accessorKey: "source",
      header: t('search.source'),
      cell: ({ row }) => {
        const { from, from_name, wire_from, account_email, account_name, mailbox_name, account_id, mailbox_id } = row.original;
        const { setFilter } = useSearchMessages();
        const accountPrefix = account_name || account_email.split('@')[0];//https://github.com/rustmailer/bichon/issues/306
        // A view's rows name the source account, which the viewer cannot filter
        // by — see `filterableAccounts`. The labels still show it (that is
        // where the message came from), they just stop being links.
        const canFilterAccount = filterableAccounts.has(account_id);

        return (
          <div className="flex flex-col py-0.5 min-w-0 group">
            <div
              className="cursor-pointer hover:text-primary transition-colors flex items-center gap-1.5"
              title={wire_from ? `via ${wire_from}` : undefined}
              onClick={(e) => {
                e.stopPropagation();
                setFilter((prev: any) => ({ ...prev, from: from }));
              }}
            >
              <LongText className="text-xs truncate">
                {formatSender(from_name, from)}
              </LongText>
            </div>

            <div className="flex items-center gap-1 mt-1 text-[10px] text-muted-foreground/70">
              <span
                className={cn(
                  "truncate max-w-[90px] transition-colors",
                  canFilterAccount && "hover:text-primary cursor-pointer"
                )}
                title={account_email}
                onClick={canFilterAccount ? (e) => {
                  e.stopPropagation();
                  setFilter((prev: any) => ({ ...prev, account_ids: [account_id], mailbox_ids: undefined }));
                } : undefined}
              >
                {accountPrefix}
              </span>

              <span className="shrink-0 opacity-40">/</span>
              <span
                className={cn(
                  "truncate max-w-[70px] transition-colors",
                  canFilterAccount && "hover:text-primary cursor-pointer"
                )}
                onClick={canFilterAccount ? (e) => {
                  e.stopPropagation();
                  setFilter((prev: any) => ({ ...prev, account_ids: [account_id], mailbox_ids: [mailbox_id] }));
                } : undefined}
              >
                {mailbox_name}
              </span>
            </div>
          </div>
        );
      },
      meta: { className: 'text-left text-xs' },
    },
    {
      accessorKey: "to",
      header: t('search.to'),
      cell: ({ row }) => {
        const recipients: string[] = row.original.to || [];
        const { setFilter } = useSearchMessages();
        const MAX_VISIBLE = 2;
        const visible = recipients.slice(0, MAX_VISIBLE);
        const hidden = recipients.slice(MAX_VISIBLE);

        const handleClick = (e: React.MouseEvent, email: string) => {
          e.stopPropagation();
          setFilter((prev: Record<string, any>) => ({ ...prev, to: email }));
        };

        return (
          <div className="group relative flex items-center w-full min-w-0 px-2 py-0.5 overflow-hidden">
            <div className="absolute left-0 top-0 bottom-0 w-[2px] bg-primary opacity-0 group-hover:opacity-100 transition-opacity" />
            <div className="text-xs flex flex-wrap items-center gap-x-1 gap-y-0.5 min-w-0 flex-1">
              {visible.map((email, index) => (
                <span key={index} className="flex items-center">
                  <button
                    type="button"
                    onClick={(e) => handleClick(e, email)}
                    className="hover:text-primary hover:underline transition-colors truncate max-w-[160px]"
                  >
                    {email}
                  </button>
                  {index < visible.length - 1 && (
                    <span className="text-muted-foreground ml-0.5">,</span>
                  )}
                </span>
              ))}

              {hidden.length > 0 && (
                <Tooltip>
                  <TooltipTrigger asChild>
                    <span
                      onClick={(e) => e.stopPropagation()}
                      className="inline-flex items-center justify-center rounded-full bg-muted text-muted-foreground px-1.5 py-0.5 text-[10px] leading-none cursor-default hover:bg-accent hover:text-accent-foreground transition-colors shrink-0"
                    >
                      +{hidden.length}
                    </span>
                  </TooltipTrigger>
                  <TooltipContent className="max-w-[480px] bg-popover text-popover-foreground border">
                    <div className="flex flex-col gap-1">
                      {hidden.map((email, idx) => (
                        <button
                          key={idx}
                          type="button"
                          onClick={(e) => handleClick(e, email)}
                          className="text-left text-xs px-1 py-0.5 rounded hover:bg-accent hover:text-accent-foreground hover:underline transition-colors"
                        >
                          {email}
                        </button>
                      ))}
                    </div>
                  </TooltipContent>
                </Tooltip>
              )}
            </div>
          </div>
        );
      },
      meta: { className: 'text-left text-xs' }
    },
    {
      accessorKey: "subject",
      header: t('search.subject'),
      cell: ({ row }) => {
        const tags = row.original.tags ?? [];
        return (
          <div className="flex flex-col gap-1 py-0.5 min-w-0">
            <LongText className='text-xs font-medium truncate'>
              {row.original.subject}
            </LongText>
            {tags.length > 0 && (
              <div className="flex items-center gap-1 flex-wrap">
                {tags.map((tag) => (
                  <span
                    key={tag}
                    className="inline-flex items-center rounded-md bg-primary/15 px-2 py-0.5 text-[10px] font-semibold text-primary ring-1 ring-inset ring-primary/20"
                  >
                    {tag}
                  </span>
                ))}
              </div>
            )}
          </div>
        );
      },
      meta: { className: 'text-left text-xs' },
      minSize: 500,
      maxSize: 500,
    },
    {
      id: "text_preview",
      header: t('search.preview'),
      cell: ({ row }) => {
        const preview = row.original.preview

        if (!preview) return null

        return (
          <HoverCard openDelay={200} closeDelay={150}>
            <HoverCardTrigger asChild>
              <button
                type="button"
                className="text-muted-foreground hover:text-primary transition"
                onClick={(e) => e.stopPropagation()}
              >
                <MessageSquareText size={16} />
              </button>
            </HoverCardTrigger>

            <HoverCardContent
              side="right"
              align="start"
              className="max-w-[520px] max-h-[420px] overflow-auto whitespace-pre-wrap text-xs leading-relaxed"
            >
              {preview}
            </HoverCardContent>
          </HoverCard>
        )
      },
      meta: { className: "text-center text-xs" },
      enableSorting: false,
      minSize: 100,
      maxSize: 100,
    },
    {
      id: "attachment_count",
      header: () => <Paperclip size={16} />,
      cell: ({ row }) => <span className='text-xs'>{row.original.regular_attachment_count}</span>,
      meta: { className: 'text-left text-xs' },
      minSize: 30,
      maxSize: 30,
    },
    {
      accessorKey: 'size',
      header: ({ column }) => (
        <DataTableColumnHeader column={column} title={t('search.size')} />
      ),
      cell: ({ row }) => <span className='text-xs max-w-[40px]'>{formatBytes(row.original.size)}</span>,
      meta: { className: 'text-left text-xs' },
      minSize: 80,
      maxSize: 80,
    },
    {
      accessorKey: 'date',
      header: ({ column }) => (
        <DataTableColumnHeader column={column} title={t('search.date')} />
      ),
      cell: ({ row }) => {
        const date = new Date(row.original.date)
        const absolute = format(date, 'yyyy-MM-dd HH:mm')
        const relative = formatDistanceToNow(date, { addSuffix: true, locale })
        return (
          <Tooltip>
            <TooltipTrigger asChild>
              <span className='text-xs whitespace-nowrap'>
                {dateDisplay === 'absolute' ? absolute : relative}
              </span>
            </TooltipTrigger>
            <TooltipContent>
              {dateDisplay === 'absolute' ? relative : format(date, 'yyyy-MM-dd HH:mm:ss')}
            </TooltipContent>
          </Tooltip>
        )
      },
      meta: { className: 'text-left text-xs' },
      minSize: 130,
      maxSize: 130,
    },
    {
      id: 'actions',
      header: t('users.columns.actions'),
      cell: DataTableRowActions,
      meta: { className: 'text-center text-xs' },
      minSize: 60,
      maxSize: 60,
    },
  ]

  const handleToggleAll = () => {
    const total = Array.from(selected.values()).reduce((sum, set) => sum + set.size, 0)

    if (total === items.length && items.length > 0) {
      setSelected(new Map())
    } else {
      setSelected(prev => {
        const next = new Map(prev)
        for (const item of items) {
          const set = new Set(next.get(item.account_id) || [])
          set.add(item.id)
          next.set(item.account_id, set)
        }
        return next
      })
    }
  }

  const toggleSelected = (accountId: number, mailId: string) => {
    setSelected(prev => {
      const next = new Map(prev)
      const set = new Set(next.get(accountId) || [])

      if (set.has(mailId)) {
        set.delete(mailId)
        if (set.size === 0) next.delete(accountId)
        else next.set(accountId, set)
      } else {
        set.add(mailId)
        next.set(accountId, set)
      }
      return next
    })
  }

  const totalSelected = Array.from(selected.values()).reduce((sum, set) => sum + set.size, 0)

  const hasSelected = (accountId: number, mailId: string) => selected.get(accountId)?.has(mailId) ?? false

  if (isLoading) {
    return (
      <div className="divide-y divide-border max-h-[calc(100vh-18rem)] overflow-hidden">
        {Array.from({ length: 30 }).map((_, i) => (
          <div key={i} className="flex items-center gap-2 px-2 py-1.5">
            <Skeleton className="h-3 w-3" />
            <Skeleton className="h-3 w-3 rounded-full" />
            <Skeleton className="h-3 flex-1" />
            <Skeleton className="h-2.5 w-16" />
          </div>
        ))}
      </div>
    )
  }

  return (
    <>
      <SearchTable
        data={items}
        columns={columns}
        onRowClick={(e, row) => {
          const target = e.target as HTMLElement
          if (target.closest('input[type="checkbox"], button')) return
          onEnvelopeChanged(row.original)
        }}
        setSortBy={setSortBy}
        setSortOrder={setSortOrder}
      >
        {(table) => {
          return <DataTableToolbar table={table} />
        }}

      </SearchTable>
      {totalSelected > 0 && <MailBulkActions />}
    </>
  )
}
