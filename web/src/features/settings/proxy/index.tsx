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


import { useState } from 'react'
import useDialogState from '@/hooks/use-dialog-state'
import { Button } from '@/components/ui/button'
import { ProxyActionDialog } from './components/action-dialog'
import { getColumns } from './components/columns'
import { ProxyDeleteDialog } from './components/delete-dialog'
import { ProxyTable } from './components/table'
import ProxyProvider, {
  type ProxyDialogType,
} from './context'
import { Plus } from 'lucide-react'
import { TableSkeleton } from '@/components/table-skeleton'
import Logo from '@/assets/logo.svg'
import useProxyList from '@/hooks/use-proxy'
import { useTranslation } from 'react-i18next'
import { Proxy } from '@/api/system/api'
import { useCurrentUser } from '@/hooks/use-current-user'


export default function ProxyManagerPage() {
  const { t } = useTranslation()
  const [currentRow, setCurrentRow] = useState<Proxy | null>(null)
  const [open, setOpen] = useDialogState<ProxyDialogType>(null)
  const { require_any_permission } = useCurrentUser()
  const { proxyList, isLoading } = useProxyList()
  const columns = getColumns(t)
  // Creating/editing/deleting proxies is root-only on the server; the UI must
  // not open a dialog the save would only reject later.
  const canManage = require_any_permission(['system:root'])

  return (
    <div className="w-full max-w-5xl ml-0 px-4">
      <ProxyProvider value={{ open, setOpen, currentRow, setCurrentRow }}>
        <div>
          <div className="mb-4 flex flex-col sm:flex-row items-start sm:items-center justify-between gap-2">
            <div className="flex gap-2">
              <Button
                className="space-x-1"
                disabled={!canManage}
                title={canManage ? undefined : t('common.noPermission')}
                onClick={() => setOpen('add')}
              >
                <span>{t('settings.add')}</span> <Plus size={18} />
              </Button>
            </div>
          </div>
          <div className="flex-1 w-full overflow-auto -mx-4 px-4 py-1">
            {isLoading ? (
              <TableSkeleton columns={columns.length} rows={10} />
            ) : proxyList?.length ? (
              <div className="overflow-x-auto">
                <ProxyTable data={proxyList} columns={columns} />
              </div>
            ) : (
              <div className="flex min-h-[300px] items-center justify-center rounded-md border border-dashed p-4">
                <div className="mx-auto flex w-full max-w-md flex-col items-center justify-center text-center">
                  <img
                    src={Logo}
                    className="max-h-[100px] w-auto opacity-20 saturate-0 transition-all duration-300 hover:opacity-100 hover:saturate-100 object-contain"
                    alt="Bichon Logo"
                  />
                  <h3 className="mt-4 text-lg font-semibold">{t('settings.noProxies')}</h3>
                  <p className="mt-2 mb-4 text-sm text-muted-foreground">
                    {t('settings.noProxiesDesc')}
                  </p>
                  <Button
                    disabled={!canManage}
                    title={canManage ? undefined : t('common.noPermission')}
                    onClick={() => setOpen('add')}
                  >
                    {t('settings.add')} {t('settings.proxy')}
                  </Button>
                </div>
              </div>
            )}
          </div>
        </div>
        <ProxyActionDialog
          key="Proxy-add"
          open={open === 'add'}
          onOpenChange={() => setOpen(null)}
        />

        {currentRow && (
          <>
            <ProxyActionDialog
              key={`Proxy-edit-${currentRow.id}`}
              open={open === 'edit'}
              currentRow={currentRow}
              onOpenChange={() => {
                setOpen(null)
                setTimeout(() => setCurrentRow(null), 500)
              }}
            />

            <ProxyDeleteDialog
              key={`Proxy-delete-${currentRow.id}`}
              open={open === 'delete'}
              currentRow={currentRow}
              onOpenChange={() => {
                setOpen(null)
                setTimeout(() => setCurrentRow(null), 500)
              }}
            />
          </>
        )}
      </ProxyProvider>
    </div>
  )
}
