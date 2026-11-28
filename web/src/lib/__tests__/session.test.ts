import { beforeEach, describe, expect, it } from 'vitest'
import { QueryClient } from '@tanstack/react-query'
import { getToken, setToken } from '@/stores/authStore'
import { clearSessionState, clearSessionStorage } from '../session'

describe('clearSessionState', () => {
  beforeEach(() => {
    localStorage.clear()
  })

  it('drops the token, cached queries and user-scoped storage', () => {
    const queryClient = new QueryClient()
    queryClient.setQueryData(['current-user'], { username: 'admin' })
    queryClient.setQueryData(['role-list'], [{ id: 1 }])
    setToken({ success: true, access_token: 'abc' })
    localStorage.setItem('bichon_search_page_size', '50')
    localStorage.setItem('bichon_search_history', '["secret query"]')
    localStorage.setItem('mailbox_account', '7')

    clearSessionState(queryClient)

    expect(getToken()).toBeNull()
    expect(queryClient.getQueryData(['current-user'])).toBeUndefined()
    expect(queryClient.getQueryData(['role-list'])).toBeUndefined()
    expect(localStorage.getItem('bichon_search_page_size')).toBeNull()
    expect(localStorage.getItem('bichon_search_history')).toBeNull()
    expect(localStorage.getItem('mailbox_account')).toBeNull()
  })

  it('keeps browser-level language and theme personalization', () => {
    localStorage.setItem('i18nextLng', 'de')
    localStorage.setItem('vite-ui-theme', 'dark')
    localStorage.setItem('vite-ui-theme-follow-system', 'false')

    clearSessionStorage()

    expect(localStorage.getItem('i18nextLng')).toBe('de')
    expect(localStorage.getItem('vite-ui-theme')).toBe('dark')
    expect(localStorage.getItem('vite-ui-theme-follow-system')).toBe('false')
  })
})
