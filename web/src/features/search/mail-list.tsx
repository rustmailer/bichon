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


import { cn, dateFnsLocaleMap, formatBytes } from "@/lib/utils"
import { formatDistanceToNow } from "date-fns"
import { MailIcon, MoreVertical, Paperclip, TagIcon, Trash2 } from "lucide-react"
import { Skeleton } from "@/components/ui/skeleton"
import { Checkbox } from "@/components/ui/checkbox"
import { EmailEnvelope } from "@/api"
import { useSearchContext } from "./context"
import { MailBulkActions } from "./bulk-actions"
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "@/components/ui/dropdown-menu"
import { Button } from "@/components/ui/button"
import { Badge } from "@/components/ui/badge"
import { useTranslation } from 'react-i18next'
import { enUS } from "date-fns/locale"

interface MailListProps {
    items: EmailEnvelope[]
    isLoading: boolean
    onEnvelopeChanged: (envelope: EmailEnvelope) => void
}

export function MailList({
    items,
    isLoading,
    onEnvelopeChanged
}: MailListProps) {
    const { t, i18n } = useTranslation()

    const locale = dateFnsLocaleMap[i18n.language.toLowerCase()] ?? enUS;
    const { setOpen, currentEnvelope, setCurrentEnvelope, selected, setSelected, setToDelete } = useSearchContext()

    const handleToggleAll = () => {
        const total = Array.from(selected.values())
            .reduce((sum, set) => sum + set.size, 0);

        if (total === items.length && items.length > 0) {
            setSelected(new Map());
        } else {
            setSelected(prev => {
                const next = new Map(prev);
                for (const item of items) {
                    const set = new Set(next.get(item.account_id) || []);
                    set.add(item.id);
                    next.set(item.account_id, set);
                }
                return next;
            });
        }
    }

    const toggleToDelete = (accountId: number, mailId: number) => {
        setToDelete(prev => {
            const next = new Map(prev);
            const set = new Set(next.get(accountId) || []);

            if (set.has(mailId)) {
                set.delete(mailId);
                if (set.size === 0) next.delete(accountId);
                else next.set(accountId, set);
            } else {
                set.add(mailId);
                next.set(accountId, set);
            }

            return next;
        });
    };

    const toggleSelected = (accountId: number, mailId: number) => {
        setSelected(prev => {
            const next = new Map(prev);
            const set = new Set(next.get(accountId) || []);

            if (set.has(mailId)) {
                set.delete(mailId);
                if (set.size === 0) next.delete(accountId);
                else next.set(accountId, set);
            } else {
                set.add(mailId);
                next.set(accountId, set);
            }

            return next;
        });
    }

    const totalSelected = Array.from(selected.values())
        .reduce((sum, set) => sum + set.size, 0);

    const hasSelected = (accountId: number, mailId: number) => {
        return selected.get(accountId)?.has(mailId) ?? false;
    }

    const handleDelete = (envelope: EmailEnvelope) => {
        setToDelete(new Map());
        toggleToDelete(envelope.account_id, envelope.id)
        setOpen("delete")
    }

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
        <div className="divide-y divide-border">
            {items.length > 0 && (
                <div className="flex items-center gap-2 px-2 py-1 bg-muted/30">
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
                    <span className="text-xs text-muted-foreground">
                        {totalSelected > 0
                            ? `${totalSelected} ${t('common.selected')}`
                            : t('common.selectAll')}
                    </span>
                </div>
            )}

            {items.map((item, index) => {
                const hasAttachments = item.attachments && item.attachments.length > 0
                const isSelectedRow = currentEnvelope?.id === item.id
                const isChecked = hasSelected(item.account_id, item.id)

                return (
                    <div
                        key={index}
                        className={cn(
                            "flex items-center gap-2 px-2 py-1.5 cursor-pointer transition-colors",
                            "hover:bg-accent/50",
                            isSelectedRow && "bg-accent"
                        )}
                        onClick={(e) => {
                            const target = e.target as HTMLElement
                            if (target.closest('input[type="checkbox"], button')) return
                            onEnvelopeChanged(item)
                        }}
                    >
                        <Checkbox
                            checked={isChecked}
                            onCheckedChange={() => toggleSelected(item.account_id, item.id)}
                            onClick={(e) => e.stopPropagation()}
                            className="h-4 w-4 shrink-0"
                        />

                        <MailIcon className="h-3.5 w-3.5 text-muted-foreground shrink-0" />
                        <div className="flex-1 min-w-0 grid grid-cols-1 sm:grid-cols-12 gap-1 sm:gap-0">

                            <div className="col-span-1 sm:col-span-8 flex flex-col min-w-0 gap-0.5">
                                <div className="flex items-center gap-1 min-w-0">
                                    <p className="text-sm font-medium truncate">{item.from}</p>
                                    <h3 className="text-sm text-muted-foreground truncate hidden sm:block">
                                        {item.subject}
                                    </h3>
                                </div>
                                <div className="flex items-center gap-1.5 text-[10px] text-muted-foreground/60">
                                    <span className="truncate">{item.account_email}</span>
                                    <span className="scale-75 opacity-50">•</span>
                                    <span className="font-medium text-primary/70">{item.mailbox_name}</span>
                                </div>
                                <h3 className="text-sm text-muted-foreground truncate sm:hidden">
                                    {item.subject}
                                </h3>

                                <div className="flex flex-wrap gap-1 mt-0.25">
                                    {item.tags?.map((tag, i) => (
                                        <Badge className="px-1 py-0.5 text-[10px] h-auto leading-none" key={i}>{tag}</Badge>
                                    ))}
                                </div>
                            </div>
                            <div className="col-span-1 sm:col-span-4 flex items-center justify-end gap-1 text-xs text-muted-foreground">

                                {hasAttachments && (
                                    <div className="flex items-center gap-1">
                                        <Paperclip className="h-3 w-3" />
                                        <span>{item.attachments?.length}</span>
                                    </div>
                                )}

                                <span className="hidden md:inline">{formatBytes(item.size)}</span>

                                <span className={cn(isSelectedRow ? "text-foreground font-medium" : "text-muted-foreground")}>
                                    {item.date && formatDistanceToNow(new Date(item.date), { addSuffix: true, locale })}
                                </span>

                                <DropdownMenu>
                                    <DropdownMenuTrigger asChild>
                                        <Button
                                            variant="ghost"
                                            size="icon"
                                            className="h-6 w-6 p-0 hover:bg-muted rounded-md"
                                            onClick={(e) => e.stopPropagation()}
                                        >
                                            <MoreVertical className="h-3 w-3" />
                                        </Button>
                                    </DropdownMenuTrigger>

                                    <DropdownMenuContent align="end" className="w-44">
                                        <DropdownMenuItem
                                            onClick={(e) => e.stopPropagation()}
                                            onSelect={(e) => {
                                                e.stopPropagation();
                                                setCurrentEnvelope(item);
                                                setOpen("edit-tags");
                                            }}
                                        >
                                            <TagIcon className="ml-2 h-3.5 w-3.5" />
                                            {t('search.editTag')}
                                        </DropdownMenuItem>
                                        <DropdownMenuItem
                                            onClick={(e) => e.stopPropagation()}
                                            onSelect={(e) => {
                                                e.stopPropagation();
                                                setCurrentEnvelope(item);
                                                setOpen("restore");
                                            }}
                                        >
                                            <TagIcon className="ml-2 h-3.5 w-3.5" />
                                            {t('restore_message.restore_to_imap', 'Restore Mail')}
                                        </DropdownMenuItem>
                                        <DropdownMenuItem
                                            className="text-destructive focus:text-destructive"
                                            onClick={(e) => e.stopPropagation()}
                                            onSelect={(e) => {
                                                e.stopPropagation();
                                                handleDelete(item);
                                            }}
                                        >
                                            <Trash2 className="ml-2 h-3.5 w-3.5" />
                                            {t('common.delete')}
                                        </DropdownMenuItem>
                                    </DropdownMenuContent>
                                </DropdownMenu>
                            </div>
                        </div>
                    </div>
                )
            })}
            {totalSelected > 0 && <MailBulkActions />}
        </div>
    )
}
