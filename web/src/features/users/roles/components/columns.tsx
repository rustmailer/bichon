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

import { ColumnDef } from '@tanstack/react-table'
import LongText from '@/components/long-text'

import { DataTableColumnHeader } from './data-table-column-header'
import { DataTableRowActions } from './data-table-row-actions'
import { format } from 'date-fns'
import { UserRole } from '@/api/users/api'
import { Badge } from '@/components/ui/badge'
import { PermissionsCellAction } from './permissions-action'
import { LockIcon } from 'lucide-react'

export const getColumns = (t: (key: string) => string): ColumnDef<UserRole>[] => [
  {
    accessorKey: 'id',
    header: ({ column }) => (
      <DataTableColumnHeader column={column} title={t('roles.columns.id')} className="justify-center" />
    ),
    cell: ({ row }) => (
      <LongText className='text-center'>{`${row.original.id}`}</LongText>
    ),
    enableHiding: false,
    meta: { className: 'w-[8%] text-center' },
    enableSorting: false
  },
  {
    accessorKey: "name",
    header: ({ column }) => (
      <DataTableColumnHeader column={column} title={t('roles.columns.name')} className="justify-center" />
    ),
    cell: ({ row }) => {
      return <LongText className='text-center'>{row.original.name}</LongText>
    },
    meta: { className: 'w-[28%] text-center' },
  },
  {
    accessorKey: 'role_type',
    header: ({ column }) => (
      <DataTableColumnHeader column={column} title={t('roles.columns.role_type')} className="justify-center" />
    ),
    cell: ({ row }) => {
      const { role_type, is_builtin } = row.original;
      return (
        <div className="flex justify-center">
          <Badge variant="outline" className="capitalize flex items-center gap-1">
            {t(`roles.types.${role_type}`)}
            {is_builtin && (
              <LockIcon className="h-3 w-3 text-muted-foreground" />
            )}
          </Badge>
        </div>
      );
    },
    meta: { className: 'w-[15%] text-center' },
  },
  {
    id: 'permissions',
    header: ({ column }) => (
      <DataTableColumnHeader column={column} title={t('roles.columns.permissions')} className="justify-center" />
    ),
    cell: PermissionsCellAction,
    meta: { className: 'w-[13%] text-center' },
    enableHiding: false,
  },
  {
    accessorKey: 'created_at',
    header: ({ column }) => (
      <DataTableColumnHeader column={column} title={t('roles.columns.created_at')} className="justify-center" />
    ),
    cell: ({ row }) => {
      const created_at = row.original.created_at;
      const date = format(new Date(created_at), 'yyyy-MM-dd HH:mm:ss');
      return <LongText className='text-center'>{date}</LongText>;
    },
    meta: { className: 'w-[14%] text-center' },
    enableHiding: false,
  },
  {
    accessorKey: 'updated_at',
    header: ({ column }) => (
      <DataTableColumnHeader column={column} title={t('roles.columns.updated_at')} className="justify-center" />
    ),
    cell: ({ row }) => {
      const updated_at = row.original.updated_at;
      const date = format(new Date(updated_at), 'yyyy-MM-dd HH:mm:ss');
      return <LongText className='text-center'>{date}</LongText>;
    },
    meta: { className: 'w-[14%] text-center' },
    enableHiding: false,
  },
  {
    id: 'actions',
    header: ({ column }) => (
      <DataTableColumnHeader column={column} title={t('roles.columns.actions')} className='text-right' />
    ),
    cell: ({ row }) => (
      <div className='flex justify-end'>
        <DataTableRowActions row={row} />
      </div>
    ),
    meta: { className: 'w-[8%] text-center' },
  },
]
