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

import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover"
import { useSearchContext } from "./context"
import { Button } from "@/components/ui/button"
import { cn } from "@/lib/utils"
import { Check, ChevronDown, Mail, X } from "lucide-react"
import React from "react"
import { useContacts } from "@/hooks/use-contacts"
import { useTranslation } from 'react-i18next'
import {
    Command,
    CommandEmpty,
    CommandGroup,
    CommandInput,
    CommandItem,
} from "@/components/ui/command"
import { ScrollArea } from "@/components/ui/scroll-area"

export function MailFilterPopover() {
    const { t } = useTranslation()
    const { filter, setFilter } = useSearchContext()
    const fields = ['from', 'to', 'cc', 'bcc', 'any_recipient', 'any_participant'] as const

    const activeCount = fields.filter(k => !!filter[k]).length

    const updateFilter = (field: string, email: string | undefined) => {
        setFilter(prev => ({
            ...prev,
            [field]: email
        }))
    }

    const resetAll = () => {
        setFilter(prev => {
            const next = { ...prev }
            fields.forEach(k => delete next[k])
            return next
        })
    }

    return (
        <Popover>
            <PopoverTrigger asChild>
                <Button
                    size="sm"
                    variant="outline"
                    className={cn(
                        'h-6 rounded-none px-3 gap-1.5 transition-colors border-l-0',
                        activeCount > 0 && 'bg-primary/10 text-primary hover:bg-primary/20'
                    )}
                >
                    <Mail className="h-3.5 w-3.5 opacity-60" />
                    <span>
                        {activeCount > 0
                            ? t('search_contacts.label_with_count', { count: activeCount })
                            : t('search_contacts.label')}
                    </span>
                    <ChevronDown className="h-3.5 w-3.5 opacity-60" />
                </Button>
            </PopoverTrigger>

            <PopoverContent
                align="start"
                className="w-fit min-w-[280px] max-w-[90vw] sm:max-w-[min(90vw,500px)] p-0 flex flex-col divide-y divide-border shadow-xl"
            >
                <div className="flex flex-col bg-muted/20 divide-y divide-border">
                    {fields.map((field) => (
                        <ContactSelectorField
                            key={field}
                            label={t(`search.${field}`)}
                            value={filter[field] as string | undefined}
                            onSelect={(email) => updateFilter(field, email)}
                            onReset={() => updateFilter(field, undefined)}
                        />
                    ))}
                </div>

                {activeCount > 0 && (
                    <div className="px-1 pb-2">
                        <Button
                            variant="ghost"
                            size="sm"
                            onClick={resetAll}
                            className="flex h-8 w-full items-center justify-start gap-2 px-2 text-xs font-medium text-destructive hover:bg-destructive/10 hover:text-destructive transition-colors"
                        >
                            <div className="flex h-4 w-4 items-center justify-center">
                                <X className="h-3.5 w-3.5" />
                            </div>
                            <span className="flex-1 text-left">
                                {t('search_contacts.reset_all')}
                            </span>
                        </Button>
                    </div>
                )}
            </PopoverContent>
        </Popover>
    )
}

function ContactSelectorField({
    label,
    value,
    onSelect,
    onReset
}: {
    label: string
    value?: string
    onSelect: (email: string | undefined) => void
    onReset: () => void
}) {
    const { t } = useTranslation()
    const [searchTerm, setSearchTerm] = React.useState("")
    const { contacts, isLoading, isError } = useContacts(searchTerm)
    // The archive only knows display names for senders; fall back to the bare
    // address when the chosen contact has none.
    const selectedName = value
        ? contacts.find((contact) => contact.email === value)?.name
        : undefined

    const handleToggle = (email: string) => {
        if (value === email) {
            onReset()
        } else {
            onSelect(email)
        }
    }

    return (
        <Popover>
            <PopoverTrigger asChild>
                <button
                    className={cn(
                        "group flex items-center justify-between w-full px-4 py-3 hover:bg-background transition-all text-left relative",
                        "min-h-[52px]",
                        value && "bg-background/60 hover:bg-background/80"
                    )}
                >
                    <div className="flex flex-col items-start pr-6">
                        <span className="text-xs font-bold uppercase opacity-50 tracking-tight leading-none">
                            {label}
                        </span>
                        <span
                            title={value}
                            className={cn(
                                "mt-0.5 truncate max-w-[320px]",
                                value
                                    ? "text-xs font-semibold text-primary"
                                    : "text-xs text-muted-foreground/90"
                            )}
                        >
                            {value
                                ? selectedName
                                    ? `${selectedName} <${value}>`
                                    : value
                                : t('search_contacts.any')}
                        </span>
                    </div>

                    <div className="flex items-center gap-1.5 opacity-0 group-hover:opacity-100 transition-opacity">
                        {value && (
                            <button
                                type="button"
                                onClick={(e) => {
                                    e.stopPropagation()
                                    onReset()
                                }}
                                className="p-1 rounded-full hover:bg-destructive/10 text-muted-foreground hover:text-destructive"
                            >
                                <X className="h-3.5 w-3.5" />
                            </button>
                        )}
                    </div>

                    {value && (
                        <div className="absolute bottom-0 left-0 right-0 h-0.5 bg-primary" />
                    )}
                </button>
            </PopoverTrigger>

            <PopoverContent
                side="right"
                align="start"
                className="p-0 w-auto min-w-[300px] max-w-[420px] shadow-2xl border-border/50"
            >
                <Command shouldFilter={false}>
                    <CommandInput
                        placeholder={t('search_contacts.search_placeholder', { field: label })}
                        className="h-9"
                        value={searchTerm}
                        onValueChange={setSearchTerm}
                    />
                    <ScrollArea className="h-[360px]">
                        {isLoading && (
                            <div className="p-4 text-xs text-center opacity-50">{t('search_contacts.loading')}</div>
                        )}
                        {isError && (
                            <div className="p-4 text-xs text-center text-destructive">
                                {t('address_select.loadFailed', 'Could not load contacts.')}
                            </div>
                        )}
                        {!isError && (
                            <CommandEmpty>{t('search_contacts.no_contact_found')}</CommandEmpty>
                        )}
                        <CommandGroup>
                            {contacts.slice(0, 100).map((contact) => (
                                <CommandItem
                                    key={contact.email}
                                    onSelect={() => handleToggle(contact.email)}
                                    className="flex items-center justify-between py-2.5 px-3 cursor-pointer whitespace-nowrap gap-4 text-xs"
                                >
                                    <div className="flex flex-col min-w-0">
                                        <span className="font-medium">
                                            {contact.name || contact.email.split('@')[0]}
                                        </span>
                                        <span className="text-xs text-muted-foreground truncate max-w-[360px]">
                                            {contact.email}
                                        </span>
                                    </div>
                                    {value === contact.email && (
                                        <Check className="h-4 w-4 text-primary shrink-0" />
                                    )}
                                </CommandItem>
                            ))}
                            {contacts.length > 100 && (
                                <div className="px-3 py-2 text-xs text-center text-muted-foreground border-t border-border/50">
                                    {t('search_contacts.showing_limit', { total: contacts.length })}
                                </div>
                            )}
                        </CommandGroup>
                    </ScrollArea>
                </Command>
            </PopoverContent>
        </Popover>
    )
}