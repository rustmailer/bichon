import axiosInstance from '@/api/axiosInstance'

// OIDC SSO configuration (Pro). Mirrors the scrubbed `SsoConfigView` returned
// by GET /api/v1/sso — the client secret never leaves the server.

export interface SsoConfigView {
  enabled: boolean
  issuer_url: string | null
  client_id: string | null
  client_secret_set: boolean
  redirect_uri: string | null
  allowed_email_domains: string[]
  /** Every required field is present, regardless of `enabled`. */
  configured: boolean
  /** Enabled + configured + licensed: what the login page uses. */
  active: boolean
  updated_at: number
}

// Partial update. Secret semantics: omit or send "********" to keep the stored
// secret, "" to clear it, anything else to replace it.
export interface SsoUpdate {
  enabled?: boolean
  issuer_url?: string | null
  client_id?: string | null
  client_secret?: string
  redirect_uri?: string | null
  allowed_email_domains?: string[]
}

export interface SsoTestResult {
  ok: boolean
  /** Error category: dns, connection_refused, timeout, tls, not_found… */
  kind?: string
  /** The URL that was probed (shown in the technical details). */
  url?: string
  /** HTTP status when the server answered. */
  status?: number | null
  /** Plain-language fallback sentence from the server. */
  hint?: string
  issuer?: string
  authorization_endpoint?: string
  token_endpoint?: string
  userinfo_endpoint?: string | null
  jwks_uri?: string
  end_session_endpoint?: string | null
  error?: string
}

export async function get_sso_config(): Promise<SsoConfigView> {
  const { data } = await axiosInstance.get<SsoConfigView>('api/v1/sso')
  return data
}

export async function update_sso_config(
  payload: SsoUpdate,
): Promise<SsoConfigView> {
  const { data } = await axiosInstance.put<SsoConfigView>('api/v1/sso', payload)
  return data
}

/** Resolve the issuer's discovery document. `issuer_url` lets the page test
 *  the value currently in the form, before saving it. */
export async function test_sso_config(
  issuer_url?: string | null,
): Promise<SsoTestResult> {
  const { data } = await axiosInstance.post<SsoTestResult>(
    'api/v1/sso/test',
    { issuer_url: issuer_url || null },
  )
  return data
}
