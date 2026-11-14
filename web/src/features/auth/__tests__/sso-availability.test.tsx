import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render, screen } from '@/test/test-utils'
import i18n from '@/i18n'
import { UserAuthForm } from '../user-auth-form'

// The sign-in page warns when the identity provider is unreachable instead
// of offering a button that ends on a raw error page.

const ssoStatus = vi.hoisted(() => ({
  available: false,
}))

vi.mock('@tanstack/react-router', () => ({
  useNavigate: () => () => {},
  useLocation: () => ({ search: {} }),
}))

vi.mock('@/hooks/use-edition', () => ({
  useEdition: () => ({ isPro: true, ssoEnabled: true, ldapEnabled: false }),
}))

vi.mock('@/context/theme-context', () => ({
  useTheme: () => ({ setTheme: () => {} }),
}))

vi.mock('@/api/users/api', () => ({
  login: vi.fn(),
  ldapLogin: vi.fn(),
  mfaVerify: vi.fn(),
}))

vi.mock('@/api/sso/api', () => ({
  get_sso_status: vi.fn(async () => ({
    configured: true,
    available: ssoStatus.available,
    code: ssoStatus.available ? null : 'discovery_timeout',
    checked_at: 1,
  })),
}))

beforeEach(() => {
  ssoStatus.available = false
})

describe('sign-in page SSO availability', () => {
  it('warns and points at the local account when the provider is down', async () => {
    render(<UserAuthForm />)

    expect(
      await screen.findByText(i18n.t('auth.ssoUnavailable'))
    ).toBeInTheDocument()
    // The local form is still usable — the notice is not a dead end.
    expect(screen.getByText(i18n.t('auth.login'))).toBeInTheDocument()
  })

  it('keeps the normal button when the provider answers', async () => {
    ssoStatus.available = true
    render(<UserAuthForm />)

    expect(
      await screen.findByText(i18n.t('auth.ssoLogin'))
    ).toBeInTheDocument()
    expect(screen.queryByText(i18n.t('auth.ssoUnavailable'))).not.toBeInTheDocument()
  })
})
