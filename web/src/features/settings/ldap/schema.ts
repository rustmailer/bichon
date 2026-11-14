import { z } from 'zod'

export type LdapFormValues = {
  enabled: boolean
  url: string
  base_dn: string
  bind_dn: string
  bind_password: string
  user_filter: string
  scope: 'subtree' | 'onelevel'
  username_attribute: string
  sso_id_attribute: string
  starttls: boolean
  no_tls_verify: boolean
  timeout_secs: string
}

/** `ldap://` and `ldaps://` are the only schemes the server accepts. */
export function isLdapUrl(value: string): boolean {
  try {
    const url = new URL(value)
    return url.protocol === 'ldap:' || url.protocol === 'ldaps:'
  } catch {
    return false
  }
}

/** Validation lives next to the form: URL shape, the `{username}` placeholder
 *  in the filter, and — only when LDAP is being enabled — the required-field
 *  set. Errors render under the corresponding field through `FormMessage`.
 *
 *  `secretAvailable` is a getter rather than a value: the resolver is built
 *  once, but whether a password is already stored changes after the config
 *  loads and when the admin clears it. */
export function ldapSchema(
  t: (key: string) => string,
  secretAvailable: () => boolean,
) {
  return z
    .object({
      enabled: z.boolean(),
      url: z.string().trim(),
      base_dn: z.string().trim(),
      bind_dn: z.string().trim(),
      bind_password: z.string(),
      user_filter: z.string().trim(),
      scope: z.enum(['subtree', 'onelevel']),
      username_attribute: z.string().trim(),
      sso_id_attribute: z.string().trim(),
      starttls: z.boolean(),
      no_tls_verify: z.boolean(),
      timeout_secs: z.string().trim(),
    })
    .superRefine((values, ctx) => {
      const issue = (path: keyof LdapFormValues, message: string) =>
        ctx.addIssue({ code: z.ZodIssueCode.custom, path: [path], message })

      if (values.url && !isLdapUrl(values.url)) {
        issue('url', t('settings.ldap.invalidUrl'))
      }
      if (values.url.startsWith('ldaps://') && values.starttls) {
        issue('starttls', t('settings.ldap.starttlsWithLdaps'))
      }
      if (
        values.user_filter &&
        !values.user_filter.includes('{username}')
      ) {
        issue('user_filter', t('settings.ldap.invalidFilter'))
      }
      const timeout = Number(values.timeout_secs)
      if (
        !Number.isInteger(timeout) ||
        timeout < 1 ||
        timeout > 300
      ) {
        issue('timeout_secs', t('settings.ldap.invalidTimeout'))
      }

      if (values.enabled) {
        if (!values.url) issue('url', t('validation.required'))
        if (!values.base_dn) issue('base_dn', t('validation.required'))
        if (!values.user_filter) {
          issue('user_filter', t('validation.required'))
        }
        if (!values.username_attribute) {
          issue('username_attribute', t('validation.required'))
        }
        if (!values.sso_id_attribute) {
          issue('sso_id_attribute', t('validation.required'))
        }
        // An anonymous or per-user bind is valid, so the bind DN and password
        // stay optional.
        void secretAvailable
      }
    })
}
