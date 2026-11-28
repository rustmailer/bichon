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


import { useNavigate } from '@tanstack/react-router'
import { ConfirmDialog } from '@/components/confirm-dialog'
import { useQueryClient } from '@tanstack/react-query'
import { clearSessionState } from '@/lib/session'
import { logout as revokeSessionOnServer } from '@/api/users/api'
import { useTranslation } from 'react-i18next'
import { useCurrentUser } from '@/hooks/use-current-user'
import { useEdition } from '@/hooks/use-edition'
import { useState } from 'react'

interface SignOutDialogProps {
  open: boolean
  onOpenChange: (open: boolean) => void
}

export function SignOutDialog({ open, onOpenChange }: SignOutDialogProps) {
  const navigate = useNavigate()
  const queryClient = useQueryClient()
  const { t } = useTranslation()
  const { user } = useCurrentUser()
  const { isPro } = useEdition()
  const [isLoading, setIsLoading] = useState(false)

  const isSsoUser =
    isPro && !!user?.sso_provider && user.sso_provider !== ''

  const localSignOut = () => {
    // A deliberate sign-out starts a fresh session: no redirect back to the
    // page the previous account was reading, and no cached data either.
    clearSessionState(queryClient)
    navigate({ to: '/sign-in', replace: true })
  }

  // Deleting the server-side record is best-effort: if the request fails
  // (offline, server restarting), the user must still be able to leave the
  // page. The token keeps its normal idle expiry in that case.
  const revokeServerSession = async () => {
    try {
      await revokeSessionOnServer()
    } catch {
      // ignored on purpose
    }
  }

  const handleConfirm = async () => {
    setIsLoading(true)
    await revokeServerSession()
    if (!isSsoUser) {
      setIsLoading(false)
      localSignOut()
      return
    }
    // Only sign out of bichon; keep the SSO session for one-click sign-in.
    fetch('/api/auth/oidc/local-logout', { redirect: 'follow' })
      .catch(() => {})
      .finally(() => {
        setIsLoading(false)
        localSignOut()
      })
  }

  return (
    <ConfirmDialog
      open={open}
      onOpenChange={onOpenChange}
      title={t('sign_out.title', 'Sign out')}
      desc={
        isSsoUser
          ? t(
              'sign_out.sso_desc',
              'Signing out of Bichon only keeps your SSO session (e.g. Keycloak) active. For full security, choose "Sign out and end SSO session".'
            )
          : t(
              'sign_out.desc',
              'Are you sure you want to sign out? You will need to sign in again to access your account.'
            )
      }
      confirmText={
        isSsoUser
          ? t('sign_out.confirm_sso', 'Sign out of Bichon only')
          : t('sign_out.confirm', 'Sign out')
      }
      destructive
      isLoading={isLoading}
      handleConfirm={handleConfirm}
      className="sm:max-w-sm"
    >
      {isSsoUser && (
        <div className='grid gap-2'>
          <button
            type='button'
            className='inline-flex h-10 items-center justify-center gap-2 rounded-md border border-destructive/50 bg-background px-4 text-sm font-medium text-destructive transition-colors hover:bg-destructive/10'
            disabled={isLoading}
            onClick={async () => {
              setIsLoading(true)
              // Keep the bichon_webui_token cookie until the redirect: the
              // OIDC logout handler reads it for the id_token_hint.
              await revokeServerSession()
              clearSessionState(queryClient)
              window.location.href = '/api/auth/oidc/logout'
            }}
          >
            {t('sign_out.full_sign_out', 'Sign out and end SSO session')}
          </button>
          <p className='text-muted-foreground px-2 text-xs'>
            {t(
              'sign_out.sso_warning',
              'This will end your SSO session (e.g. Keycloak) and sign you out of all applications using it.'
            )}
          </p>
        </div>
      )}
    </ConfirmDialog>
  )
}
