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

import { createFileRoute, Outlet } from '@tanstack/react-router'
import { cn } from '@/lib/utils'
import { SearchProvider } from '@/context/search-context'
import {
  NavLayoutProvider,
  useIsTopNav,
} from '@/context/nav-layout-context'
import { SidebarProvider } from '@/components/ui/sidebar'
import { ScrollArea } from '@/components/ui/scroll-area'
import { AppSidebar } from '@/components/layout/app-sidebar'
import { TopNav } from '@/components/layout/top-nav'

export const Route = createFileRoute('/_authenticated')({
  component: RouteComponent,
})

function RouteComponent() {
  return (
    <NavLayoutProvider>
      <SearchProvider>
        <AuthenticatedShell />
      </SearchProvider>
    </NavLayoutProvider>
  )
}

function AuthenticatedShell() {
  const defaultOpen = localStorage.getItem('sidebar_state') === 'true';
  const topNav = useIsTopNav()

  // Top navigation: a horizontal bar above the content, no sidebar rail. The
  // provider stays mounted because it also hosts the tooltip context the
  // pages rely on; `flex-col` turns its row wrapper into a column.
  if (topNav) {
    return (
      <SidebarProvider className='flex h-svh flex-col overflow-hidden'>
        <TopNav />
        <div id='content' className='flex min-h-0 w-full flex-1 flex-col'>
          <ScrollArea
            className='min-h-0 flex-1'
            viewportClassName='flex h-full w-full flex-col'
          >
            <Outlet />
          </ScrollArea>
        </div>
      </SidebarProvider>
    )
  }

  return (
    <SidebarProvider defaultOpen={defaultOpen}>
      {/* <SkipToMain /> */}
      <AppSidebar />
      <div
        id='content'
        className={cn(
          'max-w-full w-full ml-auto',
          'peer-data-[state=collapsed]:w-[calc(100%-var(--sidebar-width-icon)-1rem)]',
          'peer-data-[state=expanded]:w-[calc(100%-var(--sidebar-width))]',
          'transition-[width] ease-linear duration-200',
          'h-svh flex flex-col',
          'group-data-[scroll-locked=1]/body:h-full',
          'group-data-[scroll-locked=1]/body:has-[main.fixed-main]:h-svh'
        )}
      >
        <ScrollArea
          className='min-h-0 flex-1'
          viewportClassName='flex h-full w-full flex-col'
        >
          <Outlet />
        </ScrollArea>
      </div>
    </SidebarProvider>
  )
}
