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


import { Outlet } from '@tanstack/react-router'
import { Main } from '@/components/layout/main'
import SidebarNav from './components/sidebar-nav'
import { BadgeCheck, Brush, FileCode2, Fingerprint, Inbox, KeyRound, LockKeyhole, Palette, Radar, SettingsIcon, ShieldCheck, UserCog, Users, Waypoints } from 'lucide-react'
import { FixedHeader } from '@/components/layout/fixed-header'
import { useCurrentUser } from '@/hooks/use-current-user'
import { useEdition } from '@/hooks/use-edition'
import { useTranslation } from 'react-i18next'
import { Separator } from '@/components/ui/separator'

export default function Settings() {
  const { t } = useTranslation()
  const { canGlobal, require_any_permission } = useCurrentUser()
  const { isPro, isEnterprise } = useEdition()


  const sidebarNavItems = [
    {
      title: t('settings.sidebar.profile'),
      href: '/settings/profile',
      icon: <UserCog size={18} />,
    },
    {
      title: t('settings.sidebar.access'),
      href: '/settings/access',
      icon: <ShieldCheck size={18} />
    },
    {
      // Per-user security setting: every user manages their own authenticator.
      title: t('settings.sidebar.mfa', 'Two-factor Auth'),
      href: '/settings/mfa',
      icon: <LockKeyhole size={18} />,
    },
    {
      title: t('settings.appearance.title'),
      href: '/settings/appearance',
      icon: <Palette size={18} />,
    },
    {
      title: t('settings.sidebar.branding', 'Branding'),
      href: '/settings/branding',
      icon: <Brush size={18} />,
      visible: isPro && require_any_permission(['system:root', 'user:manage']),
    },
    {
      title: t('settings.sidebar.sso', 'Single Sign-On'),
      href: '/settings/sso',
      icon: <Fingerprint size={18} />,
      visible: isPro && require_any_permission(['system:root', 'user:manage']),
    },
    {
      title: t('settings.sidebar.siem', 'SIEM'),
      href: '/settings/siem',
      icon: <Radar size={18} />,
      visible: isEnterprise && require_any_permission(['system:root', 'user:manage']),
    },
    {
      // The mirror of SIEM: SIEM forwards audit events out, journaling takes
      // the tenant's mail in. Both are admin-configured server integrations,
      // and both need system:root — which is why this lives here and not in
      // the compliance console, where a read-only compliance officer would
      // see an entry they cannot open.
      title: t('settings.sidebar.journal', 'SMTP journaling'),
      href: '/settings/journal',
      icon: <Inbox size={18} />,
      visible: (isPro || isEnterprise) && require_any_permission(['system:root']),
    },
    {
      title: t('settings.sidebar.approvals', 'Dual control'),
      href: '/settings/approvals',
      icon: <Users size={18} />,
      visible: isEnterprise && require_any_permission(['system:root', 'user:manage']),
    },
    {
      title: t('settings.sidebar.apiTokens'),
      href: '/settings/api-tokens',
      icon: <KeyRound size={18} />,
    },
    {
      title: t('settings.sidebar.proxy'),
      icon: <Waypoints size={18} />,
      href: '/settings/proxy',
    },
    {
      title: t('settings.sidebar.configurations'),
      icon: <SettingsIcon size={18} />,
      href: '/settings/configurations',
      visible: canGlobal('system:root'),
    },
    {
      title: t('navigation.license'),
      href: '/settings/license',
      icon: <BadgeCheck size={18} />,
      visible: isPro && require_any_permission(['system:root', 'user:manage']),
    },
    {
      title: t('navigation.apiDocs'),
      href: '/settings/api-docs',
      icon: <FileCode2 size={18} />,
    },
  ].filter(item => item.visible !== false)

  return (
    <>
      <FixedHeader />
      <Main>
        <div className='space-y-0.5'>
          <h1 className='text-lg font-bold tracking-tight md:text-3xl'>
            {t('settings.header.title')}
          </h1>
          <p className='text-xs text-muted-foreground'>
            {t('settings.header.description')}
          </p>
        </div>
        <Separator className='my-4 lg:my-6' />
        <div className='flex flex-1 flex-col space-y-2 md:space-y-2 overflow-hidden lg:flex-row lg:space-x-12 lg:space-y-0'>
          <aside className='top-0 lg:sticky lg:w-1/5'>
            <SidebarNav items={sidebarNavItems} />
          </aside>
          <div className='flex w-full p-1 pr-4 overflow-y-hidden'>
            <Outlet />
          </div>
        </div>
      </Main>
    </>
  )
}


