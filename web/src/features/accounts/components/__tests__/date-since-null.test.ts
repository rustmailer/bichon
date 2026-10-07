import { describe, it, expect } from 'vitest'
import { getAccountSchema } from '../schema'

const t = (key: string) => key

// An existing account as loaded into the settings form (password cleared).
const editData = {
  email: 'test@example.com',
  imap: {
    host: 'imap.example.com',
    port: 993,
    encryption: 'Ssl' as const,
    auth: { auth_type: 'Password' as const, password: undefined },
  },
  enabled: true,
  use_dangerous: false,
  download_interval_min: 60,
  download_batch_size: 30,
  max_email_size_bytes: 100 * 1024 * 1024,
  auto_download_new_mailboxes: false,
}

describe('settings form accepts date_since as returned by the API', () => {
  const schema = getAccountSchema(true, t)

  it('accepts a fixed date with relative: null', () => {
    const result = schema.safeParse({
      ...editData,
      date_since: { fixed: '2026-09-01', relative: null },
    })
    expect(result.success).toBe(true)
  })

  it('accepts a relative date with fixed: null', () => {
    const result = schema.safeParse({
      ...editData,
      date_since: { fixed: null, relative: { unit: 'Months', value: 1 } },
    })
    expect(result.success).toBe(true)
  })

  it('still rejects an empty fixed date', () => {
    const result = schema.safeParse({
      ...editData,
      date_since: { fixed: '', relative: null },
    })
    expect(result.success).toBe(false)
  })

  it('still rejects an invalid relative value next to fixed: null', () => {
    const result = schema.safeParse({
      ...editData,
      date_since: { fixed: null, relative: { unit: 'Months', value: 0 } },
    })
    expect(result.success).toBe(false)
  })
})
