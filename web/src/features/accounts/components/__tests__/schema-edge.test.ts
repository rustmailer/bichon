import { describe, it, expect } from 'vitest'
import { getAccountSchema } from '../schema'

const t = (key: string) => key

const baseData = {
  email: 'test@example.com',
  imap: {
    host: 'imap.example.com',
    port: 993,
    encryption: 'Ssl' as const,
    auth: {
      auth_type: 'Password' as const,
      password: 'mypassword',
    },
  },
  enabled: true,
  use_dangerous: false,
  uidonly_enabled: false,
  download_interval_min: 60,
  download_batch_size: 30,
  auto_download_new_mailboxes: true,
}

describe('Account Schema - date_since validation', () => {
  const schema = getAccountSchema(false, t)

  it('accepts fixed date_since', () => {
    const result = schema.safeParse({
      ...baseData,
      date_since: { fixed: '2024-01-01' },
    })
    expect(result.success).toBe(true)
  })

  it('accepts relative date_since', () => {
    const result = schema.safeParse({
      ...baseData,
      date_since: { relative: { unit: 'Months', value: 6 } },
    })
    expect(result.success).toBe(true)
  })

  it('accepts undefined date_since', () => {
    const result = schema.safeParse(baseData)
    expect(result.success).toBe(true)
  })

  it('rejects relative date_since with value 0', () => {
    const result = schema.safeParse({
      ...baseData,
      date_since: { relative: { unit: 'Months', value: 0 } },
    })
    expect(result.success).toBe(false)
  })

  it('rejects relative date_since with negative value', () => {
    const result = schema.safeParse({
      ...baseData,
      date_since: { relative: { unit: 'Months', value: -1 } },
    })
    expect(result.success).toBe(false)
  })

  it('rejects relative date_since with non-integer value', () => {
    const result = schema.safeParse({
      ...baseData,
      date_since: { relative: { unit: 'Months', value: 1.5 } },
    })
    expect(result.success).toBe(false)
  })

  it('rejects fixed date_since with empty string', () => {
    const result = schema.safeParse({
      ...baseData,
      date_since: { fixed: '' },
    })
    expect(result.success).toBe(false)
  })
})

describe('Account Schema - date_before validation', () => {
  const schema = getAccountSchema(false, t)

  it('accepts valid date_before', () => {
    const result = schema.safeParse({
      ...baseData,
      date_before: { unit: 'Days', value: 30 },
    })
    expect(result.success).toBe(true)
  })

  it('accepts undefined date_before', () => {
    const result = schema.safeParse(baseData)
    expect(result.success).toBe(true)
  })

  it('rejects date_before with value 0', () => {
    const result = schema.safeParse({
      ...baseData,
      date_before: { unit: 'Days', value: 0 },
    })
    expect(result.success).toBe(false)
  })
})

describe('Account Schema - use_dangerous and enabled flags', () => {
  const schema = getAccountSchema(false, t)

  it('accepts use_dangerous: true', () => {
    const result = schema.safeParse({ ...baseData, use_dangerous: true })
    expect(result.success).toBe(true)
  })

  it('accepts enabled: false', () => {
    const result = schema.safeParse({ ...baseData, enabled: false })
    expect(result.success).toBe(true)
  })

  it('accepts auto_download_new_mailboxes: false', () => {
    const result = schema.safeParse({
      ...baseData,
      auto_download_new_mailboxes: false,
    })
    expect(result.success).toBe(true)
  })
})

describe('Account Schema - missing required nested fields', () => {
  const schema = getAccountSchema(false, t)

  it('rejects missing imap entirely', () => {
    const { imap, ...noImap } = baseData
    const result = schema.safeParse(noImap)
    expect(result.success).toBe(false)
  })

  it('rejects missing imap.auth', () => {
    const { auth, ...noAuth } = baseData.imap
    const result = schema.safeParse({
      ...baseData,
      imap: noAuth,
    })
    expect(result.success).toBe(false)
  })
})
