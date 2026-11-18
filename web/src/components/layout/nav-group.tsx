import { ReactNode, useEffect, useState } from 'react'
import { Link, useLocation } from '@tanstack/react-router'
import { ChevronRight } from 'lucide-react'
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from '@/components/ui/collapsible'
import {
  SidebarGroup,
  SidebarGroupLabel,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
  SidebarMenuSub,
  SidebarMenuSubButton,
  SidebarMenuSubItem,
  useSidebar,
} from '@/components/ui/sidebar'
import { Badge } from '../ui/badge'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '../ui/dropdown-menu'
import { NavCollapsible, NavItem, NavLink, type NavGroup } from './types'

// One localStorage key holding the open/close state of every nav group (a
// JSON object keyed by group id), so a reload restores the sidebar the user
// last had it in. Deliberately a single key rather than one per group.
const NAV_GROUP_STORAGE_KEY = 'bichon.sidebar.nav-group-state'

function readStoredOpen(id: string): boolean | undefined {
  try {
    const state = JSON.parse(localStorage.getItem(NAV_GROUP_STORAGE_KEY) ?? '{}')
    return typeof state[id] === 'boolean' ? state[id] : undefined
  } catch {
    return undefined
  }
}

function storeOpen(id: string, open: boolean) {
  try {
    const state = JSON.parse(localStorage.getItem(NAV_GROUP_STORAGE_KEY) ?? '{}')
    state[id] = open
    localStorage.setItem(NAV_GROUP_STORAGE_KEY, JSON.stringify(state))
  } catch {
    // localStorage unavailable (blocked / quota / private mode) — best effort.
  }
}

export function NavGroup({ id, title, items, defaultOpen }: NavGroup) {
  const { state } = useSidebar()
  const href = useLocation({ select: (location) => location.href })

  const visibleItems = items.filter(item => item.visible !== false)

  // A group is "active" when any of its (sub)items matches the current route.
  // The section you are in is never hidden (it starts open and re-opens on
  // navigation); otherwise the user's stored choice wins, else `defaultOpen`.
  const isActive = visibleItems.some(item => checkIsActive(href, item, true))
  const [open, setOpen] = useState(() => {
    if (isActive) return true
    const stored = readStoredOpen(id)
    if (typeof stored === 'boolean') return stored
    return !!defaultOpen
  })

  useEffect(() => {
    if (isActive) setOpen(true)
  }, [isActive])

  // Persist the current open/close choice (including auto-opens from
  // navigation, which match what the user sees) so reloads restore it.
  useEffect(() => {
    storeOpen(id, open)
  }, [id, open])

  if (visibleItems.length === 0) return null

  // In icon-collapsed mode the groups render as a plain icon rail, so every
  // group must stay expanded (collapsing is disabled) to keep all icons visible.
  const isOpen = state === 'collapsed' ? true : open

  return (
    <Collapsible
      asChild
      open={isOpen}
      onOpenChange={(next) => {
        if (state !== 'collapsed') setOpen(next)
      }}
      className='group/group'
    >
      <SidebarGroup>
        <CollapsibleTrigger asChild>
          <SidebarGroupLabel className='cursor-pointer uppercase select-none'>
            {title}
            <ChevronRight className='ml-auto transition-transform duration-200 group-data-[state=open]/group:rotate-90' />
          </SidebarGroupLabel>
        </CollapsibleTrigger>
        <CollapsibleContent className='CollapsibleContent'>
          <SidebarMenu>
            {visibleItems.map((item) => {
              const key = `${item.title}-${item.url}`

              if (!item.items)
                return <SidebarMenuLink key={key} item={item} href={href} />

              if (state === 'collapsed')
                return (
                  <SidebarMenuCollapsedDropdown key={key} item={item} href={href} />
                )

              return <SidebarMenuCollapsible key={key} item={item} href={href} />
            })}
          </SidebarMenu>
        </CollapsibleContent>
      </SidebarGroup>
    </Collapsible>
  )
}

const NavBadge = ({ children }: { children: ReactNode }) => (
  <Badge className='text-xs rounded-full px-1 py-0'>{children}</Badge>
)

