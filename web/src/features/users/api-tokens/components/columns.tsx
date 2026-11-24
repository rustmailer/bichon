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
import { DataTableColumnHeader } from './data-table-column-header'
import { DataTableRowActions } from './data-table-row-actions'
import { format } from 'date-fns'
import { AccessToken } from '@/api/users/api'
import { Badge } from '@/components/ui/badge'

export const getColumns = (t: (key: string) => string): ColumnDef<AccessToken>[] => {
  return [
    {
      accessorKey: 'name',
      header: ({ column }) => (
        <DataTableColumnHeader
          column={column}
          title={t('users.api_tokens.table.name')}
        />
      ),
      cell: ({ row }) => (
        <div className="flex flex-col py-1">
          <span className="font-medium text-foreground">
            {row.original.name || t('users.api_tokens.table.unnamed')}
          </span>
          <span className="text-[11px] text-muted-foreground font-mono leading-none mt-1">
            {row.original.token_prefix}...
          </span>
        </div>
      ),
      enableHiding: false,
    },
    {
      accessorKey: 'token_type',
      header: ({ column }) => (
        <DataTableColumnHeader
          column={column}
          title={t('users.api_tokens.table.type')}
          className="justify-center"
        />
      ),
      cell: ({ row }) => (
        <div className="flex items-center justify-center gap-1">
          <Badge
            variant={row.original.token_type === 'Api' ? 'default' : 'secondary'}
            className="font-normal shadow-none"
          >
            {row.original.token_type === 'Api'
              ? t('users.api_tokens.table.api_key')
              : t('users.api_tokens.table.web_ui')}
          </Badge>
          {row.original.metrics_only && (
            <Badge variant="outline" className="font-normal shadow-none">
              {t('apiTokens.list.metricsOnlyBadge')}
            </Badge>
          )}
        </div>
      ),
      meta: { className: 'text-center' },
    },
    {
      // Virtual column: not rendered, used by the toolbar's metrics-only
      // filter (hidden via the table's initial column visibility).
      id: 'metrics_only',
      accessorFn: (row) => (row.metrics_only ? 'metrics' : 'regular'),
      filterFn: (row, _id, value) => {
        if (!value || value === 'all') return true
        return value === 'metrics' ? !!row.original.metrics_only : !row.original.metrics_only
      },
      enableSorting: false,
      enableHiding: false,
      header: () => null,
      cell: () => null,
    },
    {
      accessorKey: 'last_access_at',
      header: ({ column }) => (
        <DataTableColumnHeader
          column={column}
          title={t('users.api_tokens.table.last_used')}
          className="justify-center"
        />
      ),
      cell: ({ row }) => {
        const last = row.original.last_access_at
        return (
          <div className="text-center text-xs text-muted-foreground">
            {last > 0
              ? format(new Date(last), 'yyyy-MM-dd HH:mm')
              : t('users.api_tokens.table.never')}
          </div>
        )
      },
      meta: { className: 'text-center' },
    },
    {
      accessorKey: 'expire_at',
      header: ({ column }) => (
        <DataTableColumnHeader
          column={column}
          title={t('users.api_tokens.table.expires_at')}
          className="justify-center"
        />
      ),
      cell: ({ row }) => {
        const expireAt = row.original.expire_at
        if (!expireAt) {
          return (
            <div className="text-center text-xs text-muted-foreground">
              {t('users.api_tokens.table.permanent')}
            </div>
          )
        }

        const isExpired = new Date(expireAt) < new Date()
        return (
          <div
            className={`text-center text-xs ${isExpired
              ? 'text-destructive font-bold'
              : 'text-muted-foreground'
              }`}
          >
            {format(new Date(expireAt), 'yyyy-MM-dd HH:mm')}
            {isExpired && (
              <span className="ml-1">
                ({t('users.api_tokens.table.expired')})
              </span>
            )}
          </div>
        )
      },
      meta: { className: 'text-center' },
    },
    {
      accessorKey: 'owner',
      header: ({ column }) => (
        <DataTableColumnHeader
          column={column}
          title={t('users.api_tokens.table.owner')}
        />
      ),
      cell: ({ row }) => {
        const { user_name, user_email } = row.original
        return (
          <div className="flex flex-col py-1 text-left">
            <span className="text-sm font-medium text-foreground">
              {user_name}
            </span>
            <span className="text-[11px] text-muted-foreground font-mono">
              {user_email}
            </span>
          </div>
        )
      },
    },
    {
      accessorKey: 'created_at',
      header: ({ column }) => (
        <DataTableColumnHeader
          column={column}
          title={t('users.api_tokens.table.created')}
          className="justify-center"
        />
      ),
      cell: ({ row }) => (
        <div className="text-center text-xs text-muted-foreground">
          {format(new Date(row.original.created_at), 'yyyy-MM-dd HH:mm')}
        </div>
      ),
      meta: { className: 'text-center' },
    },
    {
      id: 'actions',
      cell: DataTableRowActions,
    },
  ]
}
