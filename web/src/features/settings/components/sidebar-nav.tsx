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


import { Fragment, useState, type JSX } from 'react'
import { useLocation, useNavigate } from '@tanstack/react-router'
import { Link } from '@tanstack/react-router'
import { cn } from '@/lib/utils'
import { buttonVariants } from '@/components/ui/button'
import { ScrollArea } from '@/components/ui/scroll-area'
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

interface SidebarNavProps extends React.HTMLAttributes<HTMLElement> {
  items: {
    href: string
    title: string
    icon: JSX.Element
    /** Section heading; consecutive items with the same group share one. */
    group?: string
  }[]
}

export default function SidebarNav({
  className,
  items,
  ...props
}: SidebarNavProps) {
  const { t } = useTranslation()
  const { pathname } = useLocation()
  const navigate = useNavigate()
  const [val, setVal] = useState(pathname ?? '/settings')

  const handleSelect = (e: string) => {
    setVal(e)
    navigate({ to: e })
  }

  return (
    <>
      <div className='p-1 md:hidden'>
        <Select value={val} onValueChange={handleSelect}>
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

      <ScrollArea
        orientation='horizontal'
        type='always'
        className='hidden w-full bg-background px-1 py-2 md:block min-w-40'
      >
        <nav
          className={cn(
            'flex py-1 space-x-2 lg:flex-col lg:space-x-0 lg:space-y-1',
            className
          )}
          {...props}
        >
          {items.map((item, index) => {
            // Headings are only rendered in the vertical column layout; the
            // horizontal tablet layout stays a flat row of links.
            const startsGroup =
              item.group !== undefined &&
              (index === 0 || items[index - 1].group !== item.group)
            return (
              <Fragment key={item.href}>
                {startsGroup && (
                  <div className='hidden px-3 pt-4 text-xs font-semibold uppercase tracking-wide text-muted-foreground first:pt-0 lg:block'>
                    {item.group}
                  </div>
                )}
                <Link
                  to={item.href}
                  className={cn(
                    buttonVariants({ variant: 'ghost' }),
                    pathname === item.href
                      ? 'bg-muted hover:bg-muted'
                      : 'hover:bg-transparent hover:underline',
                    'justify-start'
                  )}
                >
                  <span className='mr-2'>{item.icon}</span>
                  {item.title}
                </Link>
              </Fragment>
            )
          })}
        </nav>
      </ScrollArea>
    </>
  )
}
