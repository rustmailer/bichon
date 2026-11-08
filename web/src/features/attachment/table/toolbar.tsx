import { type Table } from '@tanstack/react-table'
import { FileType, Laptop, Tag } from 'lucide-react'
import { Sparkles } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { Badge } from '@/components/ui/badge'
import { SearchRefreshButton } from '@/components/search-refresh-button'
import { SearchActionsMenu } from '../../search/search-actions-menu'
import { AccountPopover } from '../account-popover'
import { MetadataFilter } from '../attachment-metadata-filter'
import { useAttachmentContext } from '../context'
import { FilterResetButton } from '../filter-reset'
import { MailboxPopover } from '../mailbox-popover'
import { MoreFiltersPopover } from '../more-filters-popover'
import { SenderFilterPopover } from '../sender-popover'
import { TextSearchInput } from '../text-search-input'
import { TimePopover } from '../time-popover'
import { DataTableViewOptions } from './view-options'

type DataTableToolbarProps<TData> = {
  table: Table<TData>
}

export function DataTableToolbar<TData>({
  table,
}: DataTableToolbarProps<TData>) {
  const { filter, setFilter, effectiveSort, refetch, isFetching } =
    useAttachmentContext()
  const { t } = useTranslation()
  return (
    <div className='flex flex-col gap-1 p-1 bg-background'>
      <div className='mb-4 flex items-center justify-center w-full'>
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
            <SenderFilterPopover />
            <MetadataFilter
              type='extension'
              icon={<FileType className='h-3.5 w-3.5' />}
            />
            <MetadataFilter
              type='category'
              icon={<Tag className='h-3.5 w-3.5' />}
            />
            <MetadataFilter
              type='content_type'
              icon={<Laptop className='h-3.5 w-3.5' />}
            />

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
            kind='Attachment'
            filter={filter}
            onApply={(next) => setFilter(next)}
          />
          <SearchRefreshButton onRefresh={refetch} isRefreshing={isFetching} />
        </div>
      </div>
    </div>
  )
}
