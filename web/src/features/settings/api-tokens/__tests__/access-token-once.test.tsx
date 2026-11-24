import { beforeEach, describe, expect, it, vi } from 'vitest'

import { render, screen } from '@/test/test-utils'
import userEvent from '@testing-library/user-event'
import i18n from '@/i18n'

const createAccessToken = vi.hoisted(() => vi.fn())

vi.mock('@/api/users/api', () => ({
  create_access_token: createAccessToken,
  update_access_token: vi.fn(),
}))

import { TokensActionDialog } from '../access-token-action'

describe('access token creation dialog', () => {
  beforeEach(() => {
    createAccessToken.mockReset()
  })

  it('warns before creation and shows the token once afterwards', async () => {
    createAccessToken.mockResolvedValue('fresh-token-1234567890')

    render(
      <TokensActionDialog open userId={1} onOpenChange={() => {}} />,
    )

    // The warning is visible before the user commits to creating a token.
    expect(
      screen.getAllByText(i18n.t('apiTokens.created.warning')).length,
    ).toBeGreaterThan(0)

    await userEvent.click(
      screen.getByRole('button', { name: i18n.t('apiTokens.dialog.save') }),
    )

    // The plaintext is rendered in the one-time dialog...
    expect(
      await screen.findByText('fresh-token-1234567890'),
    ).toBeInTheDocument()
    // ...and the dialog still carries the "shown only once" warning.
    expect(
      screen.getAllByText(i18n.t('apiTokens.created.warning')).length,
    ).toBeGreaterThan(1)
  })
})
