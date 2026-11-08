import { type Table } from '@tanstack/react-table'
import { Sparkles } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { Badge } from '@/components/ui/badge'
import { SearchRefreshButton } from '@/components/search-refresh-button'
import { SearchActionsMenu } from '../search-actions-menu'
import { AccountPopover } from '../account-popover'
import { MailFilterPopover } from '../contact-popover'
import { useSearchContext } from '../context'
import { FilterResetButton } from '../filter-reset'
import { MailboxPopover } from '../mailbox-popover'
import { MoreFiltersPopover } from '../more-filters-popover'
import { TagFilterPopover } from '../tag-filter-popover'
import { TextSearchInput } from '../text-search-input'
import { TimePopover } from '../time-popover'
import { DataTableViewOptions } from './view-options'

type DataTableToolbarProps<TData> = {
  table: Table<TData>
}

export function DataTableToolbar<TData>({
  table,
}: DataTableToolbarProps<TData>) {
  const {
    filter,
    setFilter,
    effectiveSort,
    refetch,
    isFetching,
  } = useSearchContext()
  const { t } = useTranslation()
  return (
    <div className='flex flex-col gap-1 p-1 bg-background'>
      <div className='mb-2 flex items-center justify-center w-full'>
        {/* `items-start`: the search input is taller than its pill (it carries
            the hint line below). Centering the row would push Reset below the
            Search button; `mt-1` centers it on the 36px pill instead. */}
        <div className='flex items-start justify-center gap-1.5 w-full max-w-[740px]'>
          <TextSearchInput />
          <FilterResetButton className='mt-1 shrink-0' />
        </div>
      </div>
      <div className='flex flex-col sm:flex-row items-start sm:items-center justify-between gap-2 sm:gap-1'>
        <div className='flex items-center gap-2 flex-wrap w-full sm:w-auto'>
          <div className='flex items-center gap-1.5 flex-wrap'>
            <AccountPopover />
            <MailboxPopover />
            <MailFilterPopover />
            <TagFilterPopover />
            <MoreFiltersPopover />
          </div>
        </div>
        <div className='flex items-center gap-1 shrink-0'>
          {effectiveSort === 'RELEVANCE' && (
            <Badge
              variant='outline'
              className='gap-1 border-primary/40 bg-primary/10 text-primary font-medium'
              title={t('search.sortedByRelevance')}
            >
              <Sparkles className='h-3 w-3' />
              {t('search.sortedByRelevance')}
            </Badge>
          )}
          <TimePopover />
          <DataTableViewOptions table={table} />
          <SearchActionsMenu
            kind='Email'
            filter={filter}
            onApply={(next) => setFilter(next)}
          />
          <SearchRefreshButton onRefresh={refetch} isRefreshing={isFetching} />
        </div>
      </div>
    </div>
  )
}
