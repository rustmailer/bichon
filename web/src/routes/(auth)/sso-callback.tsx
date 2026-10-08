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

import { useEffect } from 'react'
import { createFileRoute, useNavigate } from '@tanstack/react-router'
import { setToken } from '@/stores/authStore'
import axiosInstance from '@/api/axiosInstance'
import { Loader2 } from 'lucide-react'

interface HandoffResponse {
  access_token: string
  redirect_to: string
}

function SsoCallback() {
  const navigate = useNavigate()

  useEffect(() => {
    const params = new URLSearchParams(window.location.search)
    const handoffId = params.get('handoff')

    // Remove the handoff id from the browser URL immediately so it does not
    // linger in history, referer headers or window.location.
    window.history.replaceState({}, '', window.location.pathname)

    if (!handoffId) {
      navigate({
        to: '/sign-in',
        search: { sso_error: 'Missing SSO handoff id' } as any,
      })
      return
    }

    let cancelled = false
    axiosInstance
      .post<HandoffResponse>('api/auth/oidc/handoff', { handoff: handoffId })
      .then((res) => {
        if (cancelled) return
        const { access_token, redirect_to } = res.data
        if (!access_token) {
          navigate({
            to: '/sign-in',
            search: { sso_error: 'Empty handoff response from server' } as any,
          })
          return
        }
        setToken({
          success: true,
          access_token,
          theme: null,
          language: null,
          error_message: null,
        } as any)
        const target = redirect_to && redirect_to.startsWith('/') ? redirect_to : '/'
        navigate({ to: target })
      })
      .catch((err) => {
        if (cancelled) return
        const msg =
          err?.response?.data && typeof err.response.data === 'string'
            ? err.response.data
            : 'Failed to complete SSO login'
        navigate({
          to: '/sign-in',
          search: { sso_error: msg } as any,
        })
      })

    return () => {
      cancelled = true
    }
  }, [navigate])

  return (
    <div className='flex min-h-screen items-center justify-center'>
      <div className='flex flex-col items-center gap-2'>
        <Loader2 className='h-8 w-8 animate-spin' />
        <p className='text-sm text-muted-foreground'>Signing you in…</p>
      </div>
    </div>
  )
}

export const Route = createFileRoute('/(auth)/sso-callback')({
  component: SsoCallback,
})
