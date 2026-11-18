import { describe, expect, it } from 'vitest'
import { errorMessage, isTooManyRequests } from '../error'

const axiosError = (status: number, message: string) =>
  Object.assign(new Error(`Request failed with status code ${status}`), {
    response: { status, data: { message, code: status } },
  })

describe('errorMessage', () => {
  it('prefers the message the API returned over the axios text', () => {
    expect(
      errorMessage(
        axiosError(
          429,
          'A batch deletion is already running (job abc). Wait for it to finish or cancel it first.'
        )
      )
    ).toBe(
      'A batch deletion is already running (job abc). Wait for it to finish or cancel it first.'
    )
  })

  it('falls back to the client error when the response has no message', () => {
    expect(errorMessage(new Error('Network Error'))).toBe('Network Error')
    expect(
      errorMessage(Object.assign(new Error('boom'), { response: { status: 500 } }))
    ).toBe('boom')
  })

  it('ignores a non-string message body', () => {
    expect(
      errorMessage(
        Object.assign(new Error('fallback'), {
          response: { data: { message: { nested: true } } },
        })
      )
    ).toBe('fallback')
  })

  it('stringifies values that are not errors', () => {
    expect(errorMessage('plain')).toBe('plain')
    expect(errorMessage(undefined)).toBe('undefined')
  })
})

describe('isTooManyRequests', () => {
  it('is true only for a 429 response', () => {
    expect(isTooManyRequests(axiosError(429, 'busy'))).toBe(true)
    expect(isTooManyRequests(axiosError(409, 'conflict'))).toBe(false)
    expect(isTooManyRequests(new Error('Network Error'))).toBe(false)
    expect(isTooManyRequests(undefined)).toBe(false)
  })
})
