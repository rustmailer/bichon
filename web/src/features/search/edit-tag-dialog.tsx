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

import { Button } from '@/components/ui/button';
import { Dialog, DialogContent, DialogHeader, DialogTitle } from '@/components/ui/dialog';
import { Badge } from '@/components/ui/badge';
import { Input } from '@/components/ui/input';
import { Plus, Tag as TagIcon, X, Loader2, Check, Search } from 'lucide-react';
import { useState, useEffect } from 'react';
import { useAvailableTags } from '@/hooks/use-available-tags';
import { useUpdateTags } from '@/hooks/use-update-tags';
import { toast } from '@/hooks/use-toast';
import { validateTag } from '@/lib/utils';
import { useTranslation } from 'react-i18next';
import { useQueryClient } from '@tanstack/react-query';
import { useSearchContext } from './context';
import { ScrollArea } from '@/components/ui/scroll-area';

interface Props {
    open: boolean
    onOpenChange: (open: boolean) => void
}

export function EditTagsDialog({ open, onOpenChange }: Props) {
    const { tags: availableTags } = useAvailableTags();
    const queryClient = useQueryClient();
    const { mutate, isPending } = useUpdateTags();
    const [selectedTags, setSelectedTags] = useState<string[]>([]);
    const [inputValue, setInputValue] = useState('');
    const [commandOpen, setCommandOpen] = useState(false);
    const { t } = useTranslation();

    const { currentEnvelope, setCurrentEnvelope } = useSearchContext()

    useEffect(() => {
        if (open && currentEnvelope) {
            setSelectedTags(currentEnvelope.tags || []);
        }
    }, [open, currentEnvelope]);

    if (!currentEnvelope) return null;

    const handleAddTag = (tag: string) => {
        const normalized = tag.toLowerCase().trim();
        const result = validateTag(normalized);
        if (!result.valid) {
            toast({
                title: t('search.addTags.invalidTitle'),
                description: result.error,
                variant: 'destructive',
            });
            return;
        }
        if (normalized && !selectedTags.includes(normalized)) {
            setSelectedTags(prev => [...prev, normalized]);
        }
        setInputValue('');
        setCommandOpen(false);
    };

    const handleRemoveTag = (tag: string) => {
        setSelectedTags(prev => prev.filter(t => t !== tag));
    };

    const handleSave = () => {
        if (inputValue.trim()) {
            const normalized = inputValue.toLowerCase().trim();
            const result = validateTag(normalized);

            if (!result.valid) {
                toast({
                    title: t('search.addTags.invalidTitle'),
                    description: result.error,
                    variant: 'destructive',
                });
                return;
            }

            if (!selectedTags.includes(normalized)) {
                setSelectedTags(prev => [...prev, normalized]);
            }

            setInputValue('');
        }

        const updates = {
            [currentEnvelope.account_id]: [currentEnvelope.id],
        };

        mutate(
            {
                updates,
                tags: inputValue.trim()
                    ? [...selectedTags, inputValue.toLowerCase().trim()]
                    : selectedTags,
                action: "Overwrite"
            },
            {
                onSuccess: () => {
                    const finalTags = inputValue.trim()
                        ? [...selectedTags, inputValue.toLowerCase().trim()]
                        : selectedTags;
                    setCurrentEnvelope(prev => prev ? { ...prev, tags: finalTags } : prev);
                    toast({
                        title: t('search.addTags.updatedTitle'),
                        description: (
                            <div className="flex items-center gap-2">
                                <Check className="h-4 w-4 text-green-500" />
                                <span>{t('search.addTags.updatedDesc')}</span>
                            </div>
                        ),
                    });
                    queryClient.invalidateQueries({ queryKey: ['all-tags'] });
                    onOpenChange(false);
                },
                onError: (error: any) => {
                    toast({
                        title: t('search.addTags.updateFailedTitle'),
                        description: error?.message || t('search.addTags.tryAgain'),
                        variant: 'destructive',
                    });
                },
            }
        );
    };

    const filteredSuggestions = availableTags.filter(
        tag => !selectedTags.includes(tag) && tag.includes(inputValue.toLowerCase())
    );

    return (
        <Dialog open={open} onOpenChange={onOpenChange}>
            <DialogContent aria-describedby={undefined} className="sm:max-w-md">
                <DialogHeader>
                    <DialogTitle className="flex items-center gap-2">
                        <TagIcon className="h-5 w-5" />
                        {t('search.addTags.title')}
                    </DialogTitle>
                </DialogHeader>

                <div className="space-y-5 py-4">
                    <div className="flex flex-wrap gap-2 mb-3">
                        {selectedTags.length === 0 ? (
                            <p className="text-sm text-muted-foreground">{t('search.addTags.none')}</p>
                        ) : (
                            selectedTags.map(tag => (
                                <Badge key={tag} variant="secondary" className="gap-1 pr-1 h-7">
                                    {tag}
                                    <button
                                        onClick={() => handleRemoveTag(tag)}
                                        className="rounded-sm hover:bg-destructive/20 hover:text-destructive transition-colors"
                                    >
                                        <X className="h-3 w-3" />
                                    </button>
                                </Badge>
                            ))
                        )}
                    </div>
                    <div className="space-y-2">
                        <div className="relative">
                            <Search className="absolute left-3 top-1/2 -translate-y-1/2 h-4 w-4 text-muted-foreground" />
                            <Input
                                placeholder={t('search.addTags.searchPlaceholder')}
                                value={inputValue}
                                onChange={(e) => {
                                    setInputValue(e.target.value);
                                    setCommandOpen(true);
                                }}
                                onFocus={() => setCommandOpen(true)}
                                className="h-9 pl-9 pr-10"
                                onKeyDown={(e) => {
                                    if (e.key === 'Enter' && inputValue.trim()) {
                                        e.preventDefault();
                                        handleAddTag(inputValue);
                                    }
                                }}
                            />
                            {inputValue.trim() && (
                                <Button
                                    size="sm"
                                    variant="ghost"
                                    className="absolute right-1 top-1 h-7 w-7 p-0"
                                    onClick={() => handleAddTag(inputValue)}
                                >
                                    <Plus className="h-3.5 w-3.5" />
                                </Button>
                            )}
                        </div>
                        {inputValue.trim() && filteredSuggestions.length === 0 && (
                            <div className="px-1 text-xs text-muted-foreground">
                                {t('search.addTags.createHint', { tag: inputValue })}
                            </div>
                        )}
                        {commandOpen && inputValue && filteredSuggestions.length > 0 && (
                            <ScrollArea className="h-[15rem] w-full pr-4 -mr-4 py-1">
                                <div className="p-1">
                                    {filteredSuggestions.map(tag => (
                                        <button
                                            key={tag}
                                            type="button"
                                            className="relative flex w-full cursor-default select-none items-center rounded-sm px-2 py-1.5 text-sm outline-none hover:bg-accent hover:text-accent-foreground"
                                            onClick={() => handleAddTag(tag)}
                                        >
                                            <Check className="mr-2 h-4 w-4 opacity-0" />
                                            {tag}
                                        </button>
                                    ))}
                                </div>
                            </ScrollArea>
                        )}
                    </div>
                </div>

                <div className="flex justify-between items-center pt-4 border-t">
                    <p className="text-xs text-muted-foreground">
                        {t('search.addTags.selectedCount', { count: selectedTags.length })}
                    </p>
                    <div className="flex gap-2">
                        <Button variant="outline" onClick={() => onOpenChange(false)}>
                            {t('search.addTags.cancel')}
                        </Button>
                        <Button onClick={handleSave} disabled={isPending}>
                            {isPending ? (
                                <>
                                    <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                                    {t('search.addTags.saving')}
                                </>
                            ) : (
                                t('search.addTags.save')
                            )}
                        </Button>
                    </div>
                </div>
            </DialogContent>
        </Dialog>
    );
}
