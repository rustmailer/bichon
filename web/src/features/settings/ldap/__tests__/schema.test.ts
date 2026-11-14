import { describe, expect, it } from 'vitest'
import { isLdapUrl, ldapSchema } from '../schema'

const t = (key: string) => key

const base = {
  enabled: false,
  url: '',
  base_dn: '',
  bind_dn: '',
  bind_password: '',
  user_filter: '(&(objectClass=person)(sAMAccountName={username}))',
  scope: 'subtree' as const,
  username_attribute: 'sAMAccountName',
  sso_id_attribute: 'distinguishedName',
  starttls: false,
  no_tls_verify: false,
  timeout_secs: '10',
}

const enabled = {
  ...base,
  enabled: true,
  url: 'ldap://directory.example.com:389',
  base_dn: 'dc=example,dc=com',
}

const schema = () => ldapSchema(t, () => false)

describe('LDAP form schema', () => {
  it('accepts an empty disabled form', () => {
    expect(schema().safeParse(base).success).toBe(true)
  })

  it('accepts a complete enabled form', () => {
    expect(schema().safeParse(enabled).success).toBe(true)
  })

  it('requires the server URL and base DN only when enabled', () => {
    const result = schema().safeParse({ ...base, enabled: true })
    expect(result.success).toBe(false)
    if (!result.success) {
      const paths = result.error.issues.map((i) => i.path[0])
      expect(paths).toEqual(expect.arrayContaining(['url', 'base_dn']))
    }
  })

  it('accepts ldap:// and ldaps:// but rejects other schemes', () => {
    expect(isLdapUrl('ldap://host:389')).toBe(true)
    expect(isLdapUrl('ldaps://host:636')).toBe(true)
    expect(isLdapUrl('https://host')).toBe(false)
    expect(isLdapUrl('not a url')).toBe(false)

    const result = schema().safeParse({ ...enabled, url: 'https://host' })
    expect(result.success).toBe(false)
    if (!result.success) {
      expect(result.error.issues.map((i) => i.path[0])).toContain('url')
    }
  })

  it('requires the {username} placeholder in the filter', () => {
    const result = schema().safeParse({
      ...enabled,
      user_filter: '(objectClass=person)',
    })
    expect(result.success).toBe(false)
    if (!result.success) {
      expect(result.error.issues.map((i) => i.path[0])).toContain(
        'user_filter',
      )
    }
  })

  it('rejects StartTLS combined with ldaps://', () => {
    const result = schema().safeParse({
      ...enabled,
      url: 'ldaps://directory.example.com',
      starttls: true,
    })
    expect(result.success).toBe(false)
    if (!result.success) {
      expect(result.error.issues.map((i) => i.path[0])).toContain('starttls')
    }
  })

  it('bounds the timeout to 1–300 seconds', () => {
    for (const timeout of ['0', '301', 'abc', '1.5']) {
      expect(
        schema().safeParse({ ...enabled, timeout_secs: timeout }).success,
      ).toBe(false)
    }
    expect(schema().safeParse({ ...enabled, timeout_secs: '1' }).success).toBe(
      true,
    )
    expect(
      schema().safeParse({ ...enabled, timeout_secs: '300' }).success,
    ).toBe(true)
  })

  it('allows an anonymous bind (no DN and password)', () => {
    expect(
      schema().safeParse({ ...enabled, bind_dn: '', bind_password: '' })
        .success,
    ).toBe(true)
  })
})
