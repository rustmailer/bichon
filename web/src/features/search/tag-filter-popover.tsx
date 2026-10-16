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
import { Tag, ChevronDown, X } from 'lucide-react'
import { useTranslation } from 'react-i18next'

import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { ScrollArea } from '@/components/ui/scroll-area'
import {
    Popover,
    PopoverContent,
    PopoverTrigger,
} from '@/components/ui/popover'

import { useAvailableTags } from '@/hooks/use-available-tags'
import { cn } from '@/lib/utils'
import { useSearchContext } from './context'
import {
    clearTagFilter,
    countTagFilters,
    getSelectedTags,
    isUntaggedOnly,
    toggleNoTags,
    toggleTag,
} from './tag-filter'

export function TagFilterPopover() {
    const { t } = useTranslation()
    const [search, setSearch] = React.useState('')
    const { filter, setFilter } = useSearchContext()

    const selectedTags = getSelectedTags(filter)
    const noTags = isUntaggedOnly(filter)
    const activeCount = countTagFilters(filter)
    const {
        tagsCount = [],
        isLoading,
    } = useAvailableTags()

    const handleTagToggle = (tag: string) => setFilter(prev => toggleTag(prev, tag))

    const handleNoTagsToggle = () => setFilter(prev => toggleNoTags(prev))

    const clearAllTags = () => setFilter(prev => clearTagFilter(prev))

    const filteredTags = React.useMemo(() => {
        const q = search.toLowerCase()

        return tagsCount
            .filter(t =>
                !q || t.tag.toLowerCase().includes(q)
            )
            .sort((a, b) => {
                const aSelected = selectedTags.includes(a.tag)
                const bSelected = selectedTags.includes(b.tag)
                if (aSelected && !bSelected) return -1
                if (!aSelected && bSelected) return 1
                return b.count - a.count
            })
    }, [tagsCount, search, selectedTags])

    return (
        <Popover>
            <PopoverTrigger asChild>
                <Button
                    size="sm"
                    variant="outline"
                    className={cn(
                        'h-6 gap-1.5 px-3 rounded-none border-l-0',
                        activeCount > 0 &&
                        'bg-primary/10 border-primary text-primary'
                    )}
                >
                    <Tag className="h-4 w-4" />
                    {t('tag.label')}
                    {activeCount > 0 && (
                        <Badge
                            variant="secondary"
                            className="ml-1 h-5 px-1.5 text-xs"
                        >
                            {activeCount}
                        </Badge>
                    )}
                    <ChevronDown className="h-3.5 w-3.5 opacity-60" />
                </Button>
            </PopoverTrigger>

            <PopoverContent
                align="start"
                className="w-96 p-1"
            >
                <div className="p-1 pb-2">
                    <Input
                        value={search}
                        onChange={(e) => setSearch(e.target.value)}
                        placeholder={t('tag.search_placeholder')}
                        className="h-8 text-sm"
                        autoFocus
                    />
                </div>
                <ScrollArea className="h-96 p-1">
                    {/*
                     * Pinned above the searchable list: "no tags" is not a tag
                     * value from the index, so matching the list against the
                     * search box must not hide it.
                     */}
                    <div
                        onClick={handleNoTagsToggle}
                        className="flex items-center gap-2 px-2 py-1.5 rounded-md cursor-pointer hover:bg-accent transition-colors"
                    >
                        <Checkbox
                            id="no-tags-option"
                            checked={noTags}
                            onCheckedChange={handleNoTagsToggle}
                            onClick={(e) => e.stopPropagation()}
                        />

                        <Label
                            htmlFor="no-tags-option"
                            className="flex-1 truncate text-xs cursor-pointer"
                        >
                            {t('tag.no_tags')}
                        </Label>
                    </div>
                    <div className="my-1 h-px bg-border" />
                    {!search && activeCount > 0 && (
                        <>
                            <div
                                onClick={clearAllTags}
                                className="flex items-center gap-2 px-2 py-1.5 rounded-md cursor-pointer text-destructive hover:bg-destructive/10 transition-colors"
                            >
                                <div className="flex h-4 w-4 items-center justify-center">
                                    <X className="h-3 w-3" />
                                </div>
                                <span className="flex-1 text-xs font-medium">
                                    {t('tag.clear_all')}
                                </span>
                                <span className="text-[10px] opacity-60">({activeCount})</span>
                            </div>
                            <div className="my-1 h-px bg-border" />
                        </>
                    )}
                    {isLoading ? (
                        <div className="space-y-2 p-2">
                            {Array.from({ length: 6 }).map((_, i) => (
                                <div
                                    key={i}
                                    className="h-4 rounded bg-muted animate-pulse"
                                />
                            ))}
                        </div>
                    ) : filteredTags.length === 0 ? (
                        <p className="px-3 py-2 text-xs text-muted-foreground">
                            {t('tag.no_tags_found')}
                        </p>
                    ) : (
                        filteredTags.map(({ tag, count }) => {
                            const checked = selectedTags.includes(tag)
                            const id = `tag-${tag}`

                            return (
                                <div
                                    key={tag}
                                    onClick={() => handleTagToggle(tag)}
                                    className={cn(
                                        'flex items-center gap-2 px-2 py-1.5 rounded-md cursor-pointer',
                                        'hover:bg-accent transition-colors'
                                    )}
                                >
                                    <Checkbox
                                        id={id}
                                        checked={checked}
                                        onCheckedChange={() =>
                                            handleTagToggle(tag)
                                        }
                                        onClick={(e) =>
                                            e.stopPropagation()
                                        }
                                    />

                                    <Label
                                        htmlFor={id}
                                        className="flex-1 truncate text-xs cursor-pointer"
                                        title={tag}
                                    >
                                        {tag}
                                    </Label>

                                    <Badge
                                        variant="secondary"
                                        className="h-5 px-1.5 text-xs"
                                    >
                                        {count}
                                    </Badge>
                                </div>
                            )
                        })
                    )}
                </ScrollArea>
            </PopoverContent>
        </Popover>
    )
}