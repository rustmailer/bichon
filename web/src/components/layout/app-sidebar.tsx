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
  SidebarHeader,
  SidebarMenuButton,
  useSidebar,
} from '@/components/ui/sidebar'
import { NavGroup } from '@/components/layout/nav-group'
import Logo from '@/assets/logo.svg'
import { useSidebarData } from './data/sidebar-data'
import { useEdition } from '@/hooks/use-edition'
import { Link } from '@tanstack/react-router';

export function AppSidebar({ ...props }: React.ComponentProps<typeof Sidebar>) {
  const { open } = useSidebar();
  const sidebarData = useSidebarData();
  const { isPro } = useEdition();
  return (
    <Sidebar collapsible='icon' variant='sidebar' {...props}>
      <SidebarHeader>
        <SidebarMenuButton
          size='lg'
          asChild
          className='data-[state=open]:bg-sidebar-accent data-[state=open]:text-sidebar-accent-foreground'
        >
          <Link to="/">
            <div className='flex aspect-square size-16 items-center justify-center rounded-lg text-sidebar-primary-foreground'>
              <img
                className={open ? "relative ml-[12px] mr-[12px]" : "mr-[30px]"}
                src={Logo}
                width={open ? 60 : 40}
                height={open ? 60 : 40}
                alt='Logo'
              />
            </div>
            <div className='grid flex-1 text-left text-lg leading-tight'>
              <span className='truncate font-semibold'>
                {isPro ? 'Bichon Pro' : 'Bichon'}
              </span>
            </div>
          </Link>
        </SidebarMenuButton>
      </SidebarHeader>
      <SidebarContent>
        {sidebarData.navGroups.map((props) => (
          <NavGroup key={props.title} {...props} />
        ))}
      </SidebarContent>
    </Sidebar>
  )
}
