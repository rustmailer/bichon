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


import { Card, CardContent } from '@/components/ui/card';
import { FixedHeader } from '@/components/layout/fixed-header';
import { Main } from '@/components/layout/main';
import { useSearchMessages } from '@/hooks/use-search-messages';
import { SearchFormDialog } from './search-form';
import { EnvelopeListPagination } from '@/components/pagination';
import { MailList } from './mail-list';
import React from 'react';
import { EmailEnvelope } from '@/api';
import { Filter, SearchIcon } from 'lucide-react';
import { MailDisplayDrawer } from './mail-display-dialog';
import { EnvelopeDeleteDialog } from './delete-dialog';
import SearchProvider, { SearchDialogType } from './context';
import useDialogState from '@/hooks/use-dialog-state';
import { ScrollArea } from '@/components/ui/scroll-area';
import { Sheet, SheetContent, SheetHeader, SheetTitle, SheetTrigger } from '@/components/ui/sheet';
import { Button } from '@/components/ui/button';
import { EnvelopeTags } from './tag-facet';
import { EditTagsDialog } from './add-tag-dialog';
import { useTranslation } from 'react-i18next';
import Logo from '@/assets/logo.svg'
import { RestoreMessageDialog } from './restore-message-dialog';

export default function Search() {
  const { t } = useTranslation()
  const [selectedEnvelope, setSelectedEnvelope] = React.useState<EmailEnvelope | undefined>(undefined);
  const [open, setOpen] = useDialogState<SearchDialogType>(null)
  const [toDelete, setToDelete] = React.useState<Map<number, Set<number>>>(new Map());
  const [selected, setSelected] = React.useState<Map<number, Set<number>>>(new Map());
  const [selectedTags, setSelectedTags] = React.useState<string[]>([]);

  const {
    emails,
    total,
    totalPages,
    isLoading,
    isFetching,
    page,
    pageSize,
    setPage,
    setPageSize,
    onSubmit,
    reset,
    filter
  } = useSearchMessages();

  const handleSetPageSize = (pageSize: number) => {
    setPage(1);
    setPageSize(pageSize)
  }


  const handleReset = () => {
    reset();
    setSelectedTags([]);
  };

  const handleTagToggle = (tag: string) => {
    setSelectedTags(prev =>
      prev.includes(tag)
        ? prev.filter(t => t !== tag)
        : [...prev, tag]
    );
  };

  return (
    <>
      <FixedHeader />
      <Main>
        <SearchProvider value={{ open, setOpen, currentEnvelope: selectedEnvelope, selectedTags, setCurrentEnvelope: setSelectedEnvelope, toDelete, setToDelete, selected, setSelected }}>
          <div className="mx-auto w-full px-4">
            <div className="mb-4 lg:hidden">
              <Sheet>
                <SheetTrigger asChild>
                  <Button variant="outline" size="sm">
                    <Filter className="mr-2 h-4 w-4" />
                    {t('search.tagFilter')}
                    {selectedTags.length > 0 && ` (${selectedTags.length})`}
                  </Button>
                </SheetTrigger>
                <SheetContent side="left" className="w-80">
                  <SheetHeader>
                    <SheetTitle>{t('search.tagFilter')}</SheetTitle>
                  </SheetHeader>
                  <div className="mt-6">
                    <EnvelopeTags
                      selectedTags={selectedTags}
                      onTagToggle={handleTagToggle}
                    />
                  </div>
                </SheetContent>
              </Sheet>
            </div>

            <div className="flex gap-6">
              <aside className="hidden lg:block w-64 flex-shrink-0">
                <div className="rounded-lg border bg-card p-4">
                  <EnvelopeTags
                    selectedTags={selectedTags}
                    onTagToggle={handleTagToggle}
                  />
                </div>
              </aside>
              <div className="flex-1 min-w-0 space-y-4">
                <Button size="sm" onClick={() => setOpen("search-form")}>
                  <SearchIcon className="mr-2 h-4 w-4" />
                  {t('common.search')}
                </Button>
                {isLoading && (
                  <Card>
                    <CardContent className="py-12">
                      <div className="flex flex-col items-center gap-2 text-muted-foreground">
                        <div className="animate-spin rounded-full h-6 w-6 border-2 border-primary border-t-transparent"></div>
                        <p className="text-sm">{t('search.searching')}</p>
                      </div>
                    </CardContent>
                  </Card>
                )}

                {total === 0 && <div className="flex h-[750px] shrink-0 items-center justify-center rounded-md border border-dashed">
                  <div className="mx-auto flex max-w-[420px] flex-col items-center justify-center text-center">
                    <img
                      src={Logo}
                      className="max-h-[100px] w-auto opacity-20 saturate-0 transition-all duration-300 hover:opacity-100 hover:saturate-100 object-contain"
                      alt="Bichon Logo"
                    />
                    <h3 className="mt-4 text-lg font-semibold">{t('search.noEmailsFound')}</h3>
                    <p className="text-sm text-muted-foreground max-w-md mx-auto">
                      {Object.keys(filter).length === 0
                        ? t('search.startSearching')
                        : t('search.adjustSearch')}
                    </p>
                  </div>
                </div>}
                {total > 0 && <ScrollArea className='h-[40rem] w-full pr-4 -mr-4 py-1'>
                  <MailList
                    isLoading={isLoading}
                    items={emails}
                    onEnvelopeChanged={(envelope) => {
                      setOpen('display');
                      setSelectedEnvelope(envelope);
                    }}
                  />
                </ScrollArea>}
                {total > 0 && <EnvelopeListPagination
                  totalItems={total}
                  hasNextPage={() => page < totalPages}
                  pageIndex={page - 1}
                  pageSize={pageSize}
                  setPageIndex={(index) => setPage(index + 1)}
                  setPageSize={handleSetPageSize}
                />}
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
            open={open === 'edit-tags'}
            onOpenChange={() => setOpen('edit-tags')}
          />

          <SearchFormDialog
            key='search-form-dialog'
            onSubmit={onSubmit} isLoading={isLoading || isFetching} reset={handleReset}
            open={open === 'search-form'}
            onOpenChange={() => setOpen('search-form')}
          />

          <RestoreMessageDialog
            key='restore-mail-dialog'
            open={open === 'restore'}
            onOpenChange={() => setOpen('restore')}
          />
        </SearchProvider>
      </Main>
    </>
  );
}