//
// Copyright (c) 2025 rustmailer.com (https://rustmailer.com)
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


import { dateFnsLocaleMap, formatBytes } from "@/lib/utils"
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
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from '@/components/ui/tooltip'
import { DataTableToolbar } from "./table/toolbar"
import { HoverCard, HoverCardContent, HoverCardTrigger } from "@/components/ui/hover-card"

interface MailListProps {
  items: EmailEnvelope[]
  isLoading: boolean
  onEnvelopeChanged: (envelope: EmailEnvelope) => void
  setSortBy: (sortBy: "DATE" | "SIZE") => void
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
  const { selected, setSelected } = useSearchContext()

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
      accessorKey: "account_email",
      header: t('search.account'),
      cell: ({ row }) => <LongText className='text-xs'>{row.original.account_email}</LongText>,
      meta: { className: 'text-left text-xs' },
      minSize: 150,
      maxSize: 156,
    },
    {
      accessorKey: "mailbox_name",
      header: t('search.mailbox'),
      cell: ({ row }) => {
        const mailbox = row.original.mailbox_name
        const tags = row.original.tags ?? []

        if (!mailbox) return null

        const visible = tags.slice(0, 3)
        const rest = tags.length - visible.length

        const fullTags = tags.join(' · ')

        return (
          <TooltipProvider delayDuration={200}>
            <Tooltip>
              <TooltipTrigger asChild>
                <div className="flex flex-col leading-tight max-w-[130px] cursor-default">
                  <span className="text-xs truncate">
                    {mailbox}
                  </span>

                  {visible.length > 0 && (
                    <span className="text-[10px] text-primary/80 truncate">
                      {visible.join(' · ')}
                      {rest > 0 && ` · +${rest}`}
                    </span>
                  )}
                </div>
              </TooltipTrigger>

              <TooltipContent
                side="right"
                align="start"
                className="max-w-xs"
              >
                <div className="text-xs font-medium mb-1">
                  {mailbox}
                </div>

                <div className="text-[11px] text-muted-foreground break-words">
                  {fullTags}
                </div>
              </TooltipContent>
            </Tooltip>
          </TooltipProvider>
        )
      },
      meta: { className: 'text-left text-xs' },
      minSize: 116,
      maxSize: 116,
    },
    {
      accessorKey: "from",
      header: t('search.from'),
      cell: ({ row }) => <LongText className='text-xs'>{row.original.from}</LongText>,
      meta: { className: 'text-left text-xs' },
      minSize: 150,
      maxSize: 156,
    },
    {
      accessorKey: "to",
      header: t('search.to'),
      cell: ({ row }) => <LongText className='text-xs'>{row.original.to.join(", ")}</LongText>,
      meta: { className: 'text-left text-xs' },
      minSize: 150,
      maxSize: 156,
    },
    {
      accessorKey: "subject",
      header: t('search.subject'),
      cell: ({ row }) => <LongText className='text-xs'>{row.original.subject}</LongText>,
      meta: { className: 'text-left text-xs' },
      minSize: 450,
      maxSize: 456,
    },
    {
      id: "text_preview",
      header: () => null,
      cell: ({ row }) => {
        const text = row.original.text

        if (!text) return null

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
              {text}
            </HoverCardContent>
          </HoverCard>
        )
      },
      meta: { className: "text-center max-w-[80px]" },
      minSize: 36,
      maxSize: 36,
      enableSorting: false,
    },
    {
      id: "attachment_count",
      header: () => <Paperclip size={16} />,
      cell: ({ row }) => <span className='text-xs'>{row.original.attachment_count}</span>,
      meta: { className: 'text-left text-xs' },
      minSize: 40,
      maxSize: 40
    },
    {
      accessorKey: 'size',
      header: ({ column }) => (
        <DataTableColumnHeader column={column} title={t('search.size')} />
      ),
      cell: ({ row }) => <span className='text-xs max-w-[40px]'>{formatBytes(row.original.size)}</span>,
      meta: { className: 'text-left text-xs' },
      minSize: 100,
      maxSize: 100,
    },
    {
      accessorKey: 'date',
      header: ({ column }) => (
        <DataTableColumnHeader column={column} title={t('search.date')} />
      ),
      cell: ({ row }) => {
        const date = new Date(row.original.date)
        const title = format(date, 'yyyy-MM-dd HH:mm:ss')
        return (
          <Tooltip>
            <TooltipTrigger asChild>
              <span className='text-xs whitespace-nowrap'>
                {formatDistanceToNow(date, { addSuffix: true, locale })}
              </span>
            </TooltipTrigger>
            <TooltipContent>{title}</TooltipContent>
          </Tooltip>
        )
      },
      meta: { className: 'text-left text-xs' },
      minSize: 100,
      maxSize: 100,
    },
    {
      id: 'actions',
      header: t('users.columns.actions'),
      cell: DataTableRowActions,
      meta: { className: 'text-left text-xs' },
      minSize: 50,
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

  const toggleSelected = (accountId: number, mailId: number) => {
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

  const hasSelected = (accountId: number, mailId: number) => selected.get(accountId)?.has(mailId) ?? false

  if (isLoading) {
    return (
      <div className="divide-y divide-border">
        {Array.from({ length: 8 }).map((_, i) => (
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
