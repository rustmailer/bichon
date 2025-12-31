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
import { EmailEnvelope } from "@/api"
import { useMailboxContext } from "../context"
import { Checkbox } from "@/components/ui/checkbox"
import { MailBulkActions } from "./bulk-actions"
import { Badge } from "@/components/ui/badge"
import { useTranslation } from 'react-i18next'
import { enUS } from "date-fns/locale"
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "@/components/ui/dropdown-menu"
import { Button } from "@/components/ui/button"

interface MailListProps {
    items: EmailEnvelope[]
    isLoading: boolean
}

export function MailList({
    items,
    isLoading,
}: MailListProps) {
    const { t, i18n } = useTranslation()
    const { currentEnvelope, setCurrentEnvelope, setDeleteIds, setOpen, selected, setSelected } = useMailboxContext()
    const locale = dateFnsLocaleMap[i18n.language.toLowerCase()] ?? enUS;

    const handleDelete = (envelope: EmailEnvelope) => {
        setDeleteIds(new Set([envelope.id]))
        setOpen("move-to-trash")
    }

    const totalSelected = selected.size;

    const handleToggleAll = () => {
        const total = selected.size;
        if (total === items.length && items.length > 0) {
            setSelected(new Set<number>());
        } else {
            const set = new Set<number>();
            for (const item of items) {
                set.add(item.id);
            }
            setSelected(set);
        }
    }

    const hasSelected = (mailId: number) => {
        return selected.has(mailId);
    }

    const toggleSelected = (id: number) => {
        setSelected(prev => {
            const next = new Set(prev)
            if (next.has(id)) {
                next.delete(id)
            } else {
                next.add(id)
            }
            return next
        });
    }

    if (isLoading) {
        return (
            <div className="divide-y divide-border">
                {Array.from({ length: 8 }).map((_, i) => (
                    <div key={i} className="flex items-center gap-2 px-2 py-1.5">
                        <Skeleton className="h-3 w-3 rounded-full" />
                        <Skeleton className="h-3 flex-1 max-w-xs" />
                        <Skeleton className="h-2.5 w-12 ml-auto" />
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
                            selected.size === items.length && items.length > 0
                                ? true
                                : selected.size > 0
                                    ? "indeterminate"
                                    : false
                        }
                        onCheckedChange={handleToggleAll}
                        className="h-4 w-4"
                    />
                    <span className="text-xs text-muted-foreground">
                        {selected.size > 0
                            ? `${selected.size} ${t('common.selected')}`
                            : t('common.selectAll')}
                    </span>
                </div>
            )}

            {items.map((item, index) => {
                const hasAttachments = item.attachments && item.attachments.length > 0
                const isSelected = currentEnvelope?.id === item.id
                const isChecked = hasSelected(item.id)
                return (
                    <div
                        key={index}
                        className={cn(
                            "group flex items-center gap-2 px-2 py-1.5 cursor-pointer transition-colors",
                            "hover:bg-accent/50",
                            isSelected && "bg-accent"
                        )}
                        onClick={(e) => {
                            const target = e.target as HTMLElement
                            if (target.closest('input[type="checkbox"], button')) return
                            setCurrentEnvelope(item);
                            setOpen("display")
                        }}
                    >
                        <Checkbox
                            checked={isChecked}
                            onCheckedChange={() => toggleSelected(item.id)}
                            onClick={(e) => e.stopPropagation()}
                            className="h-4 w-4 shrink-0"
                        />
                        <MailIcon className="h-3.5 w-3.5 text-muted-foreground shrink-0" />
                        <div className="flex-1 min-w-0 grid grid-cols-1 sm:grid-cols-12 gap-1 sm:gap-0">
                            <div className="col-span-1 sm:col-span-8 flex flex-col min-w-0 gap-1">
                                <div className="flex items-center gap-1 min-w-0">
                                    <p className="text-sm font-medium truncate">{item.from}</p>
                                    <h3 className="text-sm text-muted-foreground truncate hidden sm:block">
                                        {item.subject}
                                    </h3>
                                </div>
                                <h3 className="text-sm text-muted-foreground truncate sm:hidden">
                                    {item.subject}
                                </h3>

                                <div className="flex flex-wrap gap-1 mt-0.25">
                                    {item.tags?.map((tag, i) => (
                                        <Badge
                                            key={i}
                                            className="px-1 py-0.5 text-[10px] h-auto leading-none"
                                        >
                                            {tag}
                                        </Badge>
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
                                <span className={cn(
                                    isSelected ? "text-foreground font-medium" : "text-muted-foreground"
                                )}>
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
                                                setSelected(new Set([item.id]));
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
