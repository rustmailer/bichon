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
import React from 'react'
import { SortingState } from '@tanstack/react-table'
import { EmailEnvelope } from '@/api'
import { useTranslation } from 'react-i18next'
import { useDateDisplay } from '@/hooks/use-date-display'
import useDialogState from '@/hooks/use-dialog-state'
import { useSearchMessages } from '@/hooks/use-search-messages'
import { Card, CardContent } from '@/components/ui/card'
import { FixedHeader } from '@/components/layout/fixed-header'
import { Main } from '@/components/layout/main'
import { TablePagination } from '@/components/pagination'
import { UpdateTagsDialog } from './bulk-add-tag-dialog'
import SearchProvider, { SearchDialogType, SelectAllScope } from './context'
import { EnvelopeDeleteDialog } from './delete-dialog'
import { MailBoxDeleteDialog } from './delete-mailbox-dialog'
import { EditTagsDialog } from './edit-tag-dialog'
import { GdprErasureDialog } from './gdpr-erasure-dialog'
import { MailDisplayDrawer } from './mail-display-dialog'
import { MailListTable } from './mail-list-table'
import { BatchDeletePanel } from './batch-delete-panel'
import { RepairMessageDialog } from '@/features/repair/repair-message-dialog'
import { RestoreMessageDialog } from './restore-message-dialog'

export default function EmailSearch() {
  const { t } = useTranslation()
  const [selectedEnvelope, setSelectedEnvelope] = React.useState<
    EmailEnvelope | undefined
  >(undefined)
  const [open, setOpen] = useDialogState<SearchDialogType>(null)
  const [toDelete, setToDelete] = React.useState<Map<number, Set<string>>>(
    new Map()
  )
  const [selected, setSelected] = React.useState<Map<number, Set<string>>>(
    new Map()
  )
  const [selectAllScope, setSelectAllScope] =
    React.useState<SelectAllScope | null>(null)
  const [selectedTags, setSelectedTags] = React.useState<string[]>([])
  // Start with no client-side sort: the server already orders the page
  // (RELEVANCE when the query has a text term, DATE desc otherwise), and
  // forcing a date sort here would write sortBy=DATE into the URL and
  // override the backend's relevance default.
  const [sorting, setSorting] = React.useState<SortingState>([])
  const [deleteMailboxId, setDeleteMailboxId] = React.useState<
    string | undefined
  >(undefined)
  const [selectedAccountId, setSelectedAccountId] = React.useState<
    number | undefined
  >(undefined)
  const [editTagsOpen, setEditTagsOpen] = React.useState(false)
  const { dateDisplay, setDateDisplay: handleSetDateDisplay } = useDateDisplay()

  const {
    emails,
    total,
    totalPages,
    isLoading,
    isFetching,
    refetch,
    page,
    pageSize,
    setPage,
    setSearchPageSize,
    setSortBy,
    setSortOrder,
    effectiveSort,
    filter,
    setFilter,
  } = useSearchMessages()

  const handleSetPageSize = (pageSize: number) => {
    setPage(1)
    setSearchPageSize(pageSize)
  }

  // A selection belongs to the filter that produced it. Changing the filter
  // while "all matching" is active would otherwise leave a scope whose count
  // describes a different result set than the one the toolbar now shows.
  const filterJson = JSON.stringify(filter)
  React.useEffect(() => {
    setSelectAllScope(null)
    setSelected(new Map())
  }, [filterJson, setSelectAllScope, setSelected])

  const handleTagToggle = (tag: string) => {
    setSelectedTags((prev) =>
      prev.includes(tag) ? prev.filter((t) => t !== tag) : [...prev, tag]
    )
  }

  return (
    <>
      <FixedHeader />
      <Main>
        <SearchProvider
          value={{
            open,
            setOpen,
            currentEnvelope: selectedEnvelope,
            selectedTags,
            setCurrentEnvelope: setSelectedEnvelope,
            toDelete,
            setToDelete,
            selected,
            setSelected,
            selectAllScope,
            setSelectAllScope,
            sorting,
            setSorting,
            filter,
            setFilter,
            effectiveSort,
            refetch,
            isFetching,
            deleteMailboxId,
            setDeleteMailboxId,
            selectedAccountId,
            setSelectedAccountId,
            handleTagToggle,
            editTagsOpen,
            setEditTagsOpen,
            dateDisplay,
            setDateDisplay: handleSetDateDisplay,
          }}
        >
          <div className='mx-auto w-full px-4'>
            <div className='flex gap-6'>
              <div className='flex-1 min-w-0 space-y-4'>
                <BatchDeletePanel />
                {isLoading && (
                  <Card>
                    <CardContent className='py-12'>
                      <div className='flex flex-col items-center gap-2 text-muted-foreground'>
                        <div className='animate-spin rounded-full h-6 w-6 border-2 border-primary border-t-transparent'></div>
                        <p className='text-sm'>{t('search.searching')}</p>
                      </div>
                    </CardContent>
                  </Card>
                )}

                <MailListTable
                  isLoading={isLoading}
                  items={emails}
                  total={total}
                  onEnvelopeChanged={(envelope) => {
                    setOpen('display')
                    setSelectedEnvelope(envelope)
                  }}
                  setSortBy={setSortBy}
                  setSortOrder={setSortOrder}
                />
                {total > 0 && (
                  <TablePagination
                    totalItems={total}
                    hasNextPage={() => page < totalPages}
                    pageIndex={page - 1}
                    pageSize={pageSize}
                    setPageIndex={(index) => setPage(index + 1)}
                    setPageSize={handleSetPageSize}
                  />
                )}
              </div>
            </div>
          </div>

          <MailDisplayDrawer
            key='search-mail-display'
            open={open === 'display'}
            onOpenChange={() => setOpen('display')}
          />

          <EnvelopeDeleteDialog
            key='delete-envelope'
            open={open === 'delete'}
            onOpenChange={() => setOpen('delete')}
          />

          <EditTagsDialog
            key='edit-tags-dialog'
            open={editTagsOpen}
            onOpenChange={setEditTagsOpen}
          />

          <UpdateTagsDialog
            key='update-tags-dialog'
            open={open === 'update-tags'}
            onOpenChange={() => setOpen('update-tags')}
          />

          <RestoreMessageDialog
            key='restore-mail-dialog'
            open={open === 'restore'}
            onOpenChange={() => setOpen('restore')}
          />

          <RepairMessageDialog
            key='repair-mail-dialog'
            open={open === 'repair'}
            onOpenChange={() => setOpen('repair')}
            accountId={selectedEnvelope?.account_id}
            envelopeId={selectedEnvelope?.id}
            subject={selectedEnvelope?.subject}
          />

          <MailBoxDeleteDialog
            key='mailbox-delete'
            open={open === 'delete-mailbox'}
            onOpenChange={() => setOpen('delete-mailbox')}
          />

          <GdprErasureDialog
            key='gdpr-erasure'
            open={open === 'gdpr-erasure'}
            onOpenChange={() => setOpen('gdpr-erasure')}
          />
        </SearchProvider>
      </Main>
    </>
  )
}
