import { z } from 'zod'

export type SsoFormValues = {
  enabled: boolean
  issuer_url: string
  client_id: string
  client_secret: string
  redirect_uri: string
  allowed_email_domains: string
}

export function isHttpUrl(value: string): boolean {
  try {
    const url = new URL(value)
    return url.protocol === 'http:' || url.protocol === 'https:'
  } catch {
    return false
  }
}

/** Split on commas, semicolons and newlines — deliberately **not** on spaces,
 *  so a typo like `example com` is reported instead of being saved as two
 *  single-label domains. */
export function parseDomains(value: string): string[] {
  return value
    .split(/[\n,;]+/)
    .map((d) => d.trim())
    .filter(Boolean)
}

/** Mirrors `bichon_pro::sso::normalize_domain`: bare lowercase domain, no
 *  leading `@`, at most 253 characters, letters/digits/dots/hyphens only. */
export function normalizeDomain(raw: string): string | null {
  const domain = raw
    .trim()
    .replace(/^@+/, '')
    .replace(/\.+$/, '')
    .toLowerCase()
  if (!domain || domain.length > 253) return null
  if (!/^[a-z0-9.-]+$/.test(domain)) return null
  if (
    domain.startsWith('-') ||
    domain.endsWith('-') ||
    domain.startsWith('.') ||
    domain.includes('..')
  ) {
    return null
  }
  return domain
}

/** Validation lives next to the form: URL shape, domain shape and — only when
 *  SSO is being enabled — the required-field set. Errors render under the
 *  corresponding field through `FormMessage`.
 *
 *  `secretAvailable` is a getter rather than a value: the resolver is built
 *  once, but whether a secret is already stored changes after the config
 *  loads and when the admin clears it. */
export function ssoSchema(
  t: (key: string) => string,
  secretAvailable: () => boolean,
) {
  return z
    .object({
      enabled: z.boolean(),
      issuer_url: z.string().trim(),
      client_id: z.string().trim(),
      client_secret: z.string(),
      redirect_uri: z.string().trim(),
      allowed_email_domains: z.string(),
    })
    .superRefine((values, ctx) => {
      const issue = (path: keyof SsoFormValues, message: string) =>
        ctx.addIssue({ code: z.ZodIssueCode.custom, path: [path], message })

      if (values.issuer_url && !isHttpUrl(values.issuer_url)) {
        issue('issuer_url', t('settings.sso.invalidUrl'))
      }
      if (values.redirect_uri && !isHttpUrl(values.redirect_uri)) {
        issue('redirect_uri', t('settings.sso.invalidUrl'))
      }
      for (const domain of parseDomains(values.allowed_email_domains)) {
        if (!normalizeDomain(domain)) {
          issue(
            'allowed_email_domains',
            t('settings.sso.invalidDomain').replace('{{domain}}', domain),
          )
          break
        }
      }
      if (values.enabled) {
        if (!values.issuer_url) issue('issuer_url', t('validation.required'))
        if (!values.client_id) issue('client_id', t('validation.required'))
        if (!values.client_secret.trim() && !secretAvailable()) {
          issue('client_secret', t('settings.sso.secretRequired'))
        }
        if (!values.redirect_uri) {
          issue('redirect_uri', t('validation.required'))
        }
      }
    })
}
