//
// Copyright (c) 2025-2026 rustmailer.com (https://rustmailer.com)
//
// This file is part of the Bichon Email Archiving Project
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as
// published by the Free Software Foundation, either version 3 of the
// License, or (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <http://www.gnu.org/licenses/>.
//
// Status badges shared by the connection list and the account table. The
// server owns the vocabulary; an unknown value renders as-is so a newer server
// degrades to text instead of an empty badge (R-RS-10).
import { useTranslation } from 'react-i18next'
import { Badge } from '@/components/ui/badge'

export type BadgeVariant = 'default' | 'secondary' | 'destructive' | 'outline'

export function statusVariant(status: string): BadgeVariant {
  switch (status) {
    case 'ok':
    case 'idle':
      return 'secondary'
    case 'syncing':
    case 'gap_fill':
      return 'default'
    case 'paused':
      return 'outline'
    case 'throttled':
    case 'interrupted':
    case 'untested':
      return 'outline'
    default:
      // auth_required / needs_reauth / misconfigured / error / license_blocked
      // / disabled all need the operator's attention.
      return 'destructive'
  }
}

export function StatusBadge({ status }: { status: string }) {
  const { t } = useTranslation()
  return (
    <Badge variant={statusVariant(status)}>
      {t(`providers.status.${status}`, status)}
    </Badge>
  )
}

/** Epoch millis rendered as a short local date-time; `null` becomes the
 *  translated "never". */
export function useFormatTime() {
  const { t } = useTranslation()
  return (value?: number | null): string => {
    if (!value) return t('providers.never', 'Never')
    const d = new Date(value)
    const pad = (n: number) => String(n).padStart(2, '0')
    return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(
      d.getHours()
    )}:${pad(d.getMinutes())}`
  }
}
