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
// Human labels for the operation discriminators the server reports.
//
// Every op the server can execute needs an entry. A missing one is not a crash
// — it renders as the raw discriminator, so `account.delete` shows up as
// `account.delete` — which is exactly why it is easy to ship without noticing.
// `features/settings/approvals/index.tsx` carries the same map for the settings
// page; the two must stay in sync with `KNOWN_OPS` on the server.
export const OP_LABELS: Record<string, { key: string; fallback: string }> = {
  'legal_hold.release': {
    key: 'settings.approvals.opLegalHoldRelease',
    fallback: 'Release a legal hold',
  },
  'user.delete': {
    key: 'settings.approvals.opUserDelete',
    fallback: 'Delete a user',
  },
  'account.delete': {
    key: 'settings.approvals.opAccountDelete',
    fallback: 'Delete an account',
  },
  'mailbox.delete': {
    key: 'settings.approvals.opMailboxDelete',
    fallback: 'Delete a mailbox',
  },
  'message.delete': {
    key: 'settings.approvals.opMessageDelete',
    fallback: 'Delete messages',
  },
  'token.revoke': {
    key: 'settings.approvals.opTokenRevoke',
    fallback: 'Revoke an API token',
  },
  'export.batch': {
    key: 'settings.approvals.opExportBatch',
    fallback: 'Export a batch of archived mail',
  },
  'gdpr.erasure': {
    key: 'settings.approvals.opGdprErasure',
    fallback: 'Execute a GDPR erasure request',
  },
  'retention.update': {
    key: 'settings.approvals.opRetentionUpdate',
    fallback: 'Weaken a retention policy',
  },
}
