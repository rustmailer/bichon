//
// Copyright (c) 2025-2026 rustmailer.com (https://rustmailer.com)
//
// Audit log page (Pro edition) — one-line summaries for audit rows.
//

import type { AuditRecord } from '@/api/audit/api'

export function describeEvent(rec: AuditRecord): string {
  const p = rec.payload ?? {}
  switch (rec.event_type) {
    case 'email.viewed': {
      const subject = typeof p.subject === 'string' ? p.subject : rec.email_id ?? ''
      // Name the delegation when there was one: "who read this" is only half
      // the record, the other half is whether they were entitled to it. The
      // full before/after scope is in the payload below.
      return typeof p.via_view === 'string' ? `${subject} · via ${p.via_view}` : subject
    }
    case 'email.deleted':
      return typeof p.subject === 'string' ? p.subject : rec.email_id ?? ''
    case 'email.batch_deleted': {
      // A batch row stands for many messages; the per-message evidence lives
      // in the detail file linked from the expanded payload.
      const count = typeof p.count === 'number' ? p.count : null
      return count === null ? '' : `${count} email(s)`
    }
    case 'email.exported':
    case 'email.restored':
      return typeof p.subject === 'string' ? p.subject : rec.email_id ?? ''
    case 'email.repaired': {
      // The outcome belongs in the detail, not just the label: a `failed`
      // repair rewrote nothing and must not read like a successful one.
      const subject = typeof p.subject === 'string' ? p.subject : rec.email_id ?? ''
      return typeof p.status === 'string' ? `${subject} · ${p.status}` : subject
    }
    case 'email.tagged':
      return typeof p.count === 'number' ? `${p.count} email(s)` : ''
    case 'attachment.downloaded':
    case 'attachment.previewed':
      return typeof p.filename === 'string' ? p.filename : rec.content_hash ?? ''
    case 'attachment.tagged':
      return typeof p.count === 'number' ? `${p.count} attachment(s)` : ''
    case 'search.performed':
      return typeof p.query === 'string' ? `"${p.query}"` : ''
    case 'user.created':
      return typeof p.new_user === 'string' ? p.new_user : ''
    case 'user.updated':
    case 'user.removed':
      return typeof p.target_user === 'string' ? p.target_user : ''
    case 'role.created':
    case 'role.updated':
    case 'role.removed':
      return typeof p.role === 'string' ? p.role : ''
    case 'account.updated': {
      const email = typeof p.email === 'string' ? p.email : ''
      // Both sides of the scope are in the payload; the line only has to mark
      // that a view's visibility moved, so it is not read as a rename.
      return 'view_after' in p || 'view_before' in p
        ? `${email} · view scope changed`
        : email
    }
    case 'account.created':
    case 'account.removed':
      return typeof p.email === 'string' ? p.email : ''
    case 'account.download_started':
      return typeof p.run_gap_fill === 'boolean'
        ? `gap_fill=${p.run_gap_fill}`
        : ''
    case 'account.role_assigned':
      return typeof p.target_user === 'string' ? p.target_user : ''
    case 'access_token.created':
    case 'access_token.removed':
      return typeof p.name === 'string' && p.name
        ? p.name
        : typeof p.target_user === 'string'
          ? p.target_user
          : ''
    case 'oauth2.created':
    case 'oauth2.updated':
    case 'oauth2.removed':
      return typeof p.name === 'string' && p.name ? p.name : ''
    case 'import.performed':
      return typeof p.total === 'number'
        ? `total=${p.total} success=${p.success} failed=${p.failed}`
        : ''
    case 'mailbox.removed':
      return rec.mailbox_id !== null && rec.mailbox_id !== undefined
        ? `mailbox ${rec.mailbox_id}`
        : ''
    case 'proxy.created':
    case 'proxy.updated':
    case 'proxy.removed':
      return typeof p.url === 'string' ? p.url : ''
    case 'license.uploaded':
      return typeof p.email === 'string' ? p.email : ''
    case 'mfa.reset_by_admin':
      return typeof p.target_user === 'string' ? p.target_user : ''
    case 'timestamp.anchored':
      return typeof p.leaf_count === 'number'
        ? `root=${typeof p.root_hash === 'string' ? p.root_hash.slice(0, 12) : ''}… leaves=${p.leaf_count}`
        : ''
    case 'ldap.login_failed':
      return typeof p.reason === 'string' ? p.reason : ''
    case 'siem.forward_failed':
      return typeof p.url === 'string'
        ? `${p.url} (${p.seq_from}..${p.seq_to})`
        : `seq ${p.seq_from}..${p.seq_to}`
    default:
      return ''
  }
}
