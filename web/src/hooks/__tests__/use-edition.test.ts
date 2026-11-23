import { describe, expect, it } from 'vitest'
import { editionBrandName } from '../use-edition'

describe('editionBrandName', () => {
  it('names the community edition', () => {
    expect(editionBrandName('community')).toBe('Bichon')
  })

  it('names the pro edition', () => {
    expect(editionBrandName('pro')).toBe('Bichon Pro')
  })

  it('names the enterprise edition', () => {
    expect(editionBrandName('enterprise')).toBe('Bichon Enterprise')
  })
})
