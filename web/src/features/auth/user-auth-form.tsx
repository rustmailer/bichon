//
// Copyright (c) 2025-2026 rustmailer.com (https://rustmailer.com)
//
// This file is part of the Bichon Email Archiving Project
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <http://www.gnu.org/licenses/>.


import { HTMLAttributes, useEffect, useState } from 'react'
import { useForm } from 'react-hook-form'
import { zodResolver } from '@hookform/resolvers/zod'
import { cn, toSearchParams } from '@/lib/utils'
import { getFormSchema, type LoginFormValues } from './schema'
import {
  Form,
  FormControl,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
} from '@/components/ui/form'
import { Input } from '@/components/ui/input'
import { PasswordInput } from '@/components/password-input'
import { useMutation } from '@tanstack/react-query'
import { setToken } from '@/stores/authStore'
import { toast } from '@/hooks/use-toast'
import { AxiosError } from 'axios'
import { ToastAction } from '@/components/ui/toast'
import { useLocation, useNavigate } from '@tanstack/react-router'
import { Button } from '@/components/button'
import { useTranslation } from 'react-i18next'
import i18n from '@/i18n'
import { KeyRound, Loader2, LogIn, Shield } from 'lucide-react'
import { ldapLogin, login, mfaVerify, type LoginResult } from '@/api/users/api'
import { useTheme } from '@/context/theme-context'
import { useEdition } from '@/hooks/use-edition'

type UserAuthFormProps = HTMLAttributes<HTMLDivElement>

function buildOidcLoginUrl(redirectTo: string): string {
  const injectedBase = (window as unknown as { __BICHON_BASE__?: string }).__BICHON_BASE__
  const base = !injectedBase || injectedBase === '/' ? '' : injectedBase.replace(/\/$/, '')
  const params = new URLSearchParams()
  if (redirectTo && redirectTo !== '/') {
    params.set('redirect_to', redirectTo)
  }
  const qs = params.toString()
  return `${base}/api/auth/oidc/login${qs ? `?${qs}` : ''}`
}

