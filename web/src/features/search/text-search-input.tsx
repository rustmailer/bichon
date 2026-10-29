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

import React, { useState, useEffect, useRef } from "react"
import { Input } from "@/components/ui/input"
import { Button } from "@/components/ui/button"
import { Search, X, Clock, Trash2 } from "lucide-react"
import { cn } from "@/lib/utils"
import { useSearchContext } from "./context"
import { useTranslation } from "react-i18next"
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select"

const STORAGE_KEY = "bichon_mail_search_history"
const MAX_HISTORY = 20

type SearchField = "text" | "subject" | "body"
const SEARCH_FIELDS: SearchField[] = ["text", "subject", "body"]

export function TextSearchInput() {
    const { t } = useTranslation()
    const { filter, setFilter } = useSearchContext()

    const [value, setValue] = useState("")
    const [field, setField] = useState<SearchField>("text")
    const [history, setHistory] = useState<string[]>([])
    const [showHistory, setShowHistory] = useState(false)

    const inputRef = useRef<HTMLInputElement>(null)
    const containerRef = useRef<HTMLDivElement>(null)

    useEffect(() => {
        const activeField = SEARCH_FIELDS.find(key => !!filter[key]) || "text"
        const activeValue = filter[activeField] as string || ""

        setField(activeField)
        setValue(activeValue)
    }, [filter])

    useEffect(() => {
        try {
            const saved = localStorage.getItem(STORAGE_KEY)
            if (saved) setHistory(JSON.parse(saved))
        } catch (err) {
            console.warn("Failed to load search history", err)
        }
    }, [])

    const applyFilter = (currentField: SearchField, searchTerm: string) => {
        const trimmed = searchTerm.trim()

        setFilter((prev) => {
            const next = { ...prev }
            SEARCH_FIELDS.forEach(f => {
                delete next[f]
            })
            if (trimmed) {
                next[currentField] = trimmed
            }
            return next
        })

        if (trimmed) {
            saveToHistory(trimmed)
        }
        setShowHistory(false)
        inputRef.current?.blur()
    }

    const saveToHistory = (term: string) => {
        setHistory((prev) => {
            const trimmed = term.trim()
            const newHistory = [trimmed, ...prev.filter((item) => item !== trimmed)].slice(0, MAX_HISTORY)
            localStorage.setItem(STORAGE_KEY, JSON.stringify(newHistory))
            return newHistory
        })
    }

    const handleSearch = () => applyFilter(field, value)

    const handleClear = () => {
        setValue("")
        applyFilter(field, "")
    }

    const handleSelectHistory = (term: string) => {
        setValue(term)
        applyFilter(field, term)
    }

    const handleClearHistory = (e: React.MouseEvent) => {
        e.stopPropagation()
        setHistory([])
        localStorage.removeItem(STORAGE_KEY)
    }

    useEffect(() => {
        const handleClickOutside = (e: MouseEvent) => {
            if (containerRef.current && !containerRef.current.contains(e.target as Node)) {
                setShowHistory(false)
            }
        }
        document.addEventListener("mousedown", handleClickOutside)
        return () => document.removeEventListener("mousedown", handleClickOutside)
    }, [])

    return (
        <div ref={containerRef} className="relative w-full max-w-[620px] min-w-[320px]">
            <div className="flex items-center rounded-md border bg-background focus-within:border-primary/50 focus-within:ring-1 focus-within:ring-primary/30 transition-all">
                <Select
                    value={field}
                    onValueChange={(val) => {
                        const newField = val as SearchField
                        setField(newField)
                        if (value.trim()) applyFilter(newField, value)
                    }}
                >
                    <SelectTrigger
                        className={cn(
                            "h-9 w-[110px] md:w-[130px] border-r border-border rounded-r-none",
                            "text-xs md:text-xs bg-transparent focus:ring-0 focus:ring-offset-0 shadow-none border-y-0 border-l-0"
                        )}
                    >
                        <SelectValue/>
                    </SelectTrigger>
                    <SelectContent className="min-w-[240px]">
                        <SelectItem value="text" className="font-medium cursor-pointer text-xs">
                            {t("search_input.all")}
                            <p className="text-[11px] text-muted-foreground/60 leading-relaxed">
                                {t("search_input.all_fields_desc")}
                            </p>
                        </SelectItem>
                        <SelectItem value="subject" className="cursor-pointer text-xs">
                            {t("search_input.subject")}
                        </SelectItem>
                        <SelectItem value="body" className="cursor-pointer text-xs">
                            {t("search_input.body")}
                        </SelectItem>
                    </SelectContent>
                </Select>
                <div className="relative flex-1">
                    <Search className="absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
                    <Input
                        ref={inputRef}
                        value={value}
                        onChange={(e) => setValue(e.target.value)}
                        onFocus={() => setShowHistory(true)}
                        onKeyDown={(e) => e.key === "Enter" && handleSearch()}
                        placeholder={t("search_input.placeholder")}
                        className="h-9 border-none shadow-none focus-visible:ring-0 pl-9 pr-10 text-xs md:text-xs bg-transparent w-full"
                    />
                    {value && (
                        <Button
                            variant="ghost"
                            size="icon"
                            className="absolute right-1 top-1/2 h-7 w-7 -translate-y-1/2 text-muted-foreground hover:text-foreground"
                            onClick={handleClear}
                        >
                            <X className="h-4 w-4" />
                        </Button>
                    )}
                </div>
                <Button
                    size="sm"
                    className="h-7 mr-1.5 px-3 text-xs md:px-5 md:text-sm"
                    onClick={handleSearch}
                    disabled={!value.trim()}
                >
                    {t("search_input.button")}
                </Button>
            </div>
            <p className="mt-0.5 px-1 text-[10px] leading-relaxed text-muted-foreground/70">
                {t("search_input.hint")}
            </p>
            {showHistory && (
                <div className="absolute top-full left-0 w-full mt-1 bg-popover border rounded-md shadow-lg z-50 max-h-[300px] overflow-hidden flex flex-col">
                    <div className="py-2 px-3 text-[10px] uppercase tracking-wider text-muted-foreground font-semibold border-b flex items-center justify-between bg-muted/30">
                        <div className="flex items-center gap-1.5">
                            <Clock className="h-3 w-3" />
                            {t("search_input.recent_title")}
                        </div>
                        {history.length > 0 && (
                            <button
                                onClick={handleClearHistory}
                                className="text-destructive hover:underline flex items-center gap-1"
                            >
                                <Trash2 className="h-3 w-3" />
                                {t("search_input.clear_history")}
                            </button>
                        )}
                    </div>

                    <div className="overflow-auto py-1">
                        {history.length > 0 ? (
                            history.map((term, idx) => (
                                <button
                                    key={idx}
                                    className="w-full text-left px-3 py-2 text-sm hover:bg-accent transition-colors flex items-center gap-2 group"
                                    onClick={() => handleSelectHistory(term)}
                                >
                                    <Search className="h-3.5 w-3.5 text-muted-foreground group-hover:text-primary" />
                                    <span className="truncate flex-1 text-xs">{term}</span>
                                </button>
                            ))
                        ) : (
                            <div className="px-3 py-6 text-sm text-center text-muted-foreground">
                                {t("search_input.no_history")}
                            </div>
                        )}
                    </div>
                </div>
            )}
        </div>
    )
}