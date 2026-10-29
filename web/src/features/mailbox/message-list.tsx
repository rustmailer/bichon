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

import { format } from 'date-fns'
import { useTranslation } from 'react-i18next'

import { EmailEnvelope } from '@/api'
import { Skeleton } from '@/components/ui/skeleton'
import { ScrollArea } from '@/components/ui/scroll-area'
import { cn, formatSender } from '@/lib/utils'

interface Props {
  items: EmailEnvelope[]
  isLoading: boolean
  activeEnvelope?: EmailEnvelope
  onSelect: (envelope: EmailEnvelope) => void
  /** Empty-state text; the page uses it to distinguish "empty folder" from
   *  "nothing matches the current search". */
  emptyMessage?: string
}

/** First two letters of the sender's local part, e.g. `john.doe@x` -> `JO`. */
function avatarInitials(from: string | undefined): string {
  const local = (from ?? '').split('@')[0] ?? ''
  const letters = local.replace(/[^a-zA-Z0-9]/g, '')
  return letters.slice(0, 2).toUpperCase() || '?'
}

/** Stable pastel colour for an address, so the same sender keeps its hue. */
function avatarStyle(seed: string | undefined) {
  const text = seed ?? ''
  let hash = 0
  for (let index = 0; index < text.length; index++) {
    hash = (hash * 31 + text.charCodeAt(index)) | 0
  }
  const hue = Math.abs(hash) % 360
  return {
    backgroundColor: `hsl(${hue} 70% 90%)`,
    color: `hsl(${hue} 55% 30%)`,
  }
}

/** Full timestamp for the item's top-right corner. */
function formatMessageDate(date: Date): string {
  if (Number.isNaN(date.getTime())) return ''
  return format(date, 'yyyy-MM-dd HH:mm:ss')
}

/**
 * Mail-client style list: avatar, sender and date, subject, one-line preview.
 * Owned by the mailbox view; the search results table is untouched.
 */
export function MessageList({
  items,
  isLoading,
  activeEnvelope,
  onSelect,
  emptyMessage,
}: Props) {
  const { t } = useTranslation()

  if (isLoading) {
    return (
      <div className="min-h-0 flex-1 divide-y overflow-hidden rounded-md border">
        {Array.from({ length: 12 }).map((_, index) => (
          <div key={index} className="flex items-start gap-3 px-3 py-2">
            <Skeleton className="h-8 w-8 rounded-full" />
            <div className="flex-1 space-y-1.5">
              <Skeleton className="h-3 w-1/2" />
              <Skeleton className="h-3 w-4/5" />
              <Skeleton className="h-2.5 w-full" />
            </div>
          </div>
        ))}
      </div>
    )
  }

  if (items.length === 0) {
    return (
      <div className="flex min-h-0 flex-1 items-center justify-center rounded-md border px-4 text-center text-xs text-muted-foreground">
        {emptyMessage ?? t('mailboxView.empty')}
      </div>
    )
  }

  return (
    // Radix wraps ScrollArea content in a `display: table` element that grows
    // to the widest row instead of being constrained by the pane. That clipped
    // the right side (including the date) and defeated `truncate`. The
    // arbitrary variant below forces that wrapper back to a block so rows are
    // exactly as wide as the list.
    <ScrollArea className="min-h-0 w-full flex-1 rounded-md border [&_[data-radix-scroll-area-viewport]>div]:!block">
      <ul className="w-full divide-y">
        {items.map((envelope) => {
          const active =
            activeEnvelope?.account_id === envelope.account_id &&
            activeEnvelope?.id === envelope.id
          const date = new Date(envelope.date)

          return (
            <li key={`${envelope.account_id}:${envelope.id}`}>
              <button
                type="button"
                onClick={() => onSelect(envelope)}
                className={cn(
                  'flex w-full min-w-0 items-start gap-3 px-3 py-2 text-left transition-colors hover:bg-accent/50',
                  active && 'bg-accent/70'
                )}
              >
                <span
                  className="mt-0.5 flex h-8 w-8 shrink-0 items-center justify-center rounded-full text-[11px] font-semibold"
                  style={avatarStyle(envelope.from)}
                >
                  {avatarInitials(envelope.from)}
                </span>

                <span className="min-w-0 flex-1 overflow-hidden">
                  <span className="flex items-baseline justify-between gap-2">
                    <span
                      className="min-w-0 truncate text-xs font-medium"
                      title={envelope.wire_from ? `via ${envelope.wire_from}` : undefined}
                    >
                      {formatSender(envelope.from_name, envelope.from)}
                    </span>
                    <span className="shrink-0 whitespace-nowrap text-[10px] tabular-nums text-muted-foreground">
                      {formatMessageDate(date)}
                    </span>
                  </span>
                  <span
                    className="mt-0.5 block w-full max-w-full truncate text-xs font-semibold"
                    title={envelope.subject}
                  >
                    {envelope.subject || t('mailboxView.noSubject')}
                  </span>
                  {envelope.preview && (
                    <span className="mt-0.5 block w-full max-w-full truncate text-[11px] text-muted-foreground">
                      {envelope.preview}
                    </span>
                  )}
                </span>
              </button>
            </li>
          )
        })}
      </ul>
    </ScrollArea>
  )
}
