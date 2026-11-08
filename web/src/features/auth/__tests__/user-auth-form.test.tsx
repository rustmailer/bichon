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
        expect(container.querySelectorAll('input[maxlength="1"]')).toHaveLength(6)
    )
    const boxes = Array.from(
        container.querySelectorAll<HTMLInputElement>('input[maxlength="1"]')
    )
    return { boxes, container, user }
}

beforeEach(() => {
    vi.clearAllMocks()
})

describe('UserAuthForm two-factor step', () => {
    // A lone text input kept getting the saved credential dumped into it, even
    // with `autocomplete="one-time-code"`. One box per digit is not a shape
    // password managers fill.
    it('splits the code into six single-digit boxes', async () => {
        const { boxes, container } = await reachTwoFactorStep()

        for (const box of boxes) {
            expect(box).toHaveAttribute('inputmode', 'numeric')
            expect(box).toHaveAttribute('pattern', '[0-9]*')
            expect(box).toHaveAttribute('maxlength', '1')
        }
        expect(boxes[0]).toHaveAttribute('autocomplete', 'one-time-code')
        // Opt-outs for the common password managers; `autocomplete` alone is
        // not honoured by all of them.
        expect(boxes[0]).toHaveAttribute('data-lpignore', 'true')
        expect(boxes[0]).toHaveAttribute('data-1p-ignore', 'true')
        expect(boxes[0]).toHaveAttribute('data-bwignore', 'true')
        // The shadcn-style 3 + 3 grouping.
        expect(container.querySelector('[role="separator"]')).toBeInTheDocument()
    })

    // A manager that still targets the first box dumps the saved credential
    // there. A password is not six digits, so the whole value is discarded.
    it('ignores an autofilled credential', async () => {
        const { boxes } = await reachTwoFactorStep()

        fireEvent.input(boxes[0], { target: { value: 'Tr0ub4dor&3!xY' } })

        expect(boxes.map((box) => box.value).join('')).toBe('')
    })

    // A real six-digit value — pasted, or filled by the OS OTP suggestion —
    // still lands in the boxes.
    it('accepts a full six-digit value', async () => {
        const { boxes } = await reachTwoFactorStep()

        fireEvent.input(boxes[0], { target: { value: '123456' } })

        expect(boxes.map((box) => box.value).join('')).toBe('123456')
    })

    it('advances to the next box while typing', async () => {
        const { boxes, user } = await reachTwoFactorStep()

        await user.type(boxes[0], '9')

        expect(boxes[0].value).toBe('9')
        expect(document.activeElement).toBe(boxes[1])
    })

    // The boxes keep their positions: digits typed after a gap must not slide
    // to the left when the compact code is sent back down.
    it('keeps a digit in the box it was typed into', async () => {
        const { boxes, user } = await reachTwoFactorStep()

        await user.click(boxes[3])
        await user.type(boxes[3], '5')

        expect(boxes[3].value).toBe('5')
        expect(boxes[0].value).toBe('')
    })

    it('does not verify until all six digits are present', async () => {
        const { boxes, user } = await reachTwoFactorStep()
        const { mfaVerify } = await import('@/api/users/api')

        await user.type(boxes[0], '12345')

        expect(mfaVerify).not.toHaveBeenCalled()
    })

    // Filling the sixth box is the whole interaction; making the user click
    // Verify afterwards was pointless.
    it('verifies automatically on the sixth digit', async () => {
        const { boxes, user } = await reachTwoFactorStep()
        const { mfaVerify } = await import('@/api/users/api')

        await user.type(boxes[0], '123456')

        await waitFor(() =>
            expect(mfaVerify).toHaveBeenCalledWith('challenge-1', '123456')
        )
    })
})
