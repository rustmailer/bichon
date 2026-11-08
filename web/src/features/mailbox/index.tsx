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
import { getRouteApi, Link } from '@tanstack/react-router'
import { useQuery } from '@tanstack/react-query'
import { SortingState } from '@tanstack/react-table'
import { useTranslation } from 'react-i18next'
import { ExternalLink, Search, X } from 'lucide-react'

import { EmailEnvelope } from '@/api'
import { search_messages } from '@/api/search/api'
import { list_mailboxes } from '@/api/mailbox/api'
import { useDateDisplay } from '@/hooks/use-date-display'
import useDialogState from '@/hooks/use-dialog-state'
import useMinimalAccountList from '@/hooks/use-minimal-account-list'
import { useMinWidth } from '@/hooks/use-min-width'
import { FixedHeader } from '@/components/layout/fixed-header'
import { Main } from '@/components/layout/main'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import SearchProvider, {
  SearchDialogType,
  SelectAllScope,
} from '@/features/search/context'
import { EnvelopeDeleteDialog } from '@/features/search/delete-dialog'
import { MailBoxDeleteDialog } from '@/features/search/delete-mailbox-dialog'
import { EditTagsDialog } from '@/features/search/edit-tag-dialog'
import { MailDisplayDrawer } from '@/features/search/mail-display-dialog'
import { RestoreMessageDialog } from '@/features/search/restore-message-dialog'
import { RepairMessageDialog } from '@/features/repair/repair-message-dialog'

import { FolderTree } from './folder-tree'
import { MessageList } from './message-list'
import { MessagePagination } from './message-pagination'
import { MessageReader } from './message-reader'

const routeApi = getRouteApi('/_authenticated/mailbox/')

/// Last account the user browsed. Validated against the visible account list
/// before use; a stale value is removed rather than sent to the API.
const MAILBOX_ACCOUNT_KEY = 'bichon_mailbox_account'

/**
 * Single-account mailbox browser: folder tree, mail-client message list and a
 * reading pane.
 *
 * This is a separate page with its own state and components. It reuses the
 * search feature only where it is genuinely shared — the message reader, the
 * dialogs it opens, the mailbox API and `buildTree` — and does not change the
 * search page's behavior.
 */
