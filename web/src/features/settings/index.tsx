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
import SettingsNav from './components/settings-nav'
import { BadgeCheck, Brush, Building2, FileCode2, Fingerprint, Inbox, KeyRound, LockKeyhole, Palette, Radar, SettingsIcon, ShieldCheck, UserCog, Users, Waypoints } from 'lucide-react'
import { FixedHeader } from '@/components/layout/fixed-header'
import { useCurrentUser } from '@/hooks/use-current-user'
import { useEdition } from '@/hooks/use-edition'
import { useTranslation } from 'react-i18next'
import { ScrollArea } from '@/components/ui/scroll-area'

export default function Settings() {
  const { t } = useTranslation()
  const { canGlobal } = useCurrentUser()
  const { isPro, isEnterprise } = useEdition()


  // Grouped so the growing list stays scannable. Every entry lands in
  // exactly one group; the order inside a group is the order shown.
  const groups = {
    account: t('settings.sidebarGroups.account', 'Account'),
    authentication: t('settings.sidebarGroups.authentication', 'Authentication'),
    integrations: t('settings.sidebarGroups.integrations', 'Integrations'),
    administration: t('settings.sidebarGroups.administration', 'Administration'),
    developer: t('settings.sidebarGroups.developer', 'Developer'),
  }

  const sidebarNavItems = [
    {
      title: t('settings.sidebar.profile'),
      href: '/settings/profile',
      icon: <UserCog size={18} />,
      group: groups.account,
    },
    {
      title: t('settings.sidebar.access'),
      href: '/settings/access',
      icon: <ShieldCheck size={18} />,
      group: groups.account,
    },
    {
      // Per-user security setting: every user manages their own authenticator.
      title: t('settings.sidebar.mfa', 'Two-factor Auth'),
      href: '/settings/mfa',
      icon: <LockKeyhole size={18} />,
      group: groups.account,
    },
    {
      title: t('settings.appearance.title'),
      href: '/settings/appearance',
      icon: <Palette size={18} />,
      group: groups.account,
    },
    {
      title: t('settings.sidebar.apiTokens'),
      href: '/settings/api-tokens',
      icon: <KeyRound size={18} />,
      group: groups.account,
    },
    {
      title: t('settings.sidebar.sso', 'Single Sign-On'),
      href: '/settings/sso',
      icon: <Fingerprint size={18} />,
      group: groups.authentication,
      visible: isPro && (canGlobal('system:root') || canGlobal('user:manage')),
    },
    {
      title: t('settings.sidebar.ldap', 'LDAP'),
      href: '/settings/ldap',
      icon: <Building2 size={18} />,
      group: groups.authentication,
      visible: isPro && (canGlobal('system:root') || canGlobal('user:manage')),
    },
    {
      title: t('settings.sidebar.siem', 'SIEM'),
      href: '/settings/siem',
      icon: <Radar size={18} />,
      group: groups.integrations,
      visible: isEnterprise && (canGlobal('system:root') || canGlobal('user:manage')),
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
      group: groups.integrations,
      visible: isEnterprise && canGlobal('system:root'),
    },
    {
      title: t('settings.sidebar.proxy'),
      icon: <Waypoints size={18} />,
      href: '/settings/proxy',
      group: groups.integrations,
    },
    {
      title: t('settings.sidebar.branding', 'Branding'),
      href: '/settings/branding',
      icon: <Brush size={18} />,
      group: groups.administration,
      visible: isPro && (canGlobal('system:root') || canGlobal('user:manage')),
    },
    {
      title: t('settings.sidebar.approvals', 'Dual control'),
      href: '/settings/approvals',
      icon: <Users size={18} />,
      group: groups.administration,
      visible: isEnterprise && (canGlobal('system:root') || canGlobal('user:manage')),
    },
    {
      title: t('settings.sidebar.configurations'),
      icon: <SettingsIcon size={18} />,
      href: '/settings/configurations',
      group: groups.administration,
      visible: canGlobal('system:root'),
    },
    {
      title: t('navigation.license'),
      href: '/settings/license',
      icon: <BadgeCheck size={18} />,
      group: groups.administration,
      visible: isPro && (canGlobal('system:root') || canGlobal('user:manage')),
    },
    {
      title: t('navigation.apiDocs'),
      href: '/settings/api-docs',
      icon: <FileCode2 size={18} />,
      group: groups.developer,
    },
  ].filter(item => item.visible !== false)

  return (
    <>
      <FixedHeader />
      {/* A fixed-height shell: the settings nav stays put and only the panel
          scrolls, with the app's ScrollArea bar instead of the browser's.
          Without `fixed`, `overflow-hidden` below had nothing to constrain and
          the whole page scrolled. */}
      <Main fixed>
        <div className='mx-auto flex min-h-0 w-full max-w-5xl flex-1 flex-col overflow-hidden'>
          {/* Group tabs on top, the active group's entries below them; the
              panel keeps the remaining height and scrolls on its own. */}
          <SettingsNav items={sidebarNavItems} />
          <div className='flex min-h-0 w-full flex-1 px-4 pb-2 pt-4'>
            {/* The viewport's table wrapper is height:auto, so a panel that
                asks for h-full would instead grow with its content. Give the
                wrapper the viewport height; panels that are taller than the
                panel still scroll the outer area. */}
            <ScrollArea
              className='h-full w-full'
              viewportClassName='[&>div]:!block [&>div]:h-full'
            >
              <Outlet />
            </ScrollArea>
          </div>
        </div>
      </Main>
    </>
  )
}