export function UserAuthForm({ className, ...props }: UserAuthFormProps) {
  const [isLoading, setIsLoading] = useState(false)
  const [mfaChallenge, setMfaChallenge] = useState<string | null>(null)
  const { setTheme } = useTheme();
  const navigate = useNavigate()
  const { t } = useTranslation()
  const { oidcEnabled, oidcAutoRedirect, ssoEnabled, ldapEnabled } = useEdition()

  const { search } = useLocation();
  const searchParams = toSearchParams(search);
  const redirect = searchParams.get('redirect') || '/';
  const localOnly = searchParams.get('local') === '1';
  const ssoError = searchParams.get('sso_error');
  // Enterprise LDAP: the form authenticates against the directory (default
  // when enabled); a link lets the user fall back to their local account.
  const [loginMode, setLoginMode] = useState<'local' | 'ldap'>(
    ldapEnabled ? 'ldap' : 'local',
  )

  useEffect(() => {
    if (ssoError) {
      toast({
        variant: 'destructive',
        title: t('auth.loginFailed'),
        description: ssoError,
      })
    }
  }, [ssoError, t])

  useEffect(() => {
    if (oidcEnabled && oidcAutoRedirect && !localOnly && !ssoError) {
      window.location.href = buildOidcLoginUrl(redirect)
    }
  }, [oidcEnabled, oidcAutoRedirect, localOnly, ssoError, redirect])

  const formSchema = getFormSchema(t)
  const form = useForm<LoginFormValues>({
    resolver: zodResolver(formSchema),
    defaultValues: {
      username: '',
      password: '',
      code: '',
    },
  })

  const mutation = useMutation({
    mutationFn: (data: Record<string, any>) => {
      if (mfaChallenge) {
        return mfaVerify(mfaChallenge, data.code)
      }
      if (loginMode === 'ldap') {
        return ldapLogin({
          username: data.username ?? '',
          password: data.password ?? '',
        })
      }
      return login(data)
    },
    retry: 0,
  });

  function handleLoginSuccess(result: LoginResult) {
    // Two-factor step: the server asks for a TOTP code before issuing a token.
    if (result.mfa_required && result.mfa_challenge) {
      setMfaChallenge(result.mfa_challenge)
      return
    }

    if (result.success && result.access_token) {
      setToken(result);

      if (result.theme) {
        setTheme(result.theme);
      }

      if (result.language) {
        i18n.changeLanguage(result.language);
      }

      navigate({ to: redirect });
    }
  }

  async function onSubmit(data: LoginFormValues) {
    if (mfaChallenge && (!data.code || data.code.trim().length !== 6)) {
      toast({
        variant: "destructive",
        title: t('auth.mfaCodeRequired', 'Please enter the 6-digit code from your authenticator app.'),
      })
      return
    }

    setIsLoading(true)

    mutation.mutate(data, {
      onSuccess: (result) => {
        if (!result.success) {
          toast({
            variant: "destructive",
            title: t('auth.loginFailed'),
            description: `${result.error_message!}`,
            action: <ToastAction altText={t('common.tryAgain')}>{t('common.tryAgain')}</ToastAction>,
          })
          setIsLoading(false);
          return
        }
        handleLoginSuccess(result)
        setIsLoading(false);
      },
      onError: (error) => {
        const { t } = i18n
        if (error instanceof AxiosError && error.response && error.response.status === 401) {
          toast({
            variant: "destructive",
            title: t('auth.loginFailed'),
            description: t('auth.invalidPassword'),
            action: <ToastAction altText={t('common.tryAgain')}>{t('common.tryAgain')}</ToastAction>,
          })
        } else {
          toast({
            variant: "destructive",
            title: t('auth.somethingWentWrong'),
            description: (error as Error).message,
            action: <ToastAction altText={t('common.tryAgain')}>{t('common.tryAgain')}</ToastAction>,
          })
        }
        setIsLoading(false)
      }
    });
  }

  return (
    <div className={cn('grid gap-6', className)} {...props}>
      <Form {...form}>
        <form onSubmit={form.handleSubmit(onSubmit)}>
          <div className='grid gap-2'>
            {mfaChallenge ? (
              <>
                <p className='text-sm text-muted-foreground'>
                  {t('auth.mfaLoginDesc', 'Enter the 6-digit code shown in your authenticator app to finish signing in.')}
                </p>
                <FormField
                  control={form.control}
                  name='code'
                  render={({ field }) => (
                    <FormItem className='space-y-1'>
                      <FormLabel>{t('auth.mfaCodePlaceholder', 'Authentication code')}</FormLabel>
                      <FormControl>
                        <Input
                          inputMode='numeric'
                          autoComplete='off'
                          maxLength={6}
                          placeholder='000000'
                          className='text-center text-lg tracking-[0.5em]'
                          {...field}
                        />
                      </FormControl>
                      <FormMessage />
                    </FormItem>
                  )}
                />
              </>
            ) : (
              <>
                <FormField
                  control={form.control}
                  name='username'
                  render={({ field }) => (
                    <FormItem className='space-y-1'>
                      <FormLabel>{t('auth.username')}</FormLabel>
                      <FormControl>
                        <Input autoComplete='username' {...field} />
                      </FormControl>
                      <FormMessage />
                    </FormItem>
                  )}
                />
                <FormField
                  control={form.control}
                  name='password'
                  render={({ field }) => (
                    <FormItem className='space-y-1'>
                      <div className='flex items-center justify-between'>
                        <FormLabel>{t('auth.password')}</FormLabel>
                      </div>
                      <FormControl>
                        <PasswordInput autoComplete='current-password' placeholder='********' {...field} />
                      </FormControl>
                      <FormMessage />
                    </FormItem>
                  )}
                />
                <Button className='mt-2 w-full' disabled={isLoading}>
                  {isLoading ? <Loader2 className='animate-spin' /> : <LogIn size={16} className='mr-2' />}
                  {t('auth.login')}
                </Button>

                {(oidcEnabled || ssoEnabled) && (
                  <Button
                    variant='outline'
                    className='mt-2 w-full'
                    type='button'
                    onClick={() => {
                      // Fork community SSO and upstream Pro SSO share the
                      // same OIDC endpoints; preserve redirect + base path.
                      window.location.href = buildOidcLoginUrl(redirect)
                    }}
                  >
                    <Shield size={16} className='mr-2' />
                    {t('auth.ssoLogin')}
                  </Button>
                )}

                {/* `ldapEnabled` already means "Enterprise + flag set" — the
                    server folds the license into it so the form can never
                    offer a mode the login endpoint would refuse. */}
                {ldapEnabled && (
                  <Button
                    variant='link'
                    className='mt-2 w-full'
                    type='button'
                    onClick={() => setLoginMode((m) => (m === 'ldap' ? 'local' : 'ldap'))}
                  >
                    {loginMode === 'ldap'
                      ? t('auth.localLogin', 'Sign in with your local account')
                      : t('auth.ldapLogin', 'Sign in with LDAP')}
                  </Button>
                )}
              </>
            )}

            {mfaChallenge && (
              <Button className='mt-2 w-full' disabled={isLoading}>
                {isLoading ? <Loader2 className='animate-spin' /> : <KeyRound size={16} className='mr-2' />}
                {t('auth.mfaVerify', 'Verify')}
              </Button>
            )}
          </div>
        </form>
      </Form>
    </div>
  )
}
