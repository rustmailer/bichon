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


import { Fragment, type JSX } from 'react'
import { Link, useLocation, useNavigate } from '@tanstack/react-router'
import { cn } from '@/lib/utils'
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectLabel,
  SelectSeparator,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { useTranslation } from 'react-i18next'

interface SettingsNavItem {
  href: string
  title: string
  icon: JSX.Element
  /** Section heading; consecutive items with the same group share one. */
  group?: string
}

interface SettingsNavProps extends React.HTMLAttributes<HTMLElement> {
  items: SettingsNavItem[]
}

export default function SettingsNav({
  className,
  items,
  ...props
}: SettingsNavProps) {
  const { t } = useTranslation()
  const { pathname } = useLocation()
  const navigate = useNavigate()

  // `/settings` renders the profile panel, so treat it as profile when
  // resolving the active group and the active row.
  const activeHref = pathname === '/settings' ? '/settings/profile' : pathname

  // Consecutive items with the same group form one section. Groups with no
  // visible item never reach this component, so no tab is rendered for them.
  const groups = items.reduce<{ name: string; items: SettingsNavItem[] }[]>(
    (acc, item) => {
      const name = item.group ?? ''
      const last = acc[acc.length - 1]
      if (last && last.name === name) {
        last.items.push(item)
      } else {
        acc.push({ name, items: [item] })
      }
      return acc
    },
    []
  )

  const activeGroup =
    groups.find((group) => group.items.some((item) => item.href === activeHref)) ??
    groups[0]

  const handleSelect = (href: string) => {
    navigate({ to: href })
  }

  return (
    <>
      <div className='p-1 md:hidden'>
        <Select value={activeHref} onValueChange={handleSelect}>
          <SelectTrigger className='h-12 sm:w-48'>
            <SelectValue placeholder={t('settings.theme')} />
          </SelectTrigger>
          <SelectContent>
            {/* Radix requires a group around every label; one group for the
                whole list is enough — the separators keep the sections. */}
            <SelectGroup>
              {items.map((item, index) => {
                const startsGroup =
                  item.group !== undefined &&
                  (index === 0 || items[index - 1].group !== item.group)
                return (
                  <Fragment key={item.href}>
                    {startsGroup && (
                      <>
                        {index > 0 && <SelectSeparator />}
                        <SelectLabel className='text-xs font-semibold uppercase tracking-wide text-muted-foreground'>
                          {item.group}
                        </SelectLabel>
                      </>
                    )}
                    <SelectItem value={item.href}>
                      <div className='flex gap-x-4 px-2 py-1'>
                        <span className='scale-125'>{item.icon}</span>
                        <span className='text-md'>{item.title}</span>
                      </div>
                    </SelectItem>
                  </Fragment>
                )
              })}
            </SelectGroup>
          </SelectContent>
        </Select>
      </div>

      <div className='hidden shrink-0 md:block'>
        <div
          className={cn(
            'no-scrollbar overflow-x-auto px-1 pb-2 pt-1',
            className
          )}
          {...props}
        >
          <div className='inline-flex min-w-max items-center gap-1 rounded-lg bg-muted p-1'>
            {groups.map((group) => (
              <button
                key={group.name}
                type='button'
                onClick={() => {
                  if (group !== activeGroup) {
                    handleSelect(group.items[0].href)
                  }
                }}
                aria-current={group === activeGroup ? 'true' : undefined}
                className={cn(
                  'h-8 shrink-0 whitespace-nowrap rounded-md px-3 text-sm font-medium transition-colors',
                  group === activeGroup
                    ? 'bg-background text-foreground shadow-sm'
                    : 'text-muted-foreground hover:text-foreground'
                )}
              >
                {group.name}
              </button>
            ))}
          </div>
        </div>

        {/* A group with a single entry needs no second row: the group tab is
            the entry. */}
        {activeGroup && activeGroup.items.length > 1 && (
          <nav
            aria-label={activeGroup.name}
            className='no-scrollbar flex items-center gap-6 overflow-x-auto border-b px-8'
          >
            {activeGroup.items.map((item) => (
              <Link
                key={item.href}
                to={item.href}
                aria-current={item.href === activeHref ? 'page' : undefined}
                className={cn(
                  'inline-flex h-10 shrink-0 items-center whitespace-nowrap border-b-2 text-xs transition-colors',
                  item.href === activeHref
                    ? 'border-primary font-medium text-foreground'
                    : 'border-transparent text-muted-foreground hover:text-foreground'
                )}
              >
                {item.title}
              </Link>
            ))}
          </nav>
        )}
      </div>
    </>
  )
}
