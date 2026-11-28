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
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <http://www.gnu.org/licenses/>.

import { useState } from 'react'
import useDialogState from '@/hooks/use-dialog-state'
import { Button } from '@/components/ui/button'
import { RoleActionDialog } from './components/action-dialog'
import { getColumns } from './components/columns'
import { RoleDeleteDialog } from './components/delete-dialog'
import { RolesTable } from './components/table'
import RoleProvider, {
  type RoleDialogType,
} from './context'
import { Plus } from 'lucide-react'
import { TableSkeleton } from '@/components/table-skeleton'
import Logo from '@/assets/logo.svg'
import { list_roles, UserRole } from '@/api/users/api'
import { useQuery } from '@tanstack/react-query'
import { PermissionsDialog } from './components/permissions-dialog'
import { useTranslation } from 'react-i18next'

export default function Roles() {
  const { t } = useTranslation()
  const [currentRow, setCurrentRow] = useState<UserRole | null>(null)
  const [open, setOpen] = useDialogState<RoleDialogType>(null)

  const { data: roles, isLoading } = useQuery({
    queryKey: ['role-list'],
    queryFn: list_roles,
  })

  const columns = getColumns(t)

  return (
    <div className="w-full max-w-6xl ml-0 px-4">
      <RoleProvider value={{ open, setOpen, currentRow, setCurrentRow }}>
        <div className="w-full">
          <div className="mb-4 flex flex-col sm:flex-row items-start sm:items-center justify-between gap-2">
            <Button
              className="mt-2 sm:mt-0 space-x-1"
              onClick={() => setOpen('add')}
            >
              <span>{t('roles.actions.add')}</span> <Plus size={18} />
            </Button>
          </div>
          <div className="w-full">
            {isLoading ? (
              <TableSkeleton columns={columns.length} rows={10} />
            ) : roles?.length ? (
              <div className="overflow-x-auto">
                <RolesTable data={roles} columns={columns} />
              </div>
            ) : (
              <div className="flex min-h-[300px] items-center justify-center rounded-md border border-dashed p-4">
                <div className="mx-auto flex w-full max-w-md flex-col items-center justify-center text-center">
                  <img
                    src={Logo}
                    className="max-h-[100px] w-auto opacity-20 saturate-0 transition-all duration-300 hover:opacity-100 hover:saturate-100 object-contain"
                    alt="Bichon Logo"
                  />
                  <h3 className="mt-4 text-lg font-semibold">
                    {t('roles.empty.title')}
                  </h3>
                  <p className="mt-2 mb-4 text-sm text-muted-foreground">
                    {t('roles.empty.desc')}
                  </p>
                  <Button onClick={() => setOpen('add')}>
                    {t('roles.actions.add')}
                  </Button>
                </div>
              </div>
            )}
          </div>

          {/* Dialogs */}
          <RoleActionDialog
            key="role-add"
            open={open === 'add'}
            onOpenChange={() => setOpen(null)}
          />

          {currentRow && (
            <>
              <RoleActionDialog
                key={`role-edit-${currentRow.id}`}
                currentRow={currentRow}
                open={open === 'edit'}
                onOpenChange={() => {
                  setOpen(null)
                  setTimeout(() => setCurrentRow(null), 500)
                }}
              />

              <PermissionsDialog
                key={`permissions-${currentRow.id}`}
                currentRow={currentRow}
                open={open === 'permissions'}
                onOpenChange={() => {
                  setOpen(null)
                  setTimeout(() => setCurrentRow(null), 500)
                }}
              />

              <RoleDeleteDialog
                key={`role-delete-${currentRow.id}`}
                currentRow={currentRow}
                open={open === 'delete'}
                onOpenChange={() => {
                  setOpen(null)
                  setTimeout(() => setCurrentRow(null), 500)
                }}
              />
            </>
          )}
        </div>
      </RoleProvider>
    </div>
  )
}