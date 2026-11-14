import { createLazyFileRoute } from '@tanstack/react-router'
import { LdapSettings } from '@/features/settings/ldap'

export const Route = createLazyFileRoute('/_authenticated/settings/ldap')({
  component: LdapSettings,
})
