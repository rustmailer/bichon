import { describe, expect, it } from 'vitest'
import type { MinimalAccount } from '@/api/account/api'
import { accountPickerLabel, accountPickerOptions } from '../account-picker'

const account = (
  id: number,
  email: string,
  name?: string
): MinimalAccount => ({
  id,
  email,
  name,
  account_type: 'IMAP' as MinimalAccount['account_type'],
})

describe('accountPickerLabel', () => {
  it('uses the display name when set, otherwise the email', () => {
    expect(accountPickerLabel(account(1, 'a@example.com', 'Work'))).toBe('Work')
    expect(accountPickerLabel(account(2, 'b@example.com'))).toBe('b@example.com')
  })

  it('ignores a blank display name', () => {
    expect(accountPickerLabel(account(3, 'c@example.com', '   '))).toBe(
      'c@example.com'
    )
  })
})

describe('accountPickerOptions', () => {
  it('sorts by the visible label, case-insensitively', () => {
    const options = accountPickerOptions(
      [
        account(1, 'zoe@example.com'),
        account(2, 'Bob@example.com'),
        account(3, 'alice@example.com'),
      ],
      'en'
    )
    expect(options.map((option) => option.label)).toEqual([
      'alice@example.com',
      'Bob@example.com',
      'zoe@example.com',
    ])
  })

  it('sorts by a display name rather than the hidden email', () => {
    const options = accountPickerOptions(
      [
        account(1, 'a@example.com', 'Zulu'),
        account(2, 'z@example.com', 'Alpha'),
      ],
      'en'
    )
    expect(options.map((option) => option.label)).toEqual(['Alpha', 'Zulu'])
  })

  it('keeps the email searchable when a display name hides it', () => {
    const [named] = accountPickerOptions(
      [account(1, 'a@example.com', 'Work')],
      'en'
    )
    expect(named.description).toBe('a@example.com')

    const [plain] = accountPickerOptions([account(2, 'b@example.com')], 'en')
    expect(plain.description).toBeUndefined()
  })

  it('falls back to email then id for equal labels', () => {
    const options = accountPickerOptions(
      [
        account(9, 'b@example.com', 'Same'),
        account(4, 'a@example.com', 'Same'),
      ],
      'en'
    )
    expect(options.map((option) => option.value)).toEqual(['4', '9'])
  })

  it('returns the account id the combobox reports back', () => {
    const [option] = accountPickerOptions([account(42, 'x@example.com')], 'en')
    expect(option.value).toBe('42')
  })
})
