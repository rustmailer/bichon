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


import { useState } from 'react'
import useDialogState from '@/hooks/use-dialog-state'
import { Button } from '@/components/ui/button'
import { Main } from '@/components/layout/main'
import { TokensActionDialog } from './components/action-dialog'
import { getColumns } from './components/columns'
import { TokenDeleteDialog } from './components/delete-dialog'
import { AccessTokensTable } from './components/access-token-table'
import AccessTokensProvider, {
  type AccessTokensDialogType,
} from './context'
import Logo from '@/assets/logo.svg'
import { Plus } from 'lucide-react'
import { AccessToken } from './data/schema'
import { AccountDetailDialog } from './components/accounts-detail-dialog'
import { AclDetailDialog } from './components/acl-detail-dialog'
import { useQuery } from '@tanstack/react-query'
import { list_access_tokens } from '@/api/access-tokens/api'
import { TableSkeleton } from '@/components/table-skeleton'
import { FixedHeader } from '@/components/layout/fixed-header'
import { useTranslation } from 'react-i18next'
import { dateFnsLocaleMap } from '@/lib/utils'
import { enUS } from 'date-fns/locale'

export default function AccessTokens() {
  const { t, i18n } = useTranslation()
  const locale = dateFnsLocaleMap[i18n.language.toLowerCase()] ?? enUS;
  // Dialog states
  const [currentRow, setCurrentRow] = useState<AccessToken | null>(null)
  const [open, setOpen] = useDialogState<AccessTokensDialogType>(null)

  const { data: accessTokens, isLoading } = useQuery({
    queryKey: ['access-tokens'],
    queryFn: list_access_tokens,
  })

  const columns = getColumns(t, locale)

  return (
    <AccessTokensProvider value={{ open, setOpen, currentRow, setCurrentRow }}>
      {/* ===== Top Heading ===== */}
      <FixedHeader />

      <Main>
        <div className="mx-auto mb-2 flex max-w-5xl flex-wrap items-center justify-between gap-x-4 gap-y-2 px-2">
          <div>
            <h2 className="text-2xl font-bold tracking-tight">{t('accessTokens.title')}</h2>
            <p className="text-muted-foreground">
              {t('accessTokens.description')}
            </p>
          </div>
          <div className="flex gap-2">
            <Button className="space-x-1" onClick={() => setOpen('add')}>
              <span>{t('common.add')}</span> <Plus size={18} />
            </Button>
          </div>
        </div>

        <div className="mx-auto flex-1 overflow-auto px-4 py-1 flex-row lg:space-x-12 space-y-0 max-w-5xl">
          {isLoading ? (
            <TableSkeleton columns={columns.length} rows={10} />
          ) : accessTokens?.length ? (
            <AccessTokensTable data={accessTokens} columns={columns} />
          ) : (
            <div className="flex h-[450px] shrink-0 items-center justify-center rounded-md border border-dashed">
              <div className="mx-auto flex max-w-[420px] flex-col items-center justify-center text-center">
                <img
                  src={Logo}
                  className="max-h-[100px] w-auto opacity-20 saturate-0 transition-all duration-300 hover:opacity-100 hover:saturate-100 object-contain"
                  alt="Bichon Logo"
                />
                <h3 className="mt-4 text-lg font-semibold">{t('accessTokens.noTokens')}</h3>
                <p className="mb-4 mt-2 text-sm text-muted-foreground">
                  {t('accessTokens.noTokensDesc')}
                </p>
                <Button onClick={() => setOpen('add')}>{t('accessTokens.create')}</Button>
              </div>
            </div>
          )}
        </div>
      </Main>


      <TokensActionDialog
        key='token-add'
        open={open === 'add'}
        onOpenChange={() => setOpen('add')}
      />

      {currentRow && (
        <>
          <TokensActionDialog
            key={`token-edit-${currentRow.token}`}
            open={open === 'edit'}
            onOpenChange={() => {
              setOpen('edit')
              setTimeout(() => {
                setCurrentRow(null)
              }, 500)
            }}
            currentRow={currentRow}
          />

          <TokenDeleteDialog
            key={`token-delete-${currentRow.token}`}
            open={open === 'delete'}
            onOpenChange={() => {
              setOpen('delete')
              setTimeout(() => {
                setCurrentRow(null)
              }, 500)
            }}
            currentRow={currentRow}
          />

          <AccountDetailDialog
            key={`accounts-detail-${currentRow.token}`}
            currentRow={currentRow}
            open={open === 'account-detail'}
            onOpenChange={() => {
              setOpen('account-detail')
              setTimeout(() => {
                setCurrentRow(null)
              }, 500)
            }} />

          <AclDetailDialog
            key={`acl-detail-${currentRow.token}`}
            currentRow={currentRow}
            open={open === 'acl-detail'}
            onOpenChange={() => {
              setOpen('acl-detail')
              setTimeout(() => {
                setCurrentRow(null)
              }, 500)
            }} />
        </>
      )}
    </AccessTokensProvider>
  )
}
