import axiosInstance from '@/api/axiosInstance'

// LDAP/AD configuration (Enterprise). Mirrors the scrubbed `LdapConfigView`
// returned by GET /api/v1/ldap — the bind password never leaves the server.

export interface LdapConfigView {
  enabled: boolean
  url: string | null
  base_dn: string | null
  bind_dn: string | null
  bind_password_set: boolean
  user_filter: string
  scope: 'subtree' | 'onelevel' | string
  username_attribute: string
  sso_id_attribute: string
  starttls: boolean
  no_tls_verify: boolean
  timeout_secs: number
  /** Required fields present, regardless of `enabled`. */
  configured: boolean
  /** Enabled + configured + licensed: what the login page uses. */
  active: boolean
  updated_at: number
}

// Partial update. Password semantics: omit or send "********" to keep the
// stored password, "" to clear it, anything else to replace it.
export interface LdapUpdate {
  enabled?: boolean
  url?: string | null
  base_dn?: string | null
  bind_dn?: string | null
  bind_password?: string
  user_filter?: string
  scope?: string
  username_attribute?: string
  sso_id_attribute?: string
  starttls?: boolean
  no_tls_verify?: boolean
  timeout_secs?: number
}

/** Values currently in the form, so an admin can test them before saving. */
export interface LdapTestRequest {
  url?: string | null
  base_dn?: string | null
  bind_dn?: string | null
  bind_password?: string
  starttls?: boolean
  no_tls_verify?: boolean
  timeout_secs?: number
}

export interface LdapTestResult {
  ok: boolean
  /** Stable error category: connection_refused, timeout, tls, bind_failed… */
  kind?: string
  error?: string
  hint?: string
  server?: string
  base_dn?: string
}

export async function get_ldap_config(): Promise<LdapConfigView> {
  const { data } = await axiosInstance.get<LdapConfigView>('api/v1/ldap')
  return data
}

export async function update_ldap_config(
  payload: LdapUpdate,
): Promise<LdapConfigView> {
  const { data } = await axiosInstance.put<LdapConfigView>(
    'api/v1/ldap',
    payload,
  )
  return data
}

export async function test_ldap_config(
  payload: LdapTestRequest,
): Promise<LdapTestResult> {
  const { data } = await axiosInstance.post<LdapTestResult>(
    'api/v1/ldap/test',
    payload,
  )
  return data
}
