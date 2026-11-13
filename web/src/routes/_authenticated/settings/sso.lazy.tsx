import { createLazyFileRoute } from '@tanstack/react-router'
import { SsoSettings } from '@/features/settings/sso'

export const Route = createLazyFileRoute('/_authenticated/settings/sso')({
  component: SsoSettings,
})
