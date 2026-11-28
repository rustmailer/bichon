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


import { Row } from '@tanstack/react-table'
import { Button } from '@/components/ui/button'
import { useAccountContext } from '../context'
import { useTranslation } from 'react-i18next'
import { useCurrentUser } from '@/hooks/use-current-user'
import { toast } from '@/hooks/use-toast'
import { ToastAction } from '@/components/ui/toast'
import { AccountModel } from '@/api/account/api'

interface DataTableRowActionsProps {
  row: Row<AccountModel>
}
export function OAuth2Action({ row }: DataTableRowActionsProps) {
  const { t } = useTranslation()
  const { setOpen, setCurrentRow } = useAccountContext()
  const { require_any_permission } = useCurrentUser()
  const mailer = row.original
  const account_type = mailer.account_type;
  const hasPermission = require_any_permission(['system:root', 'account:manage'], row.original.id)

  // Local accounts have no server auth; provider accounts authenticate through
  // their connection (credentials are rotated there, never per account).
  if (account_type === "NoSync" || account_type === "Graph" || account_type === "Gmail") {
    return <Button variant={"ghost"} className="text-xs text-muted-foreground">n/a</Button>
  }

  const isOAuth2 = mailer.imap?.auth.auth_type === "OAuth2"

  if (isOAuth2) {
    return (
      <Button
        variant="ghost"
        size="sm"
        className="text-xs text-blue-500 hover:text-blue-700 underline"
        onClick={() => {
          if (hasPermission) {
            setCurrentRow(mailer)
            setOpen("oauth2")
          } else {
            toast({
              variant: 'destructive',
              title: 'Forbidden',
              description: 'You do not have permission to view oauth2 tokens.',
              action: (
                <ToastAction altText="Close">
                  Close
                </ToastAction>
              ),
            })
          }
        }}
      >
        OAuth2
      </Button>
    )
  }

  return <Button variant={"ghost"} className="text-xs text-muted-foreground">{t('settings.password')}</Button>
}
