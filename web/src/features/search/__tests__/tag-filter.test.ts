import { describe, expect, it } from 'vitest'
import {
  clearTagFilter,
  countTagFilters,
  getSelectedTags,
  isUntaggedOnly,
  toggleNoTags,
  toggleTag,
} from '../tag-filter'

describe('tag filter edits', () => {
  it('adds and removes tag names', () => {
    const one = toggleTag({}, 'work')
    expect(one).toEqual({ tags: ['work'] })

    const two = toggleTag(one, 'important')
    expect(two).toEqual({ tags: ['work', 'important'] })

    // Removing the last tag drops the key rather than leaving an empty list.
    expect(toggleTag(toggleTag(two, 'work'), 'important')).toEqual({})
  })

  it('leaves the rest of the search filter untouched', () => {
    const filter = { subject: 'invoice', page: 2 }
    expect(toggleTag(filter, 'work')).toEqual({
      subject: 'invoice',
      page: 2,
      tags: ['work'],
    })
    expect(toggleNoTags(filter)).toEqual({
      subject: 'invoice',
      page: 2,
      has_tags: false,
    })
  })

  it('does not mutate the filter it is given', () => {
    const filter = { tags: ['work'] }
    toggleTag(filter, 'important')
    toggleNoTags(filter)
    clearTagFilter(filter)
    expect(filter).toEqual({ tags: ['work'] })
  })

  it('turns untagged-only on and off', () => {
    expect(toggleNoTags({})).toEqual({ has_tags: false })
    expect(toggleNoTags({ has_tags: false })).toEqual({})
  })

  it('makes "no tags" and a tag selection replace each other', () => {
    // The backend rejects the combination, so the popover must never build it.
    expect(toggleNoTags({ tags: ['work'] })).toEqual({ has_tags: false })
    expect(toggleTag({ has_tags: false }, 'work')).toEqual({ tags: ['work'] })
  })

  it('clears every tag filter and nothing else', () => {
    expect(
      clearTagFilter({ tags: ['work'], has_tags: false, subject: 'x' })
    ).toEqual({ subject: 'x' })
  })

  it('reports the active tag filters', () => {
    expect(countTagFilters({})).toBe(0)
    expect(countTagFilters({ tags: ['work', 'important'] })).toBe(2)
    expect(countTagFilters({ has_tags: false })).toBe(1)

    expect(isUntaggedOnly({ has_tags: false })).toBe(true)
    expect(isUntaggedOnly({ tags: ['work'] })).toBe(false)

    expect(getSelectedTags({ tags: ['work'] })).toEqual(['work'])
    expect(getSelectedTags(undefined)).toEqual([])
  })
})
