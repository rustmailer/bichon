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
import { Badge } from '@/components/ui/badge'
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from '@/components/ui/tooltip'
import { avatarDataUrl } from '@/lib/avatar'
import { User, UserRole } from '@/api/users/api'
import { ClockIcon, LockIcon } from 'lucide-react'

export const getColumns = (t: (key: string) => string, roles: UserRole[]): ColumnDef<User>[] => [
  {
    accessorKey: 'id',
    header: ({ column }) => (
      <DataTableColumnHeader column={column} title={t('users.columns.id')} className="justify-center" />
    ),
    cell: ({ row }) => (
      <LongText className='text-center'>{`${row.original.id}`}</LongText>
    ),
    enableHiding: false,
    meta: { className: 'w-[7%]' },
    enableSorting: false
  },
  {
    accessorKey: "avatar",
    header: ({ column }) => (
      <DataTableColumnHeader column={column} title={t('users.columns.avatar')} className="justify-center" />
    ),
    cell: ({ row }) => {
      if (row.original.avatar) {
        const avatarSrc = avatarDataUrl(row.original.avatar);
        return <div className="flex justify-center">
          <img
            src={avatarSrc}
            alt="Avatar"
            className="h-8 w-8 rounded-full object-cover"
          />
        </div>
      } else {
        return <LongText>{t('common.na')}</LongText>
      }
    },
    meta: { className: 'w-[6%] text-center' },
  },
  {
    accessorKey: "username",
    header: ({ column }) => (
      <DataTableColumnHeader column={column} title={t('users.columns.username')} className="justify-center" />
    ),
    cell: ({ row }) => {
      return <LongText className='text-center'>{row.original.username}</LongText>
    },
    meta: { className: 'w-[13%] text-center' },
  },
  {
    accessorKey: "email",
    header: ({ column }) => (
      <DataTableColumnHeader column={column} title={t('users.columns.email')} className="justify-center" />
    ),
    cell: ({ row }) => {
      return <LongText className='text-center'>{row.original.email}</LongText>
    },
    meta: { className: 'w-[19%] text-center' },
  },
  {
    id: 'roles',
    header: ({ column }) => (
      <DataTableColumnHeader column={column} title={t('users.columns.roles')} className="justify-center" />
    ),
    cell: ({ row }) => {
      const user = row.original
      const userRoleIds = user.global_roles || []

      // Lapsed delegations are dropped from this column: `global_roles` keeps
      // the grant so it can be listed and renewed, but a role that grants
      // nothing is not a role the user holds. Showing it struck through would
      // say "holds this, expired"; the truth is "does not hold this".
      const now = Date.now()
      const liveRoleIds = userRoleIds.filter(
        (rid) => (user.global_role_expiries?.[rid] ?? Infinity) > now
      )

      const mapped = liveRoleIds
        .map((rid) => roles.find((r) => r.id === rid))
        .filter(Boolean) as UserRole[]

      if (mapped.length === 0) {
        return <span className="text-muted-foreground">-</span>
      }

      return (
        <div className="flex w-full flex-col items-center gap-1">
          {mapped.map((role) => {
            // A deadline is shown on the role itself rather than in a
            // separate column: "Manager" and "Manager until 31 Dec" are the
            // same grant, and splitting them apart is how a reviewer misses
            // that an access is about to lapse (or quietly renews it).
            const expiresAt = user.global_role_expiries?.[role.id]
            return (
              <Badge
                key={role.id}
                variant="outline"
                className="min-w-0 max-w-full"
              >
                {/* The table is table-fixed, so the badge cannot widen its
                    column. The trigger is the truncating element itself —
                    wrapping it in LongText would put a min-width:auto block
                    between the flex item and the ellipsis, which stops the
                    shrink. Hovering the name shows it in full. */}
                <TooltipProvider delayDuration={0}>
                  <Tooltip>
                    <TooltipTrigger asChild>
                      <span className="min-w-0 truncate">{role.name}</span>
                    </TooltipTrigger>
                    <TooltipContent className="max-w-sm break-words">
                      {role.name}
                    </TooltipContent>
                  </Tooltip>
                </TooltipProvider>
                {expiresAt !== undefined && (
                  <span className="ml-1 inline-flex shrink-0 items-center gap-0.5 font-normal">
                    <ClockIcon className="h-3 w-3" />
                    {format(new Date(expiresAt), 'yyyy-MM-dd')}
                  </span>
                )}
                {user.id === 100000000000000 && <LockIcon className="ml-1 h-3 w-3 shrink-0 text-muted-foreground" />}
              </Badge>
            )
          })}
        </div>
      )
    },
    meta: { className: 'w-[21%] text-center' },
    enableHiding: false,
  },
  {
    accessorKey: 'created_at',
    header: ({ column }) => (
      <DataTableColumnHeader column={column} title={t('users.columns.created_at')} className="justify-center" />
    ),
    cell: ({ row }) => {
      const created_at = row.original.created_at;
      const date = format(new Date(created_at), 'yyyy-MM-dd HH:mm:ss');
      return <LongText className='text-center'>{date}</LongText>;
    },
    meta: { className: 'w-[13%] text-center' },
    enableHiding: false,
  },
  {
    accessorKey: 'updated_at',
    header: ({ column }) => (
      <DataTableColumnHeader column={column} title={t('users.columns.updated_at')} className="justify-center" />
    ),
    cell: ({ row }) => {
      const updated_at = row.original.updated_at;
      const date = format(new Date(updated_at), 'yyyy-MM-dd HH:mm:ss');
      return <LongText className='text-center'>{date}</LongText>;
    },
    meta: { className: 'w-[13%] text-center' },
    enableHiding: false,
  },
  {
    id: 'actions',
    header: ({ column }) => (
      <DataTableColumnHeader column={column} title={t('users.columns.actions')} className="text-right" />
    ),
    cell: ({ row }) => (
      <div className='flex justify-end'>
        <DataTableRowActions row={row} />
      </div>
    ),
    meta: { className: 'w-[8%] text-center' },
  },
]
