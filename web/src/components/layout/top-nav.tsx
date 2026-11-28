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

import { Link, useLocation } from '@tanstack/react-router'
import Logo from '@/assets/logo.svg'
import { resolveApiUrl } from '@/api/branding/api'
import { useBranding } from '@/hooks/use-branding'
import { editionBrandName, useEdition } from '@/hooks/use-edition'
import {
  NavigationMenu,
  NavigationMenuContent,
  NavigationMenuItem,
  NavigationMenuLink,
  NavigationMenuList,
  NavigationMenuTrigger,
} from '@/components/ui/navigation-menu'
import { cn } from '@/lib/utils'
import { useSidebarData } from './data/sidebar-data'
import { HeaderActions } from './fixed-header'
import type { NavItem, NavLink } from './types'

/** Collapsible entries are flattened: a top bar has no room for a third level. */
function visibleLinks(items: NavItem[]): NavLink[] {
  const links: NavLink[] = []
  for (const item of items) {
    if (item.visible === false) continue
    if ('items' in item && item.items) {
      for (const nested of item.items) {
        if (nested.visible === false) continue
        links.push({ ...nested, title: `${item.title} · ${nested.title}` })
      }
    } else {
      links.push(item as NavLink)
    }
  }
  return links
}

function isActive(pathname: string, url?: string) {
  if (!url) return false
  if (url === '/') return pathname === '/'
  const base = url.endsWith('/') ? url.slice(0, -1) : url
  return pathname === base || pathname.startsWith(`${base}/`)
}

export function TopNav() {
  const { navGroups } = useSidebarData()
  const { pathname } = useLocation()
  const { edition, version } = useEdition()
  const { companyName, logoUrl } = useBranding()
  const logo = logoUrl ? resolveApiUrl(logoUrl) : Logo
  const displayName = companyName || editionBrandName(edition)

  return (
    <header className='relative z-40 flex h-14 shrink-0 items-center gap-4 border-b bg-background px-4'>
      <Link to='/' className='flex shrink-0 items-center gap-2'>
        <img
          src={logo}
          alt={displayName}
          className='h-7 w-7 rounded-md object-contain'
        />
        <span className='hidden text-sm font-semibold md:inline'>
          {displayName}
        </span>
      </Link>

      <NavigationMenu viewport={false} className='max-w-full justify-start'>
        <NavigationMenuList className='justify-start'>
          {navGroups.map((group) => {
            const links = visibleLinks(group.items)
            if (links.length === 0) return null
            const groupActive = links.some((link) => isActive(pathname, link.url))

            return (
              <NavigationMenuItem key={group.id} className='relative'>
                <NavigationMenuTrigger
                  className={cn(
                    'h-9 bg-transparent',
                    groupActive && 'bg-accent text-accent-foreground'
                  )}
                >
                  {group.title}
                </NavigationMenuTrigger>
                {/* Anchored to its own trigger: each group's list opens right
                    below that group instead of at one shared panel position. */}
                <NavigationMenuContent className='top-full mt-1 w-64 rounded-md border bg-popover p-2 shadow-md md:w-64'>
                  <ul className='grid gap-1'>
                    {links.map((link) => {
                      const Icon = link.icon
                      const active = isActive(pathname, link.url)
                      return (
                        <li key={`${String(link.url)}-${link.title}`}>
                          <NavigationMenuLink asChild active={active}>
                            <Link
                              to={link.url}
                              className={cn(
                                'flex select-none items-center gap-2 rounded-md p-2 text-sm no-underline outline-none transition-colors hover:bg-accent hover:text-accent-foreground focus:bg-accent focus:text-accent-foreground',
                                active && 'bg-accent text-accent-foreground'
                              )}
                            >
                              {Icon ? <Icon className='h-4 w-4 shrink-0' /> : null}
                              <span className='truncate'>{link.title}</span>
                            </Link>
                          </NavigationMenuLink>
                        </li>
                      )
                    })}
                  </ul>
                </NavigationMenuContent>
              </NavigationMenuItem>
            )
          })}
        </NavigationMenuList>
      </NavigationMenu>

      <div className='flex-1' />
      {edition ? (
        <span className='hidden shrink-0 items-center gap-1 rounded-full border border-primary/30 bg-primary/10 px-2 py-0.5 text-[10px] font-semibold uppercase tracking-wide text-primary xl:inline-flex'>
          {edition}
          {version ? (
            <span className='font-normal normal-case tracking-normal opacity-80'>
              v{version}
            </span>
          ) : null}
        </span>
      ) : null}
      <HeaderActions />
    </header>
  )
}
