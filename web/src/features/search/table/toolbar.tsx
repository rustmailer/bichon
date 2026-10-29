import { type Table } from '@tanstack/react-table'
import { History, Sparkles } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { SearchRefreshButton } from '@/components/search-refresh-button'
import { useCurrentUser } from '@/hooks/use-current-user'
import { SavedSearchesDropdown } from '../../saved-searches/saved-searches-dropdown'
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
    setRepairHistoryOpen,
  } = useSearchContext()
  const { t } = useTranslation()
  const { require_any_permission } = useCurrentUser()

  // No accountId: the drawer is global, so any account the caller may repair
  // is enough to make the button meaningful.
  const canSeeRepairHistory = require_any_permission([
    'system:root',
    'data:manage',
  ])
  return (
    <div className='flex flex-col gap-1 p-1 bg-background'>
      <div className='mb-2 flex items-center justify-center w-full'>
        <div className='w-full max-w-3xl'>
          <TextSearchInput />
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
          <FilterResetButton />
          <SavedSearchesDropdown
            kind='Email'
            filter={filter}
            onApply={(f) => setFilter(f)}
          />
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
          {canSeeRepairHistory && (
            <Button
              variant='outline'
              size='sm'
              className='h-6 gap-1 px-2 text-xs rounded-none'
              onClick={() => setRepairHistoryOpen(true)}
            >
              <History className='h-3.5 w-3.5' />
              {t('repair_message.history', 'Repair history')}
            </Button>
          )}
          <SearchRefreshButton onRefresh={refetch} isRefreshing={isFetching} />
        </div>
      </div>
    </div>
  )
}
