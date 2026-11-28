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

import type { QueryClient } from '@tanstack/react-query'
import { resetToken } from '@/stores/authStore'

/**
 * Browser-level personalization that outlives a session on purpose. The user
 * profile overrides theme and language at login when it carries values; a
 * profile without them should not silently change the language someone reads
 * the sign-in page in.
 */
const KEPT_STORAGE_KEYS = new Set([
  'i18nextLng',
  'vite-ui-theme',
  'vite-ui-theme-follow-system',
])

/**
 * Drop everything the previous account left behind in localStorage: search
 * histories, the last opened mailbox, export resume state and the various
 * table preferences. A whitelist sweep (rather than a list of known keys) so
 * a key added later cannot leak into the next session by omission.
 */
export function clearSessionStorage() {
  if (typeof localStorage === 'undefined') return
  for (const key of Object.keys(localStorage)) {
    if (!KEPT_STORAGE_KEYS.has(key)) localStorage.removeItem(key)
  }
}

/**
 * Forget the previous account entirely: its token, every cached query
 * (user lists, roles, account data, the current-user record) and its local
 * storage. Call this on sign-out and before adopting a new login.
 */
export function clearSessionState(queryClient: QueryClient) {
  resetToken()
  queryClient.removeQueries()
  clearSessionStorage()
}
