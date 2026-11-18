import { beforeEach, describe, expect, it, vi } from 'vitest'
import { fireEvent, render, waitFor } from '@/test/test-utils'
import userEvent from '@testing-library/user-event'
import { UserAuthForm } from '../user-auth-form'

vi.mock('@tanstack/react-router', () => ({
    useNavigate: () => () => { },
    useLocation: () => ({ search: {} }),
}))

vi.mock('@/hooks/use-edition', () => ({
    useEdition: () => ({ isPro: false, ssoEnabled: false, ldapEnabled: false }),
}))

vi.mock('@/context/theme-context', () => ({
    useTheme: () => ({ setTheme: () => { } }),
}))

// The server asks for a TOTP code before issuing a token.
vi.mock('@/api/users/api', () => ({
    login: vi.fn(async () => ({
        success: true,
        mfa_required: true,
        mfa_challenge: 'challenge-1',
    })),
    ldapLogin: vi.fn(),
    mfaVerify: vi.fn(async () => ({
        success: false,
        error_message: 'invalid-code',
    })),
}))

async function reachTwoFactorStep() {
    const user = userEvent.setup()
    const { container } = render(<UserAuthForm />)

    await user.type(container.querySelector('input[autocomplete="username"]')!, 'admin')
    await user.type(container.querySelector('input[autocomplete="current-password"]')!, 'secret')
    fireEvent.submit(container.querySelector('form')!)

    await waitFor(() =>
        expect(container.querySelector('input[name^="otp-"]')).toBeInTheDocument()
    )
    const codeInput = container.querySelector<HTMLInputElement>('input[name^="otp-"]')!
    const boxes = Array.from(
        container.querySelectorAll<HTMLElement>('[data-otp-digit]')
    )
    return { boxes, codeInput, container, user }
}

const shown = (boxes: HTMLElement[]) =>
    boxes.map((box) => box.textContent).join('')

beforeEach(() => {
    vi.clearAllMocks()
})

describe('UserAuthForm two-factor step', () => {
    // Managers fill form fields on load, and an OTP field is recognisable by
    // its attributes. None of those attributes are advertised here, and the
    // field is read-only until a real interaction.
    it('does not advertise itself as a verification-code field', async () => {
        const { boxes, codeInput, container } = await reachTwoFactorStep()

        expect(boxes).toHaveLength(6)
        expect(codeInput).not.toHaveAttribute('inputmode')
        expect(codeInput).not.toHaveAttribute('pattern')
        expect(codeInput).not.toHaveAttribute('maxlength')
        expect(codeInput).toHaveAttribute('autocomplete', 'off')
        expect(codeInput).toHaveAttribute('readonly')
        // Randomised name: managers key on names like "code"/"otp".
        expect(codeInput.getAttribute('name')).toMatch(/^otp-/)
        // Opt-outs for the common managers.
        expect(codeInput).toHaveAttribute('data-lpignore', 'true')
        expect(codeInput).toHaveAttribute('data-1p-ignore', 'true')
        expect(codeInput).toHaveAttribute('data-bwignore', 'true')
        // The shadcn-style 3 + 3 grouping.
        expect(container.querySelector('[role="separator"]')).toBeInTheDocument()
    })

    // With the sign-in fields gone, a lone input inside the login form reads
    // as the tail of that form; the TOTP step is rendered outside one.
    it('is not inside a form', async () => {
        const { codeInput } = await reachTwoFactorStep()

        expect(codeInput.closest('form')).toBeNull()
    })

    it('unlocks only after the user touches the field', async () => {
        const { codeInput } = await reachTwoFactorStep()

        expect(codeInput).toHaveAttribute('readonly')

        fireEvent.pointerDown(codeInput)

        await waitFor(() => expect(codeInput).not.toHaveAttribute('readonly'))
    })

    // A fill that still targets the field before the user touches it is
    // reverted instead of being shown as if the user had typed it.
    it('ignores a fill delivered before the user touches the field', async () => {
        const { boxes, codeInput } = await reachTwoFactorStep()

        fireEvent.input(codeInput, { target: { value: '123456' } })

        expect(codeInput.value).toBe('')
        expect(shown(boxes)).toBe('')
    })

    it('shows typed digits in the boxes', async () => {
        const { boxes, codeInput, user } = await reachTwoFactorStep()

        await user.type(codeInput, '123')

        expect(shown(boxes)).toBe('123')
    })

    it('drops non-digit characters', async () => {
        const { boxes, codeInput, user } = await reachTwoFactorStep()

        await user.type(codeInput, '1a2b3')

        expect(codeInput.value).toBe('123')
        expect(shown(boxes)).toBe('123')
    })

    it('accepts a pasted code', async () => {
        const { boxes, codeInput } = await reachTwoFactorStep()

        fireEvent.paste(codeInput, {
            clipboardData: { getData: () => '123456' },
        })

        expect(shown(boxes)).toBe('123456')
    })

    it('removes the last digit on backspace', async () => {
        const { boxes, codeInput, user } = await reachTwoFactorStep()

        await user.type(codeInput, '12')
        await user.keyboard('{Backspace}')

        expect(shown(boxes)).toBe('1')
    })

    it('does not verify until all six digits are present', async () => {
        const { codeInput, user } = await reachTwoFactorStep()
        const { mfaVerify } = await import('@/api/users/api')

        await user.type(codeInput, '12345')

        expect(mfaVerify).not.toHaveBeenCalled()
    })

    // Filling the sixth digit is the whole interaction; making the user click
    // Verify afterwards was pointless.
    it('verifies automatically on the sixth digit', async () => {
        const { codeInput, user } = await reachTwoFactorStep()
        const { mfaVerify } = await import('@/api/users/api')

        await user.type(codeInput, '123456')

        await waitFor(() =>
            expect(mfaVerify).toHaveBeenCalledWith('challenge-1', '123456')
        )
    })
})
