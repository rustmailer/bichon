import { beforeEach, describe, expect, it, vi } from 'vitest'

import { render, screen, waitFor } from '@/test/test-utils'
import userEvent from '@testing-library/user-event'
import i18n from '@/i18n'

const logoutMock = vi.hoisted(() => vi.fn(async () => undefined))
const resetTokenMock = vi.hoisted(() => vi.fn())

vi.mock('@/api/users/api', () => ({ logout: logoutMock }))
vi.mock('@/stores/authStore', () => ({ resetToken: resetTokenMock }))
vi.mock('@tanstack/react-router', () => ({
  useNavigate: () => vi.fn(),
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
})
