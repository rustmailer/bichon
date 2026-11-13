import { Loader2, PlugZap, Save, Trash2 } from 'lucide-react'
import { useEffect, useMemo, useRef, useState } from 'react'
import { useForm } from 'react-hook-form'
import { zodResolver } from '@hookform/resolvers/zod'
import { useTranslation } from 'react-i18next'
import { Navigate } from '@tanstack/react-router'
import {
  get_sso_config,
  test_sso_config,
  update_sso_config,
  type SsoConfigView,
  type SsoTestResult,
  type SsoUpdate,
} from '@/api/sso/api'
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
import { Switch } from '@/components/ui/switch'
import { Textarea } from '@/components/ui/textarea'
import { useEdition } from '@/hooks/use-edition'
import { useCurrentUser } from '@/hooks/use-current-user'
import {
  parseDomains,
  ssoSchema,
  type SsoFormValues,
} from '@/features/settings/sso/schema'

/** The callback URL this deployment expects, derived from the browser origin
 *  and the injected base path. The admin still has to register it verbatim at
 *  the identity provider. */
function suggestedRedirectUri(): string {
  const injected = (window as unknown as { __BICHON_BASE__?: string })
    .__BICHON_BASE__
  const base = (injected || '/').replace(/\/+$/, '')
  return `${window.location.origin}${base}/api/auth/oidc/callback`
}

function errorMessage(err: unknown): string | undefined {
  const e = err as {
    response?: { data?: { message?: string } }
    message?: string
  }
  return e?.response?.data?.message || e?.message
}

/** The server classifies discovery failures; the page turns each category
 *  into one short sentence. Unknown categories fall back to the server hint. */
const TEST_ERROR_KEYS: Record<string, string> = {
  invalid_issuer: 'settings.sso.errInvalidIssuer',
  dns: 'settings.sso.errDns',
  connection_refused: 'settings.sso.errRefused',
  timeout: 'settings.sso.errTimeout',
  tls: 'settings.sso.errTls',
  network: 'settings.sso.errNetwork',
  redirect: 'settings.sso.errRedirect',
  not_found: 'settings.sso.errNotFound',
  access_denied: 'settings.sso.errAccessDenied',
  server_error: 'settings.sso.errServerError',
  http_error: 'settings.sso.errServerError',
  not_json: 'settings.sso.errNotJson',
  incomplete: 'settings.sso.errIncomplete',
  issuer_mismatch: 'settings.sso.errIssuerMismatch',
}

function valuesFromConfig(cfg: SsoConfigView): SsoFormValues {
  return {
    enabled: cfg.enabled,
    issuer_url: cfg.issuer_url ?? '',
    client_id: cfg.client_id ?? '',
    client_secret: '',
    redirect_uri: cfg.redirect_uri ?? suggestedRedirectUri(),
    allowed_email_domains: cfg.allowed_email_domains.join(', '),
  }
}

