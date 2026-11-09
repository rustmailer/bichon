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
// Rendering the *target* of a dual-control request, so an approver can see
// exactly what they are about to authorize.
//
// This is the whole reason the request carries a `target` at all. The queue
// used to render `account_ids`, which is empty for three of the five gated
// operations — an approver would have been shown a blank card and asked to
// sign off on it.
import { useTranslation } from 'react-i18next'
import type { ApprovalTarget } from '@/api/approvals/api'
import { formatBytes } from '@/lib/utils'
import { accountMessageCounts, filterClauses } from './target-summary'

/** The blast radius, phrased as what will be destroyed.
 *
 *  Mailbox and message counts are snapshots taken when the request was made, so
 *  they are reported as approximations — mail arriving while the request waits
 *  for a decision makes them underestimates, and a number that silently drifts
 *  is worse than one that says it is approximate.
 */
export function TargetDescription({ target }: { target: ApprovalTarget }) {
  const { t } = useTranslation()

  switch (target.kind) {
    case 'accounts':
      return (
        <div>
          <div className='font-medium'>
            {t('approvals.targetAccounts', '{{count}} account(s)', {
              count: target.account_ids.length,
            })}
          </div>
          <div className='text-muted-foreground'>
            {target.account_ids.slice(0, 5).join(', ')}
            {target.account_ids.length > 5 && '…'}
          </div>
        </div>
      )

    case 'users':
      return (
        <div>
          <div className='font-medium'>
            {t('approvals.targetUsers', '{{count}} user(s)', {
              count: target.user_ids.length,
            })}
          </div>
          <div className='text-muted-foreground'>
            {target.user_ids.slice(0, 5).join(', ')}
            {target.user_ids.length > 5 && '…'}
          </div>
        </div>
      )

    case 'mailbox':
      return (
        <div>
          <div className='font-medium'>{target.name}</div>
          <div className='text-muted-foreground'>
            {t(
              'approvals.targetMailbox',
              'Mailbox #{{mailbox}} of account {{account}} — approximately {{count}} message(s), including subfolders',
              {
                mailbox: target.mailbox_id,
                account: target.account_id,
                count: target.messages,
              }
            )}
          </div>
        </div>
      )

    case 'messages': {
      const counts = accountMessageCounts(target)
      const accounts =
        counts.length > 0
          ? counts.map((c) => String(c.account_id))
          : (target.account_ids ?? []).map(String)
      const clauses = filterClauses(target.filter)
      const frozenBytes =
        typeof target.detail_bytes === 'number' && target.detail_bytes > 0
          ? target.detail_bytes
          : null
      return (
        <div>
          <div className='font-medium'>
            {t('approvals.targetMessages', '{{count}} message(s)', {
              count: target.messages,
            })}
          </div>
          <div className='text-muted-foreground'>
            {t(
              'approvals.targetMessagesAccounts',
              'across {{count}} account(s): {{ids}}',
              {
                count: accounts.length,
                ids: accounts.slice(0, 5).join(', '),
              }
            )}
          </div>
          {counts.length > 0 && (
            <ul className='mt-1 space-y-0.5 text-xs text-muted-foreground'>
              {counts.map((c) => (
                <li key={c.account_id}>
                  {t(
                    'approvals.targetMessagesAccountLine',
                    'Account {{account}}: {{count}} message(s)',
                    { account: c.account_id, count: c.count }
                  )}
                </li>
              ))}
            </ul>
          )}
          {clauses.length > 0 && (
            <div className='mt-1 text-xs text-muted-foreground'>
              <span>{t('approvals.targetMessagesFilter', 'Search filter')}: </span>
              <code className='break-all'>
                {clauses.map((c) => `${c.field}: ${c.value}`).join('; ')}
              </code>
            </div>
          )}
          {frozenBytes !== null && (
            <div className='mt-1 text-xs text-muted-foreground'>
              {t(
                'approvals.targetMessagesFrozen',
                'The exact ids are frozen in a detail file ({{size}}). Mail that arrives after the request was made is NOT included.',
                { size: formatBytes(frozenBytes) }
              )}
            </div>
          )}
        </div>
      )
    }

    case 'token':
      return (
        <div>
          <div className='font-medium'>{target.username || `#${target.user_id}`}</div>
          <div className='text-muted-foreground font-mono text-xs'>
            {t('approvals.targetToken', 'API token {{fingerprint}}', {
              fingerprint: target.fingerprint,
            })}
          </div>
        </div>
      )

    case 'erasure': {
      const accounts = Object.keys(target.by_account)
      return (
        <div>
          <div className='font-medium'>
            {t('approvals.targetErasure', 'GDPR erasure — {{subject}}', {
              subject: target.subject,
            })}
          </div>
          <div className='text-muted-foreground'>
            {t(
              'approvals.targetErasureCount',
              '{{count}} message(s) across {{accounts}} account(s), identified by: {{criteria}}',
              {
                count: target.messages,
                accounts: accounts.length,
                criteria: target.criteria,
              }
            )}
          </div>
          {/* The requester's assessment is shown on the card, not hidden behind
              a detail view: it is the half of the argument the approver is
              being asked to accept. */}
          <div className='text-muted-foreground italic'>{target.assessment}</div>
        </div>
      )
    }

    case 'export':
      return (
        <div>
          <div className='font-medium'>{target.saved_search_name}</div>
          <div className='text-muted-foreground'>
            {t(
              'approvals.targetExport',
              'Export approximately {{count}} message(s) as {{format}}',
              { count: target.messages, format: target.format }
            )}
          </div>
          {/* The scope is the part the approver is actually signing off on:
              a request queued by an administrator covers every account, one
              from a scoped role covers a named subset. `null` is the unscoped
              form — the server sends it rather than an empty list, so an empty
              list here would mean "no accounts", which is not a state the
              backend produces. */}
          <div className='text-muted-foreground'>
            {target.account_ids === null
              ? t('approvals.targetExportAllAccounts', 'from all accounts')
              : t(
                  'approvals.targetExportAccounts',
                  'from {{count}} account(s): {{ids}}',
                  {
                    count: target.account_ids.length,
                    ids: target.account_ids.slice(0, 5).join(', '),
                  }
                )}
          </div>
        </div>
      )
  }
}
