import axiosInstance from '@/api/axiosInstance'
import { useQuery } from '@tanstack/react-query'

export interface EditionInfo {
  edition: 'community' | 'pro' | 'enterprise'
  version: string
  features?: string[]
  oidc_enabled?: boolean
  oidc_auto_redirect?: boolean
  sso_enabled?: boolean
  ldap_enabled?: boolean
  siem_enabled?: boolean
  // Upstream Pro flag; the community features endpoint does not send it,
  // so it stays optional and falls back to false below.
  approval_enabled?: boolean
}

async function fetchEdition(): Promise<EditionInfo> {
  const { data } = await axiosInstance.get<EditionInfo>('api/v1/features')
  return data
}

export function useEdition() {
  const { data } = useQuery({
    queryKey: ['edition'],
    queryFn: fetchEdition,
    staleTime: Infinity,
    retry: 1,
  })

  return {
    isPro: data?.edition === 'pro' || data?.edition === 'enterprise',
    isEnterprise: data?.edition === 'enterprise',
    edition: data?.edition ?? 'community',
    version: data?.version ?? '',
    features: data?.features ?? [],
    // Fork OIDC (community SSO) and upstream Pro SSO flags both gate the
    // SSO button; either one enables it.
    oidcEnabled: data?.oidc_enabled ?? false,
    oidcAutoRedirect: data?.oidc_auto_redirect ?? false,
    ssoEnabled: (data?.sso_enabled ?? false) || (data?.oidc_enabled ?? false),
    ldapEnabled: data?.ldap_enabled ?? false,
    siemEnabled: data?.siem_enabled ?? false,
    approvalEnabled: data?.approval_enabled ?? false,
  } as const
}