export default function MailboxView() {
  const { t } = useTranslation()
  const search = routeApi.useSearch()
  const navigate = routeApi.useNavigate()

  const accountId = search.account
  const mailboxId = search.mailbox
  const page = search.page ?? 1
  const pageSize = search.pageSize ?? 50
  const textQuery = (search.text ?? '').trim()

  const canShowSidebar = useMinWidth(768)
  // Three fixed-ish columns need real estate: sidebar + list + reader.
  const canShowReader = useMinWidth(1152)

  const { minimalList = [], isLoading: accountsLoading } = useMinimalAccountList()

  // Views hold no mail of their own, so this browser only offers accounts
  // that actually own a mailbox tree.
  const accounts = React.useMemo(
    () => minimalList.filter((account) => account.account_type !== 'View'),
    [minimalList]
  )

  const knownAccountIds = React.useMemo(
    () => new Set(accounts.map((account) => account.id)),
    [accounts]
  )

  // Never let a stale URL / remembered account reach the API: queries only
  // run for an account the current user can actually see.
  const activeAccountId =
    !accountsLoading && accountId !== undefined && knownAccountIds.has(accountId)
      ? accountId
      : undefined

  // The page is single-account by design. Prefer the account in the URL, then
  // the last one this user browsed, then the first available account. A
  // remembered id that is no longer visible (deleted account, revoked ACL) is
  // dropped from localStorage instead of causing a failing request.
  React.useEffect(() => {
    if (accountsLoading) return

    const storedRaw = localStorage.getItem(MAILBOX_ACCOUNT_KEY)
    const storedId = storedRaw !== null ? Number(storedRaw) : undefined
    const storedValid =
      storedId !== undefined &&
      Number.isFinite(storedId) &&
      knownAccountIds.has(storedId)

    if (storedRaw !== null && !storedValid) {
      localStorage.removeItem(MAILBOX_ACCOUNT_KEY)
    }

    if (accountId !== undefined && knownAccountIds.has(accountId)) {
      localStorage.setItem(MAILBOX_ACCOUNT_KEY, String(accountId))
      return
    }

    const fallback =
      storedValid ? storedId : accounts.length > 0 ? accounts[0].id : undefined

    if (fallback !== undefined) {
      navigate({
        search: (prev) => ({
          ...prev,
          account: fallback,
          mailbox: undefined,
          page: 1,
        }),
        replace: true,
      })
    } else if (accountId !== undefined) {
      // No account is available: strip the stale parameter rather than leave
      // the page pointing at an account the API will refuse.
      navigate({
        search: (prev) => ({ ...prev, account: undefined, mailbox: undefined }),
        replace: true,
      })
    }
  }, [accountsLoading, accountId, accounts, knownAccountIds, navigate])

  const {
    data: mailboxes = [],
    isLoading: mailboxesLoading,
    isError: mailboxesError,
  } = useQuery({
    // Same key and query as the search mailbox popover: one cached response.
    queryKey: ['search-mailboxes', activeAccountId],
    queryFn: async () => (await list_mailboxes(activeAccountId!, false)).mailboxes,
    enabled: activeAccountId !== undefined,
  })

  const filter = React.useMemo(
    () => ({
      ...(activeAccountId !== undefined
        ? { account_ids: [activeAccountId] }
        : {}),
      ...(mailboxId !== undefined ? { mailbox_ids: [mailboxId] } : {}),
      ...(textQuery !== '' ? { text: textQuery } : {}),
    }),
    [activeAccountId, mailboxId, textQuery]
  )

  const { data, isLoading, isFetching, refetch } = useQuery({
    // Prefix `search-messages` so the shared delete dialog's cache
    // invalidation (which matches that prefix) also refreshes this list.
    queryKey: ['search-messages', 'mailbox', filter, page, pageSize],
    queryFn: () =>
      search_messages({
        filter,
        page,
        page_size: pageSize,
        sort_by: 'DATE',
        desc: true,
      }),
    enabled: activeAccountId !== undefined,
    staleTime: 1000,
  })

  const emails = data?.items ?? []
  const total = data?.total_items ?? 0
  const totalPages = data?.total_pages ?? 1

  // Search is scoped to the current account/folder: it is sent as the `text`
  // field of the same search filter, so the backend searches the whole folder
  // rather than only the current page.
  const [searchInput, setSearchInput] = React.useState(search.text ?? '')

  // Keep the box in sync when the URL changes (back/forward, folder switch).
  React.useEffect(() => {
    setSearchInput(search.text ?? '')
  }, [search.text])

  const commitSearch = React.useCallback(
    (value: string) => {
      const next = value.trim()
      navigate({
        search: (prev) => ({
          ...prev,
          text: next === '' ? undefined : next,
          page: 1,
        }),
        replace: true,
      })
    },
    [navigate]
  )

  // Debounce typing. Enter and clear commit immediately.
  React.useEffect(() => {
    if (searchInput.trim() === textQuery) return
    const timer = window.setTimeout(() => commitSearch(searchInput), 300)
    return () => window.clearTimeout(timer)
  }, [searchInput, textQuery, commitSearch])

  const [selectedEnvelope, setSelectedEnvelope] = React.useState<
    EmailEnvelope | undefined
  >(undefined)

  // A different folder or account starts with nothing selected.
  React.useEffect(() => {
    setSelectedEnvelope(undefined)
  }, [accountId, mailboxId])

  // The selected folder can disappear (deleted, or a stale URL). Fall back to
  // "All folders" instead of querying a mailbox id that no longer exists.
  React.useEffect(() => {
    if (mailboxId === undefined || mailboxesLoading || mailboxesError) return
    if (activeAccountId === undefined) return
    if (mailboxes.some((mailbox) => mailbox.id === mailboxId)) return

    setSearchInput('')
    navigate({
      search: (prev) => ({
        ...prev,
        account: activeAccountId,
        mailbox: undefined,
        text: undefined,
        page: 1,
      }),
      replace: true,
    })
  }, [
    mailboxId,
    mailboxes,
    mailboxesLoading,
    mailboxesError,
    activeAccountId,
    navigate,
  ])

  const selectAccount = (nextAccountId: number) => {
    // A different account starts with a clean, unfiltered folder list.
    setSearchInput('')
    navigate({
      search: (prev) => ({
        ...prev,
        account: nextAccountId,
        mailbox: undefined,
        text: undefined,
        page: 1,
      }),
    })
  }

  const selectMailbox = (nextMailboxId: number | undefined) => {
    if (activeAccountId === undefined) return
    setSearchInput('')
    navigate({
      search: (prev) => ({
        ...prev,
        account: activeAccountId,
        mailbox: nextMailboxId,
        text: undefined,
        page: 1,
      }),
    })
  }

  const setPage = (nextPage: number) => {
    navigate({ search: (prev) => ({ ...prev, page: nextPage }) })
  }

  const setPageSize = (nextPageSize: number) => {
    localStorage.setItem('bichon_mailbox_page_size', String(nextPageSize))
    navigate({
      search: (prev) => ({ ...prev, pageSize: nextPageSize, page: 1 }),
    })
  }

  const handleSelectEnvelope = (envelope: EmailEnvelope) => {
    setSelectedEnvelope(envelope)
    // Without the reading pane (narrow screens), fall back to the drawer.
    if (!canShowReader) setOpen('display')
  }

  // The reused reader and dialogs expect the search context. All of it is
  // local to this page; nothing here touches the search route.
  const [open, setOpen] = useDialogState<SearchDialogType>(null)
  const [toDelete, setToDelete] = React.useState<Map<number, Set<string>>>(
    new Map()
  )
  const [selected, setSelected] = React.useState<Map<number, Set<string>>>(
    new Map()
  )
  // The mailbox view has no cross-page scope; the field exists so both views
  // can share the search context type.
  const [selectAllScope, setSelectAllScope] =
    React.useState<SelectAllScope | null>(null)
  const [sorting, setSorting] = React.useState<SortingState>([])
  const [editTagsOpen, setEditTagsOpen] = React.useState(false)
  const [deleteMailboxId, setDeleteMailboxId] = React.useState<
    string | undefined
  >(undefined)
  const [selectedAccountId, setSelectedAccountId] = React.useState<
    number | undefined
  >(undefined)
  const [dialogFilter, setDialogFilter] = React.useState<Record<string, unknown>>({})
  const selectedTags: string[] = []
  const { dateDisplay, setDateDisplay } = useDateDisplay()

  const handleDeleteMailbox = (deletedMailboxId: number) => {
    if (activeAccountId === undefined) return
    setDeleteMailboxId(String(deletedMailboxId))
    setSelectedAccountId(activeAccountId)
    setOpen('delete-mailbox')
  }

  const folderName =
    mailboxId !== undefined
      ? mailboxes.find((mailbox) => mailbox.id === mailboxId)?.name
      : t('mailboxView.allFolders')

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
            filter: dialogFilter,
            setFilter: setDialogFilter,
            effectiveSort: 'DATE',
            refetch,
            isFetching,
            deleteMailboxId,
            setDeleteMailboxId,
            selectedAccountId,
            setSelectedAccountId,
            handleTagToggle: () => {},
            editTagsOpen,
            setEditTagsOpen,
            dateDisplay,
            setDateDisplay,
          }}
        >
          {accountsLoading ? (
            <div className="flex h-[60vh] items-center justify-center text-sm text-muted-foreground">
              {t('common.loading')}
            </div>
          ) : accounts.length === 0 ? (
            <div className="flex h-[60vh] items-center justify-center text-sm text-muted-foreground">
              {t('mailboxView.noAccounts')}
            </div>
          ) : (
            <div className="mx-auto flex h-[calc(100dvh-7rem)] min-h-[420px] w-full gap-3">
              {canShowSidebar && (
                <FolderTree
                  accounts={accounts}
                  accountId={activeAccountId}
                  mailboxes={mailboxes}
                  mailboxesLoading={mailboxesLoading}
                  mailboxesError={mailboxesError}
                  mailboxId={mailboxId}
                  onSelectAccount={selectAccount}
                  onSelectMailbox={selectMailbox}
                  onDeleteMailbox={handleDeleteMailbox}
                  className="w-[280px] shrink-0 xl:w-[320px]"
                />
              )}

              <section
                className={
                  canShowReader
                    ? 'flex w-[400px] shrink-0 flex-col overflow-hidden xl:w-[460px]'
                    : 'flex min-w-0 flex-1 flex-col overflow-hidden'
                }
              >
                <div className="mb-1 flex min-w-0 shrink-0 flex-col gap-1 px-1">
                  <div className="flex min-w-0 items-center justify-between gap-2 overflow-hidden">
                    <div className="flex min-w-0 items-center gap-2">
                      {!canShowSidebar && (
                        <Select
                          value={
                            activeAccountId !== undefined
                              ? String(activeAccountId)
                              : undefined
                          }
                          onValueChange={(value) => selectAccount(Number(value))}
                        >
                          <SelectTrigger className="h-7 w-[180px] text-xs">
                            <SelectValue placeholder={t('mailboxView.selectAccount')} />
                          </SelectTrigger>
                          <SelectContent>
                            {accounts.map((account) => (
                              <SelectItem
                                key={account.id}
                                value={String(account.id)}
                                className="text-xs"
                              >
                                {account.name || account.email}
                              </SelectItem>
                            ))}
                          </SelectContent>
                        </Select>
                      )}
                      <span className="truncate text-xs font-medium">
                        {folderName}
                      </span>
                    </div>
                    <div className="flex shrink-0 items-center gap-1">
                      <Button asChild variant="ghost" size="icon" className="h-6 w-6">
                        <Link
                          to="/search"
                          search={{
                            q: JSON.stringify(filter),
                            page: 1,
                            pageSize: 30,
                            sortOrder: 'desc',
                          }}
                          title={t('mailboxView.openInSearch')}
                        >
                          <ExternalLink className="h-3.5 w-3.5" />
                        </Link>
                      </Button>
                      <span className="text-[10px] text-muted-foreground">
                        {t('mailboxView.messageCount', { count: total })}
                      </span>
                    </div>
                  </div>

                  <div className="relative">
                    <Search className="absolute left-2 top-2 h-3.5 w-3.5 text-muted-foreground" />
                    <Input
                      value={searchInput}
                      onChange={(event) => setSearchInput(event.target.value)}
                      onKeyDown={(event) => {
                        if (event.key === 'Enter') {
                          event.preventDefault()
                          commitSearch(searchInput)
                        } else if (event.key === 'Escape') {
                          setSearchInput('')
                          commitSearch('')
                        }
                      }}
                      placeholder={t('mailboxView.searchInFolder')}
                      className="h-7 pl-7 pr-7 text-xs md:text-xs"
                    />
                    {searchInput !== '' && (
                      <button
                        type="button"
                        onClick={() => {
                          setSearchInput('')
                          commitSearch('')
                        }}
                        className="absolute right-1 top-1 flex h-5 w-5 items-center justify-center rounded text-muted-foreground transition-colors hover:text-foreground"
                        aria-label={t('mailboxView.clearSearch')}
                        title={t('mailboxView.clearSearch')}
                      >
                        <X className="h-3.5 w-3.5" />
                      </button>
                    )}
                  </div>
                </div>

                <MessageList
                  items={emails}
                  isLoading={isLoading}
                  activeEnvelope={selectedEnvelope}
                  onSelect={handleSelectEnvelope}
                  emptyMessage={
                    textQuery !== ''
                      ? t('mailboxView.noMatches')
                      : undefined
                  }
                />

                {totalPages > 1 && (
                  <MessagePagination
                    pageIndex={page - 1}
                    pageCount={totalPages}
                    pageSize={pageSize}
                    hasNextPage={() => page < totalPages}
                    setPageIndex={(index) => setPage(index + 1)}
                    setPageSize={setPageSize}
                  />
                )}
              </section>

              {canShowReader && (
                <MessageReader
                  envelope={selectedEnvelope}
                  className="min-w-[360px] flex-1"
                />
              )}
            </div>
          )}

          <MailDisplayDrawer
            key="mailbox-message-display"
            open={open === 'display'}
            onOpenChange={() => setOpen('display')}
          />

          <EnvelopeDeleteDialog
            key="mailbox-delete-envelope"
            open={open === 'delete'}
            onOpenChange={() => setOpen('delete')}
          />

          <EditTagsDialog
            key="mailbox-edit-tags"
            open={editTagsOpen}
            onOpenChange={setEditTagsOpen}
          />

          <RestoreMessageDialog
            key="mailbox-restore-message"
            open={open === 'restore'}
            onOpenChange={() => setOpen('restore')}
          />

          <RepairMessageDialog
            key="mailbox-repair-message"
            open={open === 'repair'}
            onOpenChange={() => setOpen('repair')}
            accountId={selectedEnvelope?.account_id}
            envelopeId={selectedEnvelope?.id}
            subject={selectedEnvelope?.subject}
          />

          <MailBoxDeleteDialog
            key="mailbox-delete-folder"
            open={open === 'delete-mailbox'}
            onOpenChange={() => setOpen('delete-mailbox')}
          />
        </SearchProvider>
      </Main>
    </>
  )
}
