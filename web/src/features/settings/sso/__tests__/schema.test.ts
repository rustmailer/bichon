import { describe, expect, it } from 'vitest'
import { normalizeDomain, parseDomains, ssoSchema } from '../schema'

const t = (key: string) => key

const base = {
  enabled: false,
  issuer_url: '',
  client_id: '',
  client_secret: '',
  redirect_uri: '',
  allowed_email_domains: '',
}

const enabled = {
  ...base,
  enabled: true,
  issuer_url: 'https://keycloak.example.com/realms/demo',
  client_id: 'bichon',
  client_secret: 'secret',
  redirect_uri: 'https://bichon.example.com/api/auth/oidc/callback',
}

describe('SSO form schema', () => {
  it('accepts an empty disabled form', () => {
    expect(ssoSchema(t, () => false).safeParse(base).success).toBe(true)
  })

  it('requires the four fields only when enabled', () => {
    const result = ssoSchema(t, () => false).safeParse({
      ...base,
      enabled: true,
    })
    expect(result.success).toBe(false)
    if (!result.success) {
      const paths = result.error.issues.map((i) => i.path[0])
      expect(paths).toEqual(
        expect.arrayContaining([
          'issuer_url',
          'client_id',
          'client_secret',
          'redirect_uri',
        ]),
      )
    }
  })

  it('accepts an enabled form with a stored secret', () => {
    const result = ssoSchema(t, () => true).safeParse({
      ...enabled,
      client_secret: '',
    })
    expect(result.success).toBe(true)
  })

  it('requires a secret when none is stored and the field is empty', () => {
    const result = ssoSchema(t, () => false).safeParse({
      ...enabled,
      client_secret: '',
    })
    expect(result.success).toBe(false)
  })

  it('rejects non-http issuer and redirect URLs', () => {
    const result = ssoSchema(t, () => true).safeParse({
      ...enabled,
      issuer_url: 'keycloak.example.com',
      redirect_uri: 'ftp://bichon.example.com/cb',
    })
    expect(result.success).toBe(false)
    if (!result.success) {
      const paths = result.error.issues.map((i) => i.path[0])
      expect(paths).toContain('issuer_url')
      expect(paths).toContain('redirect_uri')
    }
  })

  it('rejects an invalid domain and accepts a normalized one', () => {
    const bad = ssoSchema(t, () => true).safeParse({
      ...enabled,
      allowed_email_domains: 'example.com, bad domain',
    })
    expect(bad.success).toBe(false)

    const good = ssoSchema(t, () => true).safeParse({
      ...enabled,
      allowed_email_domains: '@Example.com\nsub.example.com',
    })
    expect(good.success).toBe(true)
  })

  it('normalizes and splits domains the same way the server does', () => {
    expect(parseDomains(' @Example.com. , sub.example.com\n')).toEqual([
      '@Example.com.',
      'sub.example.com',
    ])
    expect(normalizeDomain(' @Example.com. ')).toBe('example.com')
    expect(normalizeDomain('a@b')).toBeNull()
    expect(normalizeDomain('bad domain')).toBeNull()
  })
})
