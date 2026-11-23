import axiosInstance from '@/api/axiosInstance'
import { useQuery } from '@tanstack/react-query'

export interface EditionInfo {
  edition: 'community' | 'pro' | 'enterprise'
  version: string
  sso_enabled: boolean
  ldap_enabled: boolean
  siem_enabled: boolean
  approval_enabled: boolean
}

/** Product name for the running edition. A customer's configured branding
 *  still takes precedence where it is shown. */
export function editionBrandName(edition: EditionInfo['edition']): string {
  switch (edition) {
    case 'enterprise':
      return 'Bichon Enterprise'
    case 'pro':
      return 'Bichon Pro'
    default:
      return 'Bichon'
  }
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
    /** True until the first `/features` response arrives. Callers that
     *  redirect or hide whole pages must wait for this, or they will treat a
     *  Pro server as Community during the first render. */
    isLoading: data === undefined,
    isPro: data?.edition === 'pro' || data?.edition === 'enterprise',
    isEnterprise: data?.edition === 'enterprise',
    edition: data?.edition ?? 'community',
    version: data?.version ?? '',
    ssoEnabled: data?.sso_enabled ?? false,
    ldapEnabled: data?.ldap_enabled ?? false,
    siemEnabled: data?.siem_enabled ?? false,
    approvalEnabled: data?.approval_enabled ?? false,
  } as const
}
