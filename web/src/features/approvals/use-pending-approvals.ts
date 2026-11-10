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
//
// "Is this thing already waiting for a second person?"
//
// A gated delete no longer changes anything, so an operator who queues one gets
// no feedback from the list they were looking at — the row is unchanged, and
// the only evidence is a toast that scrolls away. The natural reaction is to
// click again, which queues a second identical request. Two approvers can then
// each approve one and the second executes against a target that is already
// gone, or worse, a target that has been recreated since.
//
// So every list page asks this hook before offering the delete, and the answer
// disables it. The request itself is what carries the target, which is why the
// matching is done on `target` and not on the row's own ids: the queue is the
// only place that knows what has actually been asked for.
import { useQuery } from '@tanstack/react-query'
import { list_approvals, type ApprovalRequest, type ApprovalTarget } from '@/api/approvals/api'
import { useEdition } from '@/hooks/use-edition'
import { useCurrentUser } from '@/hooks/use-current-user'

/** A target flattened to the ids it will destroy, so two requests can be
 *  compared without caring which operation produced them. */
function target_keys(target: ApprovalTarget): string[] {
  switch (target.kind) {
    case 'accounts':
      return target.account_ids.map((id) => `account:${id}`)
    case 'hold_case':
      // Keyed on the case, not the account: releasing MATTER-1 must not
      // disable the row for MATTER-2.
      return [`hold:${target.hold_id}`]
    case 'users':
      return target.user_ids.map((id) => `user:${id}`)
    case 'mailbox':
      return [`mailbox:${target.account_id}:${target.mailbox_id}`]
    case 'messages':
      // Per message, not per account: two different messages in one account
      // are two unrelated deletions, and blocking the second because the first
      // is queued would make the archive unusable for exactly the case dual
      // control is meant to survive — routine cleanup during an investigation.
      return Object.entries(target.by_account).flatMap(([account, ids]) =>
        ids.map((id) => `message:${account}:${id}`)
      )
    case 'token':
      // Keyed on the fingerprint rather than the owner: one user may hold many
      // tokens, and revoking one is not revoking another. This is also what
      // keeps a queued revocation from blocking the token list's own delete
      // button for every other token that user owns.
      return [`token:${target.fingerprint}`]
    case 'export':
      // Keyed on the saved search, not on the accounts: the export is
      // identified by the search it runs, and queuing two exports of the same
      // search is the duplicate this hook exists to prevent. Two *different*
      // searches overlapping in scope are two different exports, and blocking
      // the second would be wrong — an investigator exporting one search must
      // not be locked out of exporting another.
      return [`export:${target.saved_search_id}`]
    case 'erasure':
      // Keyed per message, like a deletion, so the same bulk action can ask
      // "how many of the selected messages are already spoken for". The op is
      // part of the key at the call site, so a queued *deletion* does not block
      // asking for an erasure record of the same message — the two are
      // different requests, and the erasure is the one that carries the
      // assessment.
      return Object.entries(target.by_account).flatMap(([account, ids]) =>
        ids.map((id) => `message:${account}:${id}`)
      )
    case 'retention':
      // One policy per account: two queued changes to the same account could
      // otherwise overwrite each other in either order.
      return [`retention:${target.account_id}`]
  }
}

/** A request that is still in play: decided requests have already acted (or
 *  been refused), so they must not keep a row disabled forever. */
const still_open = (r: ApprovalRequest): boolean =>
  r.status === 'pending' || r.status === 'executing' || r.status === 'approved'

/**
 * The targets with a request already in flight.
 *
 * `hasPending(op, key)` answers whether `key` — one of the strings
 * `target_keys` produces — is already covered. Both the op and the key have to
 * match: a pending mailbox deletion must not block deleting a *different*
 * mailbox in the same account.
 *
 * A failed read yields an empty set, i.e. the buttons stay enabled. That is
 * the deliberate direction to fail in — the server gates the request anyway,
 * so the worst case is a duplicate request an approver can reject, whereas
 * guessing the other way would hide a delete button with no explanation.
 */
export function usePendingApprovals() {
  const { isEnterprise } = useEdition()
  const { require_any_permission } = useCurrentUser()

  // The queue is readable with either permission; anyone who can delete is
  // expected to hold one of them. Without a token the query stays off.
  const canRead =
    isEnterprise && require_any_permission(['approval:decide', 'compliance:audit'])

  const { data } = useQuery({
    queryKey: ['approvals', 'pending'],
    queryFn: () => list_approvals('pending'),
    enabled: canRead,
    // Short: the whole point is to notice a request the *user* just queued on
    // another page, and to release the row once it has been decided.
    staleTime: 15_000,
  })

  const keys = new Set<string>()
  for (const req of data ?? []) {
    if (!still_open(req)) continue
    for (const key of target_keys(req.target)) keys.add(`${req.op}|${key}`)
  }

  return {
    /** Whether a request for `op` against `key` is already waiting or running. */
    hasPending: (op: string, key: string): boolean => keys.has(`${op}|${key}`),

    /** Convenience for the per-account rows, which are the common case. */
    hasPendingAccount: (op: string, account_id: number): boolean =>
      keys.has(`${op}|account:${account_id}`),

    hasPendingUser: (op: string, user_id: number): boolean =>
      keys.has(`${op}|user:${user_id}`),

    /** Whether this exact message is already queued for deletion. */
    hasPendingMessage: (op: string, account_id: number, email_id: string): boolean =>
      keys.has(`${op}|message:${account_id}:${email_id}`),

    /** How many of `email_ids` are already queued, so a bulk action can say so
     *  instead of silently ignoring part of the selection. */
    countPendingMessages: (
      op: string,
      by_account: Map<number, Set<string>>
    ): number => {
      let n = 0
      for (const [account_id, ids] of by_account) {
        for (const id of ids) {
          if (keys.has(`${op}|message:${account_id}:${id}`)) n++
        }
      }
      return n
    },
  }
}
