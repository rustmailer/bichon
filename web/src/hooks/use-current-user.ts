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


import { useQuery } from '@tanstack/react-query'
import { AxiosError } from 'axios'
import { get_current_user, User } from '@/api/users/api'
import { useMemo } from 'react'
import { getToken } from '@/stores/authStore'

export function useCurrentUser() {
  const token = getToken();

  const query = useQuery<User | null, AxiosError>({
    queryKey: ['current-user'],
    enabled: !!token,
    queryFn: get_current_user,
    retry: false,
  })

  const permission = useMemo(() => {
    if (!query.data) {
      return {
        canGlobal: () => false,
        canAccount: () => false,
        require_any_permission: () => false,
      }
    }

    const globalSet = new Set(query.data.global_permissions ?? [])
    const accountMap = new Map<number, Set<string>>(
      Object.entries(query.data.account_permissions ?? {}).map(
        ([id, perms]) => [Number(id), new Set(perms)],
      ),
    )

    return {
      canGlobal(permission: string) {
        return globalSet.has(permission)
      },

      canAccount(accountId: number, permission: string) {
        return accountMap.get(accountId)?.has(permission) ?? false
      },

      require_any_permission(permissions: string[], accountId?: number) {
        return permissions.some(perm => {
          if (globalSet.has(perm)) return true
          if (accountId !== undefined) {
            return accountMap.get(accountId)?.has(perm) ?? false
          }
          return false
        })
      }
    }
  }, [query.data])

  return {
    ...query,
    user: query.data,
    ...permission,
  }
}