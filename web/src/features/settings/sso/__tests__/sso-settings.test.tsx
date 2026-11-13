import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render, screen, waitFor } from '@/test/test-utils'
import userEvent from '@testing-library/user-event'
import { SsoSettings } from '../index'
import {
  get_sso_config,
  update_sso_config,
  type SsoConfigView,
} from '@/api/sso/api'

vi.mock('@/api/sso/api', () => ({
  get_sso_config: vi.fn(),
  update_sso_config: vi.fn(),
  test_sso_config: vi.fn(),
}))

const state = vi.hoisted(() => ({ allowed: true }))

vi.mock('@/hooks/use-edition', () => ({
  useEdition: () => ({ isPro: true, isLoading: false }),
}))

vi.mock('@/hooks/use-current-user', () => ({
  useCurrentUser: () => ({
    require_any_permission: () => state.allowed,
    isLoading: false,
  }),
}))

// The real Navigate needs a router; the test only cares that the page
// redirects instead of rendering the configuration.
vi.mock('@tanstack/react-router', () => ({
  Navigate: () => <div data-testid='redirect' />,
}))

const emptyConfig: SsoConfigView = {
  enabled: false,
  issuer_url: null,
  client_id: null,
  client_secret_set: false,
  redirect_uri: null,
  allowed_email_domains: [],
  configured: false,
  active: false,
  updated_at: 0,
}

describe('SsoSettings', () => {
  beforeEach(() => {
    state.allowed = true
    vi.mocked(get_sso_config).mockReset()
    vi.mocked(update_sso_config).mockReset()
    vi.mocked(get_sso_config).mockResolvedValue(emptyConfig)
  })

  it('loads the configuration once and does not refetch on form resets', async () => {
    render(<SsoSettings />)

    await screen.findByText('Single Sign-On (OIDC)')
    // Let any post-reset render/effect settle; a `[form]` dependency here
    // produced an endless GET loop in a real browser.
    await waitFor(() => expect(get_sso_config).toHaveBeenCalledTimes(1))
    await new Promise((resolve) => setTimeout(resolve, 50))
    expect(get_sso_config).toHaveBeenCalledTimes(1)
  })

  it('redirects users without the admin permission and never loads the config', async () => {
    state.allowed = false
    render(<SsoSettings />)

    await screen.findByTestId('redirect')
    expect(get_sso_config).not.toHaveBeenCalled()
  })

  it('blocks enabling SSO with empty fields and shows messages under the form', async () => {
    const user = userEvent.setup()
    render(<SsoSettings />)
    await screen.findByText('Single Sign-On (OIDC)')

    await user.click(screen.getByRole('switch'))
    await user.click(
      screen.getByRole('button', { name: /save configuration/i }),
    )

    await waitFor(() =>
      expect(screen.getAllByText('This field is required').length).toBeGreaterThan(
        0,
      ),
    )
    expect(update_sso_config).not.toHaveBeenCalled()
  })
})