export function SsoSettings() {
  const { t } = useTranslation()
  const { isPro, isLoading: editionLoading } = useEdition()
  const {
    require_any_permission,
    isLoading: userLoading,
  } = useCurrentUser()

  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)
  const [testing, setTesting] = useState(false)
  const [current, setCurrent] = useState<SsoConfigView | null>(null)
  const [loadError, setLoadError] = useState('')
  const [clearSecret, setClearSecret] = useState(false)
  const [testResult, setTestResult] = useState<SsoTestResult | null>(null)
  const [testError, setTestError] = useState('')
  const [notice, setNotice] = useState<{ ok: boolean; text: string } | null>(
    null,
  )

  const canManage =
    isPro && require_any_permission(['system:root', 'user:manage'])

  const secretAvailable = Boolean(current?.client_secret_set) && !clearSecret
  // The resolver is created once; this getter lets it observe the current
  // value without rebuilding the form.
  const secretAvailableRef = useRef(secretAvailable)
  secretAvailableRef.current = secretAvailable

  const schema = useMemo(
    () => ssoSchema(t, () => secretAvailableRef.current),
    [t],
  )

  const form = useForm<SsoFormValues>({
    resolver: zodResolver(schema),
    mode: 'onSubmit',
    defaultValues: valuesFromConfig({
      enabled: false,
      issuer_url: null,
      client_id: null,
      client_secret_set: false,
      redirect_uri: null,
      allowed_email_domains: [],
      configured: false,
      active: false,
      updated_at: 0,
    }),
  })

  const issuerValue = form.watch('issuer_url')

  const testErrorKey =
    testResult && !testResult.ok
      ? TEST_ERROR_KEYS[testResult.kind ?? '']
      : undefined
  const testErrorMessage =
    testResult && !testResult.ok
      ? testErrorKey
        ? t(testErrorKey, { status: testResult.status ?? '-' })
        : testResult.hint || testResult.error || ''
      : ''

  // `useForm` returns a new wrapper object whenever form state changes, so a
  // `[form]` dependency would refetch the config on every `reset()` — which
  // itself changes form state, producing an endless GET loop. Call `reset`
  // through a ref and keep the effect tied to the (stable) translator only.
  const resetForm = useRef(form.reset)
  resetForm.current = form.reset

  useEffect(() => {
    // Configuration is admin-only on the server; do not even issue the GET
    // for a user who cannot manage it (the page will redirect instead).
    if (!canManage) {
      setLoading(false)
      return
    }
    let mounted = true
    get_sso_config()
      .then((cfg) => {
        if (!mounted) return
        setCurrent(cfg)
        resetForm.current(valuesFromConfig(cfg))
      })
      .catch((err) => {
        if (mounted) {
          setLoadError(
            errorMessage(err) ||
              t('settings.sso.loadFailed', 'Failed to load SSO configuration'),
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

  const onSubmit = async (values: SsoFormValues) => {
    setSaving(true)
    setNotice(null)
    try {
      const payload: SsoUpdate = {
        enabled: values.enabled,
        issuer_url: values.issuer_url.trim(),
        client_id: values.client_id.trim(),
        redirect_uri: values.redirect_uri.trim(),
        allowed_email_domains: parseDomains(values.allowed_email_domains),
      }
      // Secrets: omit keeps the stored value, "" clears it.
      if (clearSecret) {
        payload.client_secret = ''
      } else if (values.client_secret.trim()) {
        payload.client_secret = values.client_secret
      }
      const saved = await update_sso_config(payload)
      setCurrent(saved)
      setClearSecret(false)
      form.reset(valuesFromConfig(saved))
      setNotice({
        ok: true,
        text: t('settings.sso.saved', 'SSO configuration saved'),
      })
    } catch (err) {
      setNotice({
        ok: false,
        text:
          errorMessage(err) ||
          t('settings.sso.saveFailed', 'Failed to save SSO configuration'),
      })
    } finally {
      setSaving(false)
    }
  }

  const handleTest = async () => {
    setTesting(true)
    setTestResult(null)
    setTestError('')
    try {
      const result = await test_sso_config(
        form.getValues('issuer_url').trim() || null,
      )
      setTestResult(result)
    } catch (err) {
      setTestError(errorMessage(err) || '')
    } finally {
      setTesting(false)
    }
  }

  const toggleClearSecret = () => {
    const next = !clearSecret
    setClearSecret(next)
    if (next) form.setValue('client_secret', '')
    void form.trigger('client_secret')
  }

  // Wait for the current user's permissions before redirecting; otherwise an
  // admin whose user query is still loading would be bounced as well.
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
          {t('settings.sso.title', 'Single Sign-On (OIDC)')}
        </h2>
        <p className='text-sm text-muted-foreground'>
          {t(
            'settings.sso.description',
            'Sign users in through your identity provider using the OIDC Authorization Code flow with PKCE. Changes take effect immediately — no restart.',
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
                  <FormLabel>{t('settings.sso.enabled', 'SSO enabled')}</FormLabel>
                  <FormDescription>
                    {t(
                      'settings.sso.enabledHint',
                      'When enabled, the sign-in page shows an SSO button. Every required field must be configured first.',
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
            name='issuer_url'
            render={({ field }) => (
              <FormItem>
                <FormLabel>
                  {t('settings.sso.issuerUrl', 'Issuer URL')}
                </FormLabel>
                <FormControl>
                  <Input
                    {...field}
                    placeholder='https://keycloak.example.com/realms/myorg'
                  />
                </FormControl>
                <FormDescription>
                  {t(
                    'settings.sso.issuerHint',
                    'The issuer is used to fetch its .well-known/openid-configuration document. Any OIDC-compliant provider works.',
                  )}
                </FormDescription>
                <FormMessage />
              </FormItem>
            )}
          />

          <div className='grid gap-4 sm:grid-cols-2'>
            <FormField
              control={form.control}
              name='client_id'
              render={({ field }) => (
                <FormItem>
                  <FormLabel>
                    {t('settings.sso.clientId', 'Client ID')}
                  </FormLabel>
                  <FormControl>
                    <Input {...field} />
                  </FormControl>
                  <FormMessage />
                </FormItem>
              )}
            />

            <FormField
              control={form.control}
              name='client_secret'
              render={({ field }) => (
                <FormItem>
                  <FormLabel>
                    {t('settings.sso.clientSecret', 'Client secret')}
                  </FormLabel>
                  <FormControl>
                    <PasswordInput
                      {...field}
                      disabled={clearSecret}
                      placeholder={
                        current?.client_secret_set && !clearSecret
                          ? t('settings.sso.secretSet', '******** (unchanged)')
                          : undefined
                      }
                    />
                  </FormControl>
                  {current?.client_secret_set && (
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
                        : t('settings.sso.clearSecret', 'Clear')}
                    </Button>
                  )}
                  <FormMessage />
                </FormItem>
              )}
            />
          </div>

          <FormField
            control={form.control}
            name='redirect_uri'
            render={({ field }) => (
              <FormItem>
                <FormLabel>
                  {t('settings.sso.redirectUri', 'Redirect URI')}
                </FormLabel>
                <FormControl>
                  <Input {...field} placeholder={suggestedRedirectUri()} />
                </FormControl>
                <FormDescription>
                  {t(
                    'settings.sso.redirectHint',
                    'Register this exact URL at your identity provider. After login, users are returned to this host — BICHON_PUBLIC_URL no longer needs to be set for SSO.',
                  )}
                </FormDescription>
                <FormMessage />
              </FormItem>
            )}
          />

          <FormField
            control={form.control}
            name='allowed_email_domains'
            render={({ field }) => (
              <FormItem>
                <FormLabel>
                  {t(
                    'settings.sso.allowedDomains',
                    'Allowed email domains (optional)',
                  )}
                </FormLabel>
                <FormControl>
                  <Textarea
                    {...field}
                    rows={3}
                    placeholder='example.com, subsidiary.example.com'
                  />
                </FormControl>
                <FormDescription>
                  {t(
                    'settings.sso.allowedDomainsHint',
                    'Comma or newline separated. When set, only users whose email address is on one of these domains may sign in. Leave empty to accept everyone the identity provider authenticates.',
                  )}
                </FormDescription>
                <FormMessage />
              </FormItem>
            )}
          />

          <div className='space-y-3 border-t pt-4'>
            <div className='flex flex-wrap items-center gap-2'>
              <Button
                type='button'
                variant='outline'
                onClick={handleTest}
                disabled={testing || !issuerValue.trim()}
              >
                {testing ? (
                  <Loader2 className='mr-2 h-4 w-4 animate-spin' />
                ) : (
                  <PlugZap className='mr-2 h-4 w-4' />
                )}
                {t('settings.sso.test', 'Test connection')}
              </Button>
              <Button type='submit' disabled={saving}>
                {saving ? (
                  <Loader2 className='mr-2 h-4 w-4 animate-spin' />
                ) : (
                  <Save className='mr-2 h-4 w-4' />
                )}
                {t('settings.sso.save', 'Save configuration')}
              </Button>
            </div>

            {testError && (
              <p className='text-sm font-medium text-destructive'>{testError}</p>
            )}
            {testResult && !testResult.ok && (
              <div className='space-y-1 rounded-md border border-destructive/40 bg-destructive/5 p-3'>
                <p className='text-sm font-medium text-destructive'>
                  {t('settings.sso.testFailed', 'Connection test failed')}
                </p>
                <p className='text-sm'>{testErrorMessage}</p>
                {(testResult.url || testResult.error) && (
                  <details className='text-xs text-muted-foreground'>
                    <summary className='cursor-pointer select-none'>
                      {t('settings.sso.technicalDetails', 'Technical details')}
                    </summary>
                    {testResult.url && (
                      <p className='mt-1 break-all font-mono'>
                        {testResult.url}
                      </p>
                    )}
                    {testResult.error && (
                      <p className='break-all font-mono'>{testResult.error}</p>
                    )}
                  </details>
                )}
              </div>
            )}
            {testResult?.ok && (
              <p className='text-sm font-medium text-emerald-600'>
                {t('settings.sso.testOk', 'Connection successful')}
              </p>
            )}

            {notice && (
              <p
                className={
                  notice.ok
                    ? 'text-sm font-medium text-emerald-600'
                    : 'text-sm font-medium text-destructive'
                }
              >
                {notice.text}
              </p>
            )}
          </div>
        </form>
      </Form>
    </div>
  )
}
