import { afterEach, describe, expect, it, vi } from 'vitest'
import { fireEvent, render } from '@/test/test-utils'
import { BichonMascot } from '../bichon-mascot'

function stubMatchMedia(reduced: boolean) {
    vi.stubGlobal('matchMedia', (query: string) => ({
        matches: reduced && query.includes('prefers-reduced-motion'),
        media: query,
        onchange: null,
        addEventListener: () => { },
        removeEventListener: () => { },
        addListener: () => { },
        removeListener: () => { },
        dispatchEvent: () => false,
    }))
}

function renderMascot() {
    const { container } = render(<BichonMascot />)
    const root = container.firstElementChild as HTMLElement
    vi.spyOn(root, 'getBoundingClientRect').mockReturnValue({
        x: 0,
        y: 0,
        width: 200,
        height: 200,
        top: 0,
        left: 0,
        right: 200,
        bottom: 200,
        toJSON: () => ({}),
    } as DOMRect)
    return container.querySelector('img')!.parentElement as HTMLElement
}

const flushFrame = () => new Promise((resolve) => setTimeout(resolve, 40))

afterEach(() => {
    vi.unstubAllGlobals()
})

describe('BichonMascot', () => {
    it('tilts toward the pointer', async () => {
        stubMatchMedia(false)
        const card = renderMascot()

        fireEvent.pointerMove(window, { clientX: 200, clientY: 100 })
        await flushFrame()

        expect(card.style.transform).toBe('rotateX(0deg) rotateY(12deg)')
    })

    it('stays still under prefers-reduced-motion', async () => {
        stubMatchMedia(true)
        const card = renderMascot()

        fireEvent.pointerMove(window, { clientX: 200, clientY: 100 })
        await flushFrame()

        expect(card.style.transform).toBe('rotateX(0deg) rotateY(0deg)')
    })
})
