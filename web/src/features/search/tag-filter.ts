//
// Copyright (c) 2025-2026 rustmailer.com (https://rustmailer.com)
//
// This file is part of the Bichon Email Archiving Project
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <http://www.gnu.org/licenses/>.

/**
 * Filter edits behind the tag popover.
 *
 * The popover drives two mutually exclusive dimensions of a search request:
 * `tags`, a non-empty list of tag names matched with OR semantics, and
 * `has_tags: false`, which asks for messages carrying no tag at all. The
 * second one exists because a tag list cannot express absence — there is no
 * tag name for "untagged" — so it is a separate flag on the backend filter
 * and the two must never be sent together.
 *
 * Keeping them in sync is the whole job of these helpers, which is why they
 * are pure functions here rather than inline `setFilter` callbacks.
 */

/**
 * The search filter as it travels through the URL, the API payload and saved
 * searches: a plain object whose keys are `EmailSearchFilter` fields.
 */
export type SearchFilter = Record<string, unknown>

/** The tag names currently selected, defaulting to none. */
export function getSelectedTags(filter: SearchFilter | undefined): string[] {
  return (filter?.tags as string[]) || []
}

/** Whether the filter asks only for messages that carry no tag at all. */
export function isUntaggedOnly(filter: SearchFilter | undefined): boolean {
  return filter?.has_tags === false
}

/** How many tag filters are active, for the popover badge. */
export function countTagFilters(filter: SearchFilter | undefined): number {
  return getSelectedTags(filter).length + (isUntaggedOnly(filter) ? 1 : 0)
}

/**
 * Add or remove one tag. Selecting a tag clears `has_tags`: the user asking
 * for a tag wants tagged mail, so the untagged-only flag must not linger and
 * contradict the request.
 */
export function toggleTag(filter: SearchFilter, tag: string): SearchFilter {
  const next = { ...filter }
  delete next.has_tags

  const current = getSelectedTags(next)
  const tags = current.includes(tag)
    ? current.filter((t) => t !== tag)
    : [...current, tag]

  if (tags.length > 0) {
    next.tags = tags
  } else {
    delete next.tags
  }

  return next
}

/**
 * Turn "no tags" on or off. It replaces any selected tags, for the same
 * reason `toggleTag` clears it: the two would otherwise describe a
 * contradiction the backend rejects.
 */
export function toggleNoTags(filter: SearchFilter): SearchFilter {
  const next = { ...filter }
  delete next.tags

  if (isUntaggedOnly(filter)) {
    delete next.has_tags
  } else {
    next.has_tags = false
  }

  return next
}

/** Drop every tag filter, leaving the rest of the search untouched. */
export function clearTagFilter(filter: SearchFilter): SearchFilter {
  const next = { ...filter }
  delete next.tags
  delete next.has_tags
  return next
}
