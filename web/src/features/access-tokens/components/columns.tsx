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


import { ColumnDef } from '@tanstack/react-table'
import LongText from '@/components/long-text'

import { AccessToken } from '../data/schema'
import { DataTableColumnHeader } from './data-table-column-header'
import { DataTableRowActions } from './data-table-row-actions'
import { format, formatDistanceToNow, Locale } from 'date-fns'
import { AccountCellAction } from './account-action'
import { AclCellAction } from './acl-action'

export const getColumns = (t: (key: string) => string, locale: Locale): ColumnDef<AccessToken>[] => [
  {
    accessorKey: 'token',
    header: ({ column }) => (
      <DataTableColumnHeader column={column} title={t('settings.token')} />
    ),
    cell: ({ row }) => {
      return <LongText className='w-40'>{row.original.token}</LongText>
    },
    meta: { className: 'w-40' },
    enableHiding: false,
    enableSorting: false,
  },
  {
    accessorKey: 'accounts',
    header: ({ column }) => (
      <DataTableColumnHeader column={column} title={t('settings.accounts')} />
    ),
    cell: AccountCellAction,
    meta: { className: 'w-10 text-center' },
    filterFn: (row, columnId, filterValue) => {
      const accounts = row.getValue(columnId) as { account_id: number; email: string }[];
      if (!filterValue) return true;
      return accounts.some(
        (account) =>
          `${account.account_id}`.includes(filterValue) ||
          account.email.includes(filterValue)
      );
    },
  },
  {
    id: 'acl',
    header: ({ column }) => (
      <DataTableColumnHeader column={column} title={t('settings.acl')} />
    ),
    cell: AclCellAction,
    meta: { className: 'w-8 text-center' },
    enableSorting: false
  },
  {
    accessorKey: 'description',
    header: ({ column }) => (
      <DataTableColumnHeader column={column} title={t('settings.description')} />
    ),
    cell: ({ row }) => (
      <LongText className='max-w-80'>{row.original.description}</LongText>
    ),
    meta: { className: 'w-80' },
    enableHiding: true,
    enableSorting: false
  },
  {
    accessorKey: 'created_at',
    header: ({ column }) => (
      <DataTableColumnHeader column={column} title={t('settings.createdAt')} />
    ),
    cell: ({ row }) => {
      const created_at = row.original.created_at;
      const date = format(new Date(created_at), 'yyyy-MM-dd HH:mm:ss');
      return <LongText className='max-w-36'>{date}</LongText>;
    },
    meta: { className: 'w-36' },
    enableHiding: false,
  },
  {
    accessorKey: 'updated_at',
    header: ({ column }) => (
      <DataTableColumnHeader column={column} title={t('settings.updatedAt')} />
    ),
    cell: ({ row }) => {
      const updated_at = row.original.updated_at;
      const date = format(new Date(updated_at), 'yyyy-MM-dd HH:mm:ss');
      return <LongText className='max-w-36'>{date}</LongText>;
    },
    meta: { className: 'w-36' },
    enableHiding: false,
  },
  {
    accessorKey: 'last_access_at',
    header: ({ column }) => (
      <DataTableColumnHeader column={column} title={t('settings.lastAccess')} />
    ),
    cell: ({ row }) => {
      const last_access_at = row.original.last_access_at;
      if (last_access_at === 0) {
        return <LongText className='max-w-40'>{t('accessTokens.notUsedYet')}</LongText>;
      }
      const result = formatDistanceToNow(new Date(last_access_at), { addSuffix: true, locale });
      return <LongText className='max-w-40'>{result}</LongText>;
    },
    meta: { className: 'w-40' },
    enableHiding: false,
  },
  {
    id: 'actions',
    cell: DataTableRowActions,
  },
]
