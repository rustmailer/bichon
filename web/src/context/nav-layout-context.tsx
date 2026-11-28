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
import { useCurrentUser } from '@/hooks/use-current-user'

export type NavLayout = 'sidebar' | 'top'

function normalize(value?: string | null): NavLayout | null {
  return value === 'sidebar' || value === 'top' ? value : null
}

// The login response carries the saved layout; remembering it avoids a
// sidebar -> top jump while `current-user` is still loading after sign-in.
let loginHint: NavLayout | null = null

export function rememberNavLayout(layout?: string | null) {
  // Always overwrite: a profile without a stored choice means the sidebar, it
  // must not keep the previous account's `top` hint alive.
  loginHint = normalize(layout) ?? 'sidebar'
}

interface NavLayoutContextValue {
  layout: NavLayout
  setLayout: (layout: NavLayout) => void
}

// A default value rather than a thrown error: pieces such as the page header
// also render in isolated tests and previews, where no provider is mounted.
const NavLayoutContext = React.createContext<NavLayoutContextValue>({
  layout: 'sidebar',
  setLayout: () => {},
})

export function NavLayoutProvider({ children }: { children: React.ReactNode }) {
  const { user } = useCurrentUser()
  const [layout, setLayoutState] = React.useState<NavLayout>(loginHint ?? 'sidebar')

  // The preference lives on the user profile (PUT /users/:id), so it arrives
  // with the current-user query rather than from localStorage.
  React.useEffect(() => {
    if (!user) return
    setLayoutState(normalize(user.nav_layout) ?? 'sidebar')
  }, [user, user?.nav_layout])

  const setLayout = React.useCallback((next: NavLayout) => {
    setLayoutState(next)
  }, [])

  const value = React.useMemo(
    () => ({ layout, setLayout }),
    [layout, setLayout]
  )

  return (
    <NavLayoutContext.Provider value={value}>
      {children}
    </NavLayoutContext.Provider>
  )
}

export function useNavLayout() {
  return React.useContext(NavLayoutContext)
}

/**
 * The bar needs room for the brand, six group menus and the utility cluster,
 * so it only takes over from lg up. Narrower screens keep the sidebar (and
 * its drawer), which also remains the only navigation on a phone.
 */
const WIDE_QUERY = '(min-width: 1024px)'

function useIsWide() {
  const [wide, setWide] = React.useState<boolean>(() => {
    if (typeof window === 'undefined' || !window.matchMedia) return false
    return window.matchMedia(WIDE_QUERY).matches
  })

  React.useEffect(() => {
    if (typeof window === 'undefined' || !window.matchMedia) return
    const mql = window.matchMedia(WIDE_QUERY)
    const onChange = () => setWide(mql.matches)
    onChange()
    mql.addEventListener('change', onChange)
    return () => mql.removeEventListener('change', onChange)
  }, [])

  return wide
}

/** Whether the top bar is the active navigation right now. */
export function useIsTopNav() {
  const { layout } = useNavLayout()
  const wide = useIsWide()
  return layout === 'top' && wide
}
