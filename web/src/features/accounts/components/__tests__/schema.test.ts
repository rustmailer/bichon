import { describe, it, expect } from 'vitest'
import { getAccountSchema, getAuthConfigSchema } from '../schema'

const t = (key: string) => key

const validAccountData = {
  email: 'user@example.com',
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

describe('Account Form Schema', () => {
  describe('email field', () => {
    const schema = getAccountSchema(false, t)

    it('rejects empty email', () => {
      const result = schema.safeParse({ ...validAccountData, email: '' })
      expect(result.success).toBe(false)
    })

    it('rejects invalid email format', () => {
      const result = schema.safeParse({
        ...validAccountData,
        email: 'not-an-email',
      })
      expect(result.success).toBe(false)
    })

    it('rejects email without @', () => {
      const result = schema.safeParse({
        ...validAccountData,
        email: 'username',
      })
      expect(result.success).toBe(false)
    })

    it('accepts valid email', () => {
      const result = schema.safeParse(validAccountData)
      expect(result.success).toBe(true)
    })
  })

  describe('imap.host field', () => {
    it('rejects empty IMAP host', () => {
      const result = getAccountSchema(false, t).safeParse({
        ...validAccountData,
        imap: { ...validAccountData.imap, host: '' },
      })
      expect(result.success).toBe(false)
    })

    it('accepts valid hostname', () => {
      const result = getAccountSchema(false, t).safeParse(validAccountData)
      expect(result.success).toBe(true)
    })

    it('accepts IP address as host', () => {
      const result = getAccountSchema(false, t).safeParse({
        ...validAccountData,
        imap: { ...validAccountData.imap, host: '192.168.1.1' },
      })
      expect(result.success).toBe(true)
    })
  })

  describe('imap.port field', () => {
    it('accepts port 993 (standard IMAP SSL)', () => {
      const result = getAccountSchema(false, t).safeParse(validAccountData)
      expect(result.success).toBe(true)
    })

    it('accepts port 143 (standard IMAP)', () => {
      const result = getAccountSchema(false, t).safeParse({
        ...validAccountData,
        imap: { ...validAccountData.imap, port: 143 },
      })
      expect(result.success).toBe(true)
    })

    it('accepts port 0 (auto-detect)', () => {
      const result = getAccountSchema(false, t).safeParse({
        ...validAccountData,
        imap: { ...validAccountData.imap, port: 0 },
      })
      expect(result.success).toBe(true)
    })

    it('rejects negative port', () => {
      const result = getAccountSchema(false, t).safeParse({
        ...validAccountData,
        imap: { ...validAccountData.imap, port: -1 },
      })
      expect(result.success).toBe(false)
    })

    it('rejects port > 65535', () => {
      const result = getAccountSchema(false, t).safeParse({
        ...validAccountData,
        imap: { ...validAccountData.imap, port: 99999 },
      })
      expect(result.success).toBe(false)
    })

    it('rejects non-integer port', () => {
      const result = getAccountSchema(false, t).safeParse({
        ...validAccountData,
        imap: { ...validAccountData.imap, port: 993.5 },
      })
      expect(result.success).toBe(false)
    })
  })

  describe('imap.encryption field', () => {
    it('accepts Ssl', () => {
      const result = getAccountSchema(false, t).safeParse(validAccountData)
      expect(result.success).toBe(true)
    })

    it('accepts StartTls', () => {
      const result = getAccountSchema(false, t).safeParse({
        ...validAccountData,
        imap: { ...validAccountData.imap, encryption: 'StartTls' },
      })
      expect(result.success).toBe(true)
    })

    it('accepts None', () => {
      const result = getAccountSchema(false, t).safeParse({
        ...validAccountData,
        imap: { ...validAccountData.imap, encryption: 'None' },
      })
      expect(result.success).toBe(true)
    })

    it('rejects invalid encryption value', () => {
      const result = getAccountSchema(false, t).safeParse({
        ...validAccountData,
        imap: { ...validAccountData.imap, encryption: 'TLS' },
      })
      expect(result.success).toBe(false)
    })
  })

  describe('download_interval_min field', () => {
    it('rejects value less than 1', () => {
      const result = getAccountSchema(false, t).safeParse({
        ...validAccountData,
        download_interval_min: 0,
      })
      expect(result.success).toBe(false)
    })

    it('accepts value of exactly 1', () => {
      const result = getAccountSchema(false, t).safeParse({
        ...validAccountData,
        download_interval_min: 1,
      })
      expect(result.success).toBe(true)
    })

    it('rejects non-integer value', () => {
      const result = getAccountSchema(false, t).safeParse({
        ...validAccountData,
        download_interval_min: 30.5,
      })
      expect(result.success).toBe(false)
    })
  })

  describe('download_batch_size field', () => {
    it('rejects value less than 10', () => {
      const result = getAccountSchema(false, t).safeParse({
        ...validAccountData,
        download_batch_size: 5,
      })
      expect(result.success).toBe(false)
    })

    it('rejects value greater than 200', () => {
      const result = getAccountSchema(false, t).safeParse({
        ...validAccountData,
        download_batch_size: 500,
      })
      expect(result.success).toBe(false)
    })

    it('accepts value of exactly 10', () => {
      const result = getAccountSchema(false, t).safeParse({
        ...validAccountData,
        download_batch_size: 10,
      })
      expect(result.success).toBe(true)
    })

    it('accepts value of exactly 200', () => {
      const result = getAccountSchema(false, t).safeParse({
        ...validAccountData,
        download_batch_size: 200,
      })
      expect(result.success).toBe(true)
    })
  })

  describe('account_name and login_name fields', () => {
    it('accepts undefined account_name and login_name', () => {
      const result = getAccountSchema(false, t).safeParse(validAccountData)
      expect(result.success).toBe(true)
    })

    it('accepts provided account_name', () => {
      const result = getAccountSchema(false, t).safeParse({
        ...validAccountData,
        account_name: 'My Work Email',
      })
      expect(result.success).toBe(true)
    })

    it('accepts provided login_name', () => {
      const result = getAccountSchema(false, t).safeParse({
        ...validAccountData,
        login_name: 'username',
      })
      expect(result.success).toBe(true)
    })
  })

  describe('download_schedule field', () => {
    it('accepts undefined download_schedule', () => {
      const result = getAccountSchema(false, t).safeParse(validAccountData)
      expect(result.success).toBe(true)
    })

    it('accepts valid 6-field cron expression', () => {
      const result = getAccountSchema(false, t).safeParse({
        ...validAccountData,
        download_schedule: '0 0 0 * * *',
      })
      expect(result.success).toBe(true)
    })

    it('accepts cron with */step syntax', () => {
      const result = getAccountSchema(false, t).safeParse({
        ...validAccountData,
        download_schedule: '0 */30 8-17 * * 1-5',
      })
      expect(result.success).toBe(true)
    })

    it('rejects cron with too few fields', () => {
      const result = getAccountSchema(false, t).safeParse({
        ...validAccountData,
        download_schedule: '0 0 *',
      })
      expect(result.success).toBe(false)
    })

    it('accepts empty string cron (treated as not set)', () => {
      const result = getAccountSchema(false, t).safeParse({
        ...validAccountData,
        download_schedule: '',
      })
      expect(result.success).toBe(true)
    })
  })
})

describe('Auth Config Schema (password validation)', () => {
  describe('when creating (isEdit = false)', () => {
    const schema = getAuthConfigSchema(false, t)

    it('requires password when auth_type is Password', () => {
      const result = schema.safeParse({
        auth_type: 'Password',
        password: '',
      })
      expect(result.success).toBe(false)
    })

    it('requires password when auth_type is Password and password undefined', () => {
      const result = schema.safeParse({
        auth_type: 'Password',
      })
      expect(result.success).toBe(false)
    })

    it('accepts valid password with Password auth', () => {
      const result = schema.safeParse({
        auth_type: 'Password',
        password: 'mypassword',
      })
      expect(result.success).toBe(true)
    })

    it('does not require password when auth_type is OAuth2', () => {
      const result = schema.safeParse({
        auth_type: 'OAuth2',
      })
      expect(result.success).toBe(true)
    })
  })

  describe('when editing (isEdit = true)', () => {
    const schema = getAuthConfigSchema(true, t)

    it('does not require password even with Password auth', () => {
      const result = schema.safeParse({
        auth_type: 'Password',
        password: '',
      })
      expect(result.success).toBe(true)
    })

    it('accepts with undefined password', () => {
      const result = schema.safeParse({
        auth_type: 'Password',
      })
      expect(result.success).toBe(true)
    })
  })
})
