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
import { IconLayoutDashboard, IconSettings } from '@tabler/icons-react'
import {
  Archive,
  BarChart3,
  Cloud,
  Download,
  FileCheck2,
  Fingerprint,
  FolderOpen,
  Gauge,
  IdCard,
  Inbox,
  Lock,
  Paperclip,
  Search,
  Settings2,
  ShieldCheck,
  Upload,
  Users2,
  ScrollText,
} from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { useCurrentUser } from '@/hooks/use-current-user'
import { useEdition } from '@/hooks/use-edition'
import { type SidebarData } from '../types'

export function useSidebarData(): SidebarData {
  const { t } = useTranslation()

  const { require_any_permission } = useCurrentUser()
  const { isPro, isEnterprise } = useEdition()

  return {
    navGroups: [
      {
        id: 'general',
        title: t('navigation.general'),
        defaultOpen: true,
        items: [
          {
            title: t('navigation.dashboard'),
            url: '/',
            icon: IconLayoutDashboard,
          },
        ],
      },
      {
        id: 'accounts',
        title: t('navigation.accounts'),
        defaultOpen: true,
        items: [
          {
            title: t('navigation.accounts'),
            url: '/accounts',
            icon: Inbox,
          },
          {
            title: t('navigation.mailbox'),
            url: '/mailbox',
            icon: FolderOpen,
          },
          {
            title: t('common.search'),
            url: '/search',
            icon: Search,
          },
          {
            title: t('navigation.attachment'),
            url: '/attachment',
            icon: Paperclip,
          },
          {
            title: t('navigation.analytics', 'Analytics'),
            url: '/analytics',
            icon: BarChart3,
            visible:
              isPro &&
              require_any_permission([
                'system:root',
                'user:manage',
                'data:read:all',
                // Account-scoped managers / readers can analyze the accounts
                // they can read - global "manage account all" is not required.
                'data:read',
                'account:manage',
              ]),
          },
          {
            title: t('import.title', 'Import'),
            url: '/import',
            icon: Upload,
            visible: require_any_permission(['data:import:batch']),
          },
          {
            title: t('export_tasks.title', 'Export Tasks'),
            url: '/exports',
            icon: Download,
            visible: require_any_permission([
              'data:export:batch',
              'data:export:batch:all',
            ]),
          },
        ],
      },
      {
        id: 'auth',
        title: t('navigation.auth'),
        defaultOpen: true,
        items: [
          {
            title: t('navigation.oauth2'),
            url: '/oauth2',
            icon: IdCard,
          },
          {
            // Provider connections are tenant-level credentials (client
            // secrets / service-account keys), so they belong with the other
            // auth configuration rather than under day-to-day accounts. Only
            // an administrator can create, rotate or delete them.
            title: t('navigation.providers', 'Cloud providers'),
            url: '/providers',
            icon: Cloud,
            visible:
              isPro &&
              require_any_permission(['system:root', 'user:manage']),
          },
        ],
      },
      {
        id: 'users',
        title: t('navigation.users'),
        defaultOpen: true,
        items: [
          {
            title: t('navigation.users'),
            url: '/users',
            icon: Users2,
            visible: require_any_permission(['system:root', 'user:manage']),
          },
        ],
      },
      {
        id: 'compliance',
        title: t('navigation.compliance'),
        items: [
          {
            // First in the group on purpose: it is the one entry that answers
            // the question the whole group exists for, and it links out to
            // every entry below it rather than duplicating them.
            title: t('navigation.complianceDashboard', 'Compliance dashboard'),
            url: '/compliance-dashboard',
            icon: Gauge,
            visible:
              isEnterprise &&
              require_any_permission([
                'system:root',
                'user:manage',
                // The read-only separation-of-duties view. A compliance
                // officer holds only this, and this page is written for them.
                'compliance:audit',
              ]),
          },
          {
            title: t('navigation.integrity', 'Integrity'),
            url: '/integrity',
            icon: ShieldCheck,
            // The backend serves integrity checks from the Pro edition up
            // (bichon-pro integrity.rs) and the page itself gates on isPro;
            // isEnterprise here hid the entry for plain-Pro installs.
            visible:
              isPro &&
              require_any_permission([
                'system:root',
                'account:manage:all',
                'account:manage',
                // Read-only separation-of-duties view (compliance officers).
                'compliance:audit',
              ]),
          },
          {
            title: t('compliance_export.title', 'Compliance Export'),
            url: '/compliance-export',
            icon: FileCheck2,
            visible:
              isEnterprise &&
              require_any_permission([
                'data:export:batch',
                'data:export:batch:all',
              ]),
          },
          {
            title: t('navigation.auditLog'),
            url: '/audit-log',
            icon: ScrollText,
            visible:
              isEnterprise &&
              require_any_permission([
                'system:root',
                'user:manage',
                'data:read:all',
                'compliance:audit',
              ]),
          },
          {
            title: t('navigation.legalHold', 'Legal Hold'),
            url: '/legal-hold',
            icon: Lock,
            visible:
              isEnterprise &&
              require_any_permission([
                'legal:hold',
                'system:root',
                'compliance:audit',
              ]),
          },
          {
            title: t('navigation.approvals', 'Approvals'),
            url: '/approvals',
            icon: ShieldCheck,
            visible:
              isEnterprise &&
              require_any_permission([
                // The queue is the second half of the two-person rule. A
                // compliance officer without `approval:decide` may still read
                // it — that is the separation-of-duties view, not a way to
                // move it.
                'approval:decide',
                'compliance:audit',
                'system:root',
              ]),
          },
          {
            title: t('navigation.timestampAnchor', 'Timestamp anchoring'),
            url: '/timestamp-anchor',
            icon: Fingerprint,
            visible:
              isEnterprise &&
              require_any_permission([
                'timestamp:manage',
                'system:root',
                'compliance:audit',
              ]),
          },
          {
            // The compliance REST endpoints live on the Pro server only
            // (see rest/compliance_config.rs there), so the community
            // edition — which always reports edition=community — never
            // passes this gate and never shows the entry.
            title: t(
              'navigation.complianceSettings',
              'Configuration',
            ),
            url: '/configuration',
            icon: Settings2,
            visible:
              (isPro || isEnterprise) &&
              require_any_permission(['system:root']),
          },
        ],
      },
      {
        id: 'other',
        title: t('navigation.other'),
        defaultOpen: true,
        items: [
          {
            title: t('navigation.backup', 'Backup'),
            url: '/backup',
            icon: Archive,
            visible: require_any_permission(['backup:manage', 'system:root']),
          },
          {
            title: t('navigation.settings'),
            url: '/settings',
            icon: IconSettings,
          },
        ],
      },
    ],
  }
}
