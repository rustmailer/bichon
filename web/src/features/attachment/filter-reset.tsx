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


import { X } from "lucide-react"
import { Button } from "@/components/ui/button"
import { useAttachmentContext } from "./context"
import { cn } from "@/lib/utils"
import { useTranslation } from "react-i18next";

export function FilterResetButton({ className }: { className?: string }) {
    const { filter, setFilter } = useAttachmentContext();
    const { t } = useTranslation()
    const { q, ...restFilters } = filter;

    const activeFiltersCount = Object.keys(restFilters).filter(key => {
        const value = restFilters[key];
        if (Array.isArray(value)) return value.length > 0;
        return value !== undefined && value !== null && value !== '';
    }).length;

    if (activeFiltersCount === 0) return null;

    return (
        <Button
            variant="ghost"
            size="sm"
            onClick={() => setFilter(q ? { q } : {})}
            className={cn(
                "h-7 px-2 text-xs gap-1.5 font-medium rounded-md",
                "text-foreground/70 hover:text-foreground hover:bg-accent transition-all duration-200",
                className
            )}
            title={t('search_reset.tooltip')}
        >
            <span>{t('search_reset.label')}</span>
            <div className="flex items-center justify-center min-w-4 h-4 px-1 rounded-full bg-primary text-primary-foreground text-[10px] font-bold">
                {activeFiltersCount}
            </div>
            <X className="h-4 w-4" />
        </Button>
    );
}