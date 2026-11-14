import { Loader2, PlugZap, Save, Trash2 } from 'lucide-react'
import { useEffect, useMemo, useRef, useState } from 'react'
import { useForm } from 'react-hook-form'
import { useQueryClient } from '@tanstack/react-query'
import { zodResolver } from '@hookform/resolvers/zod'
import { useTranslation } from 'react-i18next'
import { Navigate } from '@tanstack/react-router'
import {
  get_ldap_config,
  test_ldap_config,
  update_ldap_config,
  type LdapConfigView,
  type LdapTestRequest,
  type LdapTestResult,
  type LdapUpdate,
} from '@/api/ldap/api'
import { Button } from '@/components/ui/button'
import {
  Form,
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
} from '@/components/ui/form'
import { Input } from '@/components/ui/input'
import { PasswordInput } from '@/components/password-input'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { Switch } from '@/components/ui/switch'
import { useEdition } from '@/hooks/use-edition'
import { useCurrentUser } from '@/hooks/use-current-user'
import {
  ldapSchema,
  type LdapFormValues,
} from '@/features/settings/ldap/schema'

function errorMessage(err: unknown): string | undefined {
  const e = err as {
    response?: { data?: { message?: string } }
    message?: string
  }
  return e?.response?.data?.message || e?.message
}

/** The server classifies connection failures; the page turns each category
 *  into one short sentence. Unknown categories fall back to the server hint. */
const TEST_ERROR_KEYS: Record<string, string> = {
  invalid_url: 'settings.ldap.errInvalidUrl',
  connection_refused: 'settings.ldap.errConnectionRefused',
  timeout: 'settings.ldap.errTimeout',
  tls: 'settings.ldap.errTls',
  bind_failed: 'settings.ldap.errBindFailed',
  invalid_credentials: 'settings.ldap.errBindFailed',
  base_dn_not_found: 'settings.ldap.errBaseDn',
  insufficient_access: 'settings.ldap.errAccess',
  server_error: 'settings.ldap.errGeneric',
}

function valuesFromConfig(cfg: LdapConfigView): LdapFormValues {
  return {
    enabled: cfg.enabled,
    url: cfg.url ?? '',
    base_dn: cfg.base_dn ?? '',
    bind_dn: cfg.bind_dn ?? '',
    bind_password: '',
    user_filter: cfg.user_filter,
    scope: cfg.scope === 'onelevel' ? 'onelevel' : 'subtree',
    username_attribute: cfg.username_attribute,
    sso_id_attribute: cfg.sso_id_attribute,
    starttls: cfg.starttls,
    no_tls_verify: cfg.no_tls_verify,
    timeout_secs: String(cfg.timeout_secs),
  }
}

export function LdapSettings() {
  const { t } = useTranslation()
  const { isEnterprise, isLoading: editionLoading } = useEdition()
  const { canGlobal, isLoading: userLoading } = useCurrentUser()
  const queryClient = useQueryClient()

  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)
  const [testing, setTesting] = useState(false)
  const [current, setCurrent] = useState<LdapConfigView | null>(null)
  const [loadError, setLoadError] = useState('')
  const [clearSecret, setClearSecret] = useState(false)
  const [testResult, setTestResult] = useState<LdapTestResult | null>(null)
  const [testError, setTestError] = useState('')
  const [notice, setNotice] = useState<{ ok: boolean; text: string } | null>(
    null,
  )

  const canManage =
    isEnterprise && (canGlobal('system:root') || canGlobal('user:manage'))

  const passwordAvailable = Boolean(current?.bind_password_set) && !clearSecret
  const passwordAvailableRef = useRef(passwordAvailable)
  passwordAvailableRef.current = passwordAvailable

  const schema = useMemo(
    () => ldapSchema(t, () => passwordAvailableRef.current),
    [t],
  )

  const form = useForm<LdapFormValues>({
    resolver: zodResolver(schema),
    mode: 'onSubmit',
    defaultValues: valuesFromConfig({
      enabled: false,
      url: null,
      base_dn: null,
      bind_dn: null,
      bind_password_set: false,
      user_filter: '(&(objectClass=person)(sAMAccountName={username}))',
      scope: 'subtree',
      username_attribute: 'sAMAccountName',
      sso_id_attribute: 'distinguishedName',
      starttls: false,
      no_tls_verify: false,
      timeout_secs: 10,
      configured: false,
      active: false,
      updated_at: 0,
    }),
  })

  const testErrorMessage =
    testResult && !testResult.ok
      ? TEST_ERROR_KEYS[testResult.kind ?? '']
        ? t(TEST_ERROR_KEYS[testResult.kind ?? ''])
        : testResult.hint || testResult.error || ''
      : ''

  // `useForm` returns a new wrapper whenever form state changes, so tie the
  // load effect to the (stable) translator and call `reset` through a ref.
  const resetForm = useRef(form.reset)
  resetForm.current = form.reset

  useEffect(() => {
    if (!canManage) {
      setLoading(false)
      return
    }
    let mounted = true
    get_ldap_config()
      .then((cfg) => {
        if (!mounted) return
        setCurrent(cfg)
        resetForm.current(valuesFromConfig(cfg))
      })
      .catch((err) => {
        if (mounted) {
          setLoadError(
            errorMessage(err) ||
              t('settings.ldap.loadFailed', 'Failed to load LDAP configuration'),
          )
        }
      })
      .finally(() => {
        if (mounted) setLoading(false)
      })
    return () => {
      mounted = false
    }
  }, [canManage, t])

  const basePayload = (values: LdapFormValues) => ({
    enabled: values.enabled,
    url: values.url.trim(),
    base_dn: values.base_dn.trim(),
    bind_dn: values.bind_dn.trim(),
    user_filter: values.user_filter.trim(),
    scope: values.scope,
    username_attribute: values.username_attribute.trim(),
    sso_id_attribute: values.sso_id_attribute.trim(),
    starttls: values.starttls,
    no_tls_verify: values.no_tls_verify,
    timeout_secs: Number(values.timeout_secs),
  })

  const onSubmit = async (values: LdapFormValues) => {
    setSaving(true)
    setNotice(null)
    try {
      const payload: LdapUpdate = basePayload(values)
      // Secrets: omit keeps the stored value, "" clears it.
      if (clearSecret) {
        payload.bind_password = ''
      } else if (values.bind_password.trim()) {
        payload.bind_password = values.bind_password
      }
      const saved = await update_ldap_config(payload)
      setCurrent(saved)
      // `/features` is cached for the life of the page; without this the
      // sign-in page would keep offering the old LDAP state after logout.
      queryClient.invalidateQueries({ queryKey: ['edition'] })
      setClearSecret(false)
      form.reset(valuesFromConfig(saved))
      setNotice({
        ok: true,
        text: t('settings.ldap.saved', 'LDAP configuration saved'),
      })
    } catch (err) {
      setNotice({
        ok: false,
        text:
          errorMessage(err) ||
          t('settings.ldap.saveFailed', 'Failed to save LDAP configuration'),
      })
    } finally {
      setSaving(false)
    }
  }

  const handleTest = async () => {
    // Test the values currently in the form, not the stored ones — that is
    // the point of testing before saving. Fail closed on invalid input.
    if (!(await form.trigger())) return
    const values = form.getValues()
    const payload: LdapTestRequest = {
      ...basePayload(values),
      // Omit an untouched password so the server uses the stored one.
      bind_password: values.bind_password.trim() || undefined,
    }
    setTesting(true)
    setTestResult(null)
    setTestError('')
    try {
      setTestResult(await test_ldap_config(payload))
    } catch (err) {
      setTestError(errorMessage(err) || '')
    } finally {
      setTesting(false)
    }
  }

  const toggleClearSecret = () => {
    const next = !clearSecret
    setClearSecret(next)
    if (next) form.setValue('bind_password', '')
    void form.trigger('bind_password')
  }

  if (userLoading || editionLoading) {
    return (
      <div className='flex h-64 items-center justify-center'>
        <Loader2 className='h-6 w-6 animate-spin' />
      </div>
    )
  }

  if (!canManage) {
    return <Navigate to='/settings/profile' replace />
  }

  if (loading) {
    return (
      <div className='flex h-64 items-center justify-center'>
        <Loader2 className='h-6 w-6 animate-spin' />
      </div>
    )
  }

  if (loadError) {
    return (
      <div className='w-full max-w-7xl space-y-4 px-4'>
        <p className='text-sm font-medium text-destructive'>{loadError}</p>
      </div>
    )
  }

  return (
    <div className='w-full max-w-7xl space-y-6 px-4'>
      <div className='space-y-2'>
        <h2 className='text-xl font-bold'>
          {t('settings.ldap.title', 'LDAP / Active Directory')}
        </h2>
        <p className='text-sm text-muted-foreground'>
          {t(
            'settings.ldap.description',
            'Sign users in with their directory username and password. Bichon verifies the credentials with a bind against your LDAP server and never stores the password. Changes take effect immediately — no restart.',
          )}
        </p>
      </div>

      <Form {...form}>
        <form
          onSubmit={form.handleSubmit(onSubmit)}
          className='space-y-6 rounded-lg border p-6'
        >
          <FormField
            control={form.control}
            name='enabled'
            render={({ field }) => (
              <FormItem className='flex items-center justify-between gap-4'>
                <div className='space-y-1'>
                  <FormLabel>
                    {t('settings.ldap.enabled', 'LDAP sign-in enabled')}
                  </FormLabel>
                  <FormDescription>
                    {t(
                      'settings.ldap.enabledHint',
                      'When enabled, the sign-in page offers LDAP/AD. Every required field must be configured first.',
                    )}
                  </FormDescription>
                </div>
                <FormControl>
                  <Switch
                    checked={field.value}
                    onCheckedChange={(checked) => {
                      field.onChange(checked)
                      if (checked) void form.trigger()
                    }}
                  />
                </FormControl>
              </FormItem>
            )}
          />

          <FormField
            control={form.control}
            name='url'
            render={({ field }) => (
              <FormItem>
                <FormLabel>
                  {t('settings.ldap.serverUrl', 'Server URL')}
                </FormLabel>
                <FormControl>
                  <Input {...field} placeholder='ldap://directory.example.com:389' />
                </FormControl>
                <FormDescription>
                  {t(
                    'settings.ldap.serverUrlHint',
                    'Use ldap:// for StartTLS or plain LDAP, and ldaps:// for LDAP over TLS.',
                  )}
                </FormDescription>
                <FormMessage />
              </FormItem>
            )}
          />

          <FormField
            control={form.control}
            name='base_dn'
            render={({ field }) => (
              <FormItem>
                <FormLabel>{t('settings.ldap.baseDn', 'Base DN')}</FormLabel>
                <FormControl>
                  <Input {...field} placeholder='dc=example,dc=com' />
                </FormControl>
                <FormDescription>
                  {t(
                    'settings.ldap.baseDnHint',
                    'The subtree searched for user entries.',
                  )}
                </FormDescription>
                <FormMessage />
              </FormItem>
            )}
          />

          <div className='grid gap-4 sm:grid-cols-2'>
            <FormField
              control={form.control}
              name='bind_dn'
              render={({ field }) => (
                <FormItem>
                  <FormLabel>
                    {t(
                      'settings.ldap.bindDn',
                      'Service-account DN (optional)',
                    )}
                  </FormLabel>
                  <FormControl>
                    <Input
                      {...field}
                      placeholder='cn=svc-bichon,ou=services,dc=example,dc=com'
                    />
                  </FormControl>
                  <FormDescription>
                    {t(
                      'settings.ldap.bindDnHint',
                      'Used for the initial search bind. Leave empty for an anonymous or per-user bind.',
                    )}
                  </FormDescription>
                  <FormMessage />
                </FormItem>
              )}
            />

            <FormField
              control={form.control}
              name='bind_password'
              render={({ field }) => (
                <FormItem>
                  <FormLabel>
                    {t(
                      'settings.ldap.bindPassword',
                      'Service-account password',
                    )}
                  </FormLabel>
                  <FormControl>
                    <PasswordInput
                      {...field}
                      disabled={clearSecret}
                      placeholder={
                        current?.bind_password_set && !clearSecret
                          ? t('settings.ldap.secretSet', '******** (unchanged)')
                          : undefined
                      }
                    />
                  </FormControl>
                  {current?.bind_password_set && (
                    <Button
                      type='button'
                      variant='ghost'
                      size='sm'
                      className='h-6 px-2 text-xs'
                      onClick={toggleClearSecret}
                    >
                      <Trash2 className='mr-1 h-3 w-3' />
                      {clearSecret
                        ? t('common.cancel', 'Cancel')
                        : t('settings.ldap.clearSecret', 'Clear')}
                    </Button>
                  )}
                  <FormMessage />
                </FormItem>
              )}
            />
          </div>

          <FormField
            control={form.control}
            name='user_filter'
            render={({ field }) => (
              <FormItem>
                <FormLabel>
                  {t('settings.ldap.userFilter', 'User search filter')}
                </FormLabel>
                <FormControl>
                  <Input
                    {...field}
                    placeholder='(&(objectClass=person)(sAMAccountName={username}))'
                  />
                </FormControl>
                <FormDescription>
                  {t(
                    'settings.ldap.userFilterHint',
                    '{username} is replaced with what the user typed. The filter must match exactly one entry.',
                  )}
                </FormDescription>
                <FormMessage />
              </FormItem>
            )}
          />

          <div className='grid gap-4 sm:grid-cols-3'>
            <FormField
              control={form.control}
              name='scope'
              render={({ field }) => (
                <FormItem>
                  <FormLabel>{t('settings.ldap.scope', 'Search scope')}</FormLabel>
                  <Select
                    value={field.value}
                    onValueChange={field.onChange}
                  >
                    <FormControl>
                      <SelectTrigger>
                        <SelectValue />
                      </SelectTrigger>
                    </FormControl>
                    <SelectContent>
                      <SelectItem value='subtree'>
                        {t('settings.ldap.scopeSubtree', 'Whole subtree')}
                      </SelectItem>
                      <SelectItem value='onelevel'>
                        {t('settings.ldap.scopeOneLevel', 'One level')}
                      </SelectItem>
                    </SelectContent>
                  </Select>
                  <FormMessage />
                </FormItem>
              )}
            />

            <FormField
              control={form.control}
              name='username_attribute'
              render={({ field }) => (
                <FormItem>
                  <FormLabel>
                    {t(
                      'settings.ldap.usernameAttribute',
                      'Username attribute',
                    )}
                  </FormLabel>
                  <FormControl>
                    <Input {...field} placeholder='sAMAccountName' />
                  </FormControl>
                  <FormMessage />
                </FormItem>
              )}
            />

            <FormField
              control={form.control}
              name='sso_id_attribute'
              render={({ field }) => (
                <FormItem>
                  <FormLabel>
                    {t('settings.ldap.ssoIdAttribute', 'SSO id attribute')}
                  </FormLabel>
                  <FormControl>
                    <Input {...field} placeholder='distinguishedName' />
                  </FormControl>
                  <FormDescription>
                    {t(
                      'settings.ldap.ssoIdHint',
                      'A stable identifier such as the DN, so a rename does not create a second account.',
                    )}
                  </FormDescription>
                  <FormMessage />
                </FormItem>
              )}
            />
          </div>

          <FormField
            control={form.control}
            name='timeout_secs'
            render={({ field }) => (
              <FormItem className='max-w-xs'>
                <FormLabel>
                  {t('settings.ldap.timeout', 'Timeout (seconds)')}
                </FormLabel>
                <FormControl>
                  <Input {...field} type='number' min={1} max={300} />
                </FormControl>
                <FormMessage />
              </FormItem>
            )}
          />

          <div className='space-y-4 rounded-lg border p-4'>
            <FormField
              control={form.control}
              name='starttls'
              render={({ field }) => (
                <FormItem className='flex items-center justify-between gap-4'>
                  <div className='space-y-1'>
                    <FormLabel>
                      {t('settings.ldap.starttls', 'Use StartTLS')}
                    </FormLabel>
                    <FormDescription>
                      {t(
                        'settings.ldap.starttlsHint',
                        'Upgrade an ldap:// connection to TLS. Not used with ldaps://.',
                      )}
                    </FormDescription>
                  </div>
                  <FormControl>
                    <Switch
                      checked={field.value}
                      onCheckedChange={field.onChange}
                    />
                  </FormControl>
                </FormItem>
              )}
            />

            <FormField
              control={form.control}
              name='no_tls_verify'
              render={({ field }) => (
                <FormItem className='flex items-center justify-between gap-4'>
                  <div className='space-y-1'>
                    <FormLabel>
                      {t(
                        'settings.ldap.noTlsVerify',
                        'Skip certificate verification',
                      )}
                    </FormLabel>
                    <FormDescription>
                      {t(
                        'settings.ldap.noTlsVerifyHint',
                        'Only for a private CA or a self-signed certificate. This disables a security check.',
                      )}
                    </FormDescription>
                  </div>
                  <FormControl>
                    <Switch
                      checked={field.value}
                      onCheckedChange={field.onChange}
                    />
                  </FormControl>
                </FormItem>
              )}
            />
          </div>

          {current && !current.active && current.configured && (
            <p className='text-sm text-muted-foreground'>
              {t(
                'settings.ldap.inactiveHint',
                'The configuration is complete but LDAP sign-in is not active yet — enable it, or check that the current license is Enterprise.',
              )}
            </p>
          )}

          <div className='flex flex-wrap items-center gap-3'>
            <Button type='submit' disabled={saving}>
              {saving ? (
                <Loader2 className='mr-2 h-4 w-4 animate-spin' />
              ) : (
                <Save className='mr-2 h-4 w-4' />
              )}
              {saving
                ? t('settings.ldap.saving', 'Saving…')
                : t('settings.ldap.save', 'Save')}
            </Button>
            <Button
              type='button'
              variant='outline'
              onClick={handleTest}
              disabled={testing}
            >
              {testing ? (
                <Loader2 className='mr-2 h-4 w-4 animate-spin' />
              ) : (
                <PlugZap className='mr-2 h-4 w-4' />
              )}
              {testing
                ? t('settings.ldap.testing', 'Testing…')
                : t('settings.ldap.testConnection', 'Test connection')}
            </Button>
          </div>

          {notice && (
            <p
              className={
                notice.ok
                  ? 'text-sm text-green-600'
                  : 'text-sm text-destructive'
              }
            >
              {notice.text}
            </p>
          )}

          {testError && (
            <p className='text-sm text-destructive'>{testError}</p>
          )}

          {testResult && (
            <div className='space-y-1 rounded-md border p-3 text-sm'>
              {testResult.ok ? (
                <>
                  <p className='font-medium text-green-600'>
                    {t(
                      'settings.ldap.testOk',
                      'Connected and the base DN is searchable.',
                    )}
                  </p>
                  {testResult.server && (
                    <p className='text-muted-foreground'>
                      {testResult.server} · {testResult.base_dn}
                    </p>
                  )}
                </>
              ) : (
                <>
                  <p className='font-medium text-destructive'>
                    {testErrorMessage ||
                      t('settings.ldap.testFailed', 'Connection test failed')}
                  </p>
                  {testResult.error && (
                    <p className='text-muted-foreground break-all'>
                      {testResult.error}
                    </p>
                  )}
                </>
              )}
            </div>
          )}
        </form>
      </Form>
    </div>
  )
}