const SidebarMenuLink = ({ item, href }: { item: NavLink; href: string }) => {
  const { setOpenMobile } = useSidebar()
  return (
    <SidebarMenuItem>
      <SidebarMenuButton
        asChild
        isActive={checkIsActive(href, item)}
        tooltip={item.title}
      >
        <Link to={item.url} onClick={() => setOpenMobile(false)}>
          {item.icon && <item.icon />}
          <span>{item.title}</span>
          {item.badge && <NavBadge>{item.badge}</NavBadge>}
        </Link>
      </SidebarMenuButton>
    </SidebarMenuItem>
  )
}

const SidebarMenuCollapsible = ({
  item,
  href,
}: {
  item: NavCollapsible
  href: string
}) => {
  const { setOpenMobile } = useSidebar()
  const visibleSubItems = item.items.filter(sub => sub.visible !== false)

  if (visibleSubItems.length === 0) return null

  return (
    <Collapsible
      asChild
      defaultOpen={checkIsActive(href, item, true)}
      className='group/collapsible'
    >
      <SidebarMenuItem>
        <CollapsibleTrigger asChild>
          <SidebarMenuButton tooltip={item.title}>
            {item.icon && <item.icon />}
            <span>{item.title}</span>
            {item.badge && <NavBadge>{item.badge}</NavBadge>}
            <ChevronRight className='ml-auto transition-transform duration-200 group-data-[state=open]/collapsible:rotate-90' />
          </SidebarMenuButton>
        </CollapsibleTrigger>
        <CollapsibleContent className='CollapsibleContent'>
          <SidebarMenuSub>
            {visibleSubItems.map((subItem) => (
              <SidebarMenuSubItem key={subItem.title}>
                <SidebarMenuSubButton
                  asChild
                  isActive={checkIsActive(href, subItem)}
                >
                  <Link to={subItem.url} onClick={() => setOpenMobile(false)}>
                    {subItem.icon && <subItem.icon />}
                    <span>{subItem.title}</span>
                    {subItem.badge && <NavBadge>{subItem.badge}</NavBadge>}
                  </Link>
                </SidebarMenuSubButton>
              </SidebarMenuSubItem>
            ))}
          </SidebarMenuSub>
        </CollapsibleContent>
      </SidebarMenuItem>
    </Collapsible>
  )
}

const SidebarMenuCollapsedDropdown = ({
  item,
  href,
}: {
  item: NavCollapsible
  href: string
}) => {
  const visibleSubItems = item.items.filter(sub => sub.visible !== false)

  if (visibleSubItems.length === 0) return null

  return (
    <SidebarMenuItem>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <SidebarMenuButton
            tooltip={item.title}
            isActive={checkIsActive(href, item)}
          >
            {item.icon && <item.icon />}
            <span>{item.title}</span>
            {item.badge && <NavBadge>{item.badge}</NavBadge>}
            <ChevronRight className='ml-auto transition-transform duration-200 group-data-[state=open]/collapsible:rotate-90' />
          </SidebarMenuButton>
        </DropdownMenuTrigger>
        <DropdownMenuContent side='right' align='start' sideOffset={4}>
          <DropdownMenuLabel>
            {item.title} {item.badge ? `(${item.badge})` : ''}
          </DropdownMenuLabel>
          <DropdownMenuSeparator />
          {visibleSubItems.map((sub) => (
            <DropdownMenuItem key={`${sub.title}-${sub.url}`} asChild>
              <Link
                to={sub.url}
                className={`${checkIsActive(href, sub) ? 'bg-secondary' : ''}`}
              >
                {sub.icon && <sub.icon />}
                <span className='max-w-52 text-wrap'>{sub.title}</span>
                {sub.badge && (
                  <span className='ml-auto text-xs'>{sub.badge}</span>
                )}
              </Link>
            </DropdownMenuItem>
          ))}
        </DropdownMenuContent>
      </DropdownMenu>
    </SidebarMenuItem>
  )
}

function checkIsActive(href: string, item: NavItem, mainNav = false) {
  return (
    href === item.url ||
    href.split('?')[0] === item.url ||
    !!item?.items?.filter((i) => i.url === href).length ||
    (mainNav &&
      href.split('/')[1] !== '' &&
      href.split('/')[1] === item?.url?.split('/')[1])
  )
}