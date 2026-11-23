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


import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarHeader,
  SidebarMenuButton,
} from '@/components/ui/sidebar'
import { NavGroup } from '@/components/layout/nav-group'
import Logo from '@/assets/logo.svg'
import { resolveApiUrl } from '@/api/branding/api'
import { useBranding } from '@/hooks/use-branding'
import { AutoFitText } from '@/components/branding/auto-fit-text'
import { useSidebarData } from './data/sidebar-data'
import { editionBrandName, useEdition } from '@/hooks/use-edition'
import { Link } from '@tanstack/react-router';

export function AppSidebar({ ...props }: React.ComponentProps<typeof Sidebar>) {
  const sidebarData = useSidebarData();
  const { edition, version } = useEdition();
  const { companyName, logoUrl } = useBranding();
  const logo = logoUrl ? resolveApiUrl(logoUrl) : Logo;
  const displayName = companyName || editionBrandName(edition);
  return (
    <Sidebar collapsible='icon' variant='sidebar' {...props}>
      <SidebarHeader>
        <SidebarMenuButton
          asChild
          className='h-auto flex-col items-center gap-1 rounded-lg px-2 py-2 group-data-[collapsible=icon]:!p-0 data-[state=open]:bg-sidebar-accent data-[state=open]:text-sidebar-accent-foreground'
        >
          <Link to="/">
            <div className='flex size-12 shrink-0 items-center justify-center overflow-hidden'>
              <img
                src={logo}
                alt={displayName}
                className='max-h-full max-w-full object-contain'
              />
            </div>
            <AutoFitText
              text={displayName}
              title={displayName}
              className='flex w-full justify-center'
            />
          </Link>
        </SidebarMenuButton>
      </SidebarHeader>
      <SidebarContent>
        {sidebarData.navGroups.map((props) => (
          <NavGroup key={props.title} {...props} />
        ))}
      </SidebarContent>
      {/* Keeps the bottom-most nav item off the sidebar's bottom edge: an
          edition badge + version caption always reserves a few rows, and in
          icon-collapsed mode the caption hides but the footer still pads the
          icon rail. Edition names are the raw license values (English), no i18n. */}
      <SidebarFooter>
        <div className='flex h-8 items-center justify-center px-2'>
          {version && (
            <span className='flex min-w-0 items-center gap-2 group-data-[collapsible=icon]:hidden'>
              <span className='shrink-0 rounded-full border border-primary/30 bg-primary/10 px-2 py-0.5 text-[10px] font-semibold uppercase tracking-wide text-primary'>
                {edition}
              </span>
              <span className='truncate text-xs text-muted-foreground'>
                v{version}
              </span>
            </span>
          )}
        </div>
      </SidebarFooter>
    </Sidebar>
  )
}
