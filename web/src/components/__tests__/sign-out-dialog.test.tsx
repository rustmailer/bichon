import { beforeEach, describe, expect, it, vi } from 'vitest'

import { render, screen, waitFor } from '@/test/test-utils'
import userEvent from '@testing-library/user-event'
import i18n from '@/i18n'

const logoutMock = vi.hoisted(() => vi.fn(async () => undefined))
const resetTokenMock = vi.hoisted(() => vi.fn())
const navigateMock = vi.hoisted(() => vi.fn())

vi.mock('@/api/users/api', () => ({ logout: logoutMock }))
vi.mock('@/stores/authStore', () => ({ resetToken: resetTokenMock }))
vi.mock('@tanstack/react-router', () => ({
  useNavigate: () => navigateMock,
  useLocation: () => ({ href: '/somewhere' }),
}))
vi.mock('@/hooks/use-current-user', () => ({
  useCurrentUser: () => ({ user: { sso_provider: null } }),
}))
vi.mock('@/hooks/use-edition', () => ({
  useEdition: () => ({ isPro: false }),
}))

import { SignOutDialog } from '../sign-out-dialog'

describe('sign-out dialog', () => {
  beforeEach(() => {
    logoutMock.mockClear()
    resetTokenMock.mockClear()
    navigateMock.mockClear()
    localStorage.clear()
  })

  it('revokes the session on the server before clearing local state', async () => {
    render(<SignOutDialog open onOpenChange={() => {}} />)

    await userEvent.click(
      screen.getByRole('button', { name: i18n.t('sign_out.confirm') }),
    )

    await waitFor(() => expect(logoutMock).toHaveBeenCalledTimes(1))
    expect(resetTokenMock).toHaveBeenCalledTimes(1)
    expect(logoutMock.mock.invocationCallOrder[0]).toBeLessThan(
      resetTokenMock.mock.invocationCallOrder[0],
    )
  })

  it('starts a fresh session: no redirect back to the old page, no left-over storage', async () => {
    localStorage.setItem('bichon_search_page_size', '50')
    localStorage.setItem('i18nextLng', 'de')

    render(<SignOutDialog open onOpenChange={() => {}} />)

    await userEvent.click(
      screen.getByRole('button', { name: i18n.t('sign_out.confirm') }),
    )

    await waitFor(() =>
      expect(navigateMock).toHaveBeenCalledWith({
        to: '/sign-in',
        replace: true,
      }),
    )
    // The previous account's page is not carried over.
    expect(navigateMock.mock.calls[0][0]).not.toHaveProperty('search')
    expect(localStorage.getItem('bichon_search_page_size')).toBeNull()
    // Browser-level language personalization survives.
    expect(localStorage.getItem('i18nextLng')).toBe('de')
  })
})
