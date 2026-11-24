import { describe, expect, it } from 'vitest'

import { buildCsp, withCsp } from '@/lib/mail-csp'

describe('mail body CSP', () => {
  it('blocks remote content by default', () => {
    const csp = buildCsp(false)
    expect(csp).toContain("default-src 'none'")
    expect(csp).toContain('img-src data: blob:;')
    expect(csp).not.toContain('http:')
    expect(csp).toContain("form-action 'none'")
  })

  it('allows remote images only after the user opts in', () => {
    const csp = buildCsp(true)
    expect(csp).toContain('img-src data: blob: http: https:')
    expect(csp).toContain('media-src data: blob: http: https:')
    // Scripts stay blocked in both modes.
    expect(csp).toContain('default-src \'none\'')
    expect(csp).not.toContain('script-src')
  })

  it('inserts the policy after the head when the email has one', () => {
    const out = withCsp('<html><head><title>x</title></head><body>hi</body></html>', false)
    expect(out.indexOf('Content-Security-Policy')).toBeLessThan(out.indexOf('<title>'))
  })

  it('creates a head when the email has html but no head', () => {
    const out = withCsp('<html><body><img src="https://tracker/x"></body></html>', false)
    expect(out).toMatch(/<html><head><meta http-equiv="Content-Security-Policy"/)
    expect(out).toContain('<body>')
  })

  it('keeps the doctype first and adds the policy after it', () => {
    const out = withCsp('<!doctype html><html><body>x</body></html>', true)
    expect(out.startsWith('<!doctype html>')).toBe(true)
    expect(out.indexOf('Content-Security-Policy')).toBeGreaterThan(0)
  })

  it('prepends the policy for a bare fragment', () => {
    const out = withCsp('<p>hello</p>', false)
    expect(out.startsWith('<meta http-equiv="Content-Security-Policy"')).toBe(true)
  })
})
