import { useQuery } from '@tanstack/react-query'
import { get_branding } from '@/api/branding/api'
import { useEdition } from '@/hooks/use-edition'

export function useBranding() {
  // `/api/v1/branding` is registered by the Pro server only. This hook is
  // mounted from the root layout, the sign-in page and the sidebar, so an
  // unconditional query made every community install log two 404s (initial +
  // `retry: 1`). Gate on the edition instead; community keeps the built-in
  // Bichon branding.
  const { isPro } = useEdition()
  const { data } = useQuery({
    queryKey: ['branding'],
    queryFn: get_branding,
    enabled: isPro,
    staleTime: Infinity,
    retry: 1,
  })

  return {
    branding: data ?? null,
    companyName: data?.company_name ?? null,
    tagline: data?.tagline ?? null,
    logoUrl: data?.logo_url ?? null,
  } as const
}
