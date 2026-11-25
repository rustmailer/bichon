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

import { useForm } from 'react-hook-form'
import { zodResolver } from '@hookform/resolvers/zod'
import { z } from 'zod'
import { useMutation } from '@tanstack/react-query'
import { useTranslation } from 'react-i18next'
import { useLocation, useNavigate } from '@tanstack/react-router'
import { AxiosError } from 'axios'
import { Loader2, KeyRound } from 'lucide-react'
import {
  Form,
  FormControl,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
} from '@/components/ui/form'
import { PasswordInput } from '@/components/password-input'
import { Button } from '@/components/button'
import { toast } from '@/hooks/use-toast'
import { setToken } from '@/stores/authStore'
import { changePassword } from '@/api/users/api'
import { useTheme } from '@/context/theme-context'
import { isTheme } from '@/lib/theme'
import { toSearchParams } from '@/lib/utils'
import i18n from '@/i18n'

function getSchema(t: (key: string) => string) {
  return z
    .object({
      newPassword: z
        .string()
        .min(8, { message: t('settings.profile.validation.password.min') })
        .max(256, { message: t('settings.profile.validation.password.max') }),
      confirmPassword: z.string().min(1, {
        message: t('validation.pleaseEnterPassword'),
      }),
    })
    .refine((v) => v.newPassword === v.confirmPassword, {
      message: t('auth.passwordsDoNotMatch'),
      path: ['confirmPassword'],
    })
}

type ChangePasswordValues = z.infer<ReturnType<typeof getSchema>>

type ChangePasswordFormProps = {
  ticket: string
  onCancel: () => void
}

/**
 * Shown instead of the app when a local-password login returned
 * `must_change_password` (built-in administrator still on the default
 * password). The server issued no session token, only a one-time ticket.
 */
export function ChangePasswordForm({ ticket, onCancel }: ChangePasswordFormProps) {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const { search } = useLocation()
  const { setTheme } = useTheme()
  const redirect = toSearchParams(search).get('redirect') || '/'

  const form = useForm<ChangePasswordValues>({
    resolver: zodResolver(getSchema(t)),
    defaultValues: { newPassword: '', confirmPassword: '' },
  })

  const mutation = useMutation({
    mutationFn: (values: ChangePasswordValues) =>
      changePassword(ticket, values.newPassword),
    retry: 0,
  })

  function onSubmit(values: ChangePasswordValues) {
    mutation.mutate(values, {
      onSuccess: (result) => {
        if (result.success && result.access_token) {
          setToken(result)
          if (isTheme(result.theme)) {
            setTheme(result.theme)
          }
          if (result.language) {
            i18n.changeLanguage(result.language)
          }
          navigate({ to: redirect })
          return
        }
        toast({
          variant: 'destructive',
          title: t('auth.somethingWentWrong'),
          description: result.error_message || t('auth.somethingWentWrong'),
        })
      },
      onError: (error) => {
        const err = error as AxiosError<{ message?: string }>
        toast({
          variant: 'destructive',
          title: t('auth.somethingWentWrong'),
          description: err.response?.data?.message || err.message,
        })
      },
    })
  }

  return (
    <Form {...form}>
      <form
        onSubmit={form.handleSubmit(onSubmit)}
        className='space-y-4'
        noValidate
      >
        <FormField
          control={form.control}
          name='newPassword'
          render={({ field }) => (
            <FormItem>
              <FormLabel>{t('auth.newPassword', 'New password')}</FormLabel>
              <FormControl>
                <PasswordInput
                  autoComplete='new-password'
                  placeholder={t('auth.newPassword', 'New password')}
                  {...field}
                />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />
        <FormField
          control={form.control}
          name='confirmPassword'
          render={({ field }) => (
            <FormItem>
              <FormLabel>
                {t('auth.confirmPassword', 'Confirm new password')}
              </FormLabel>
              <FormControl>
                <PasswordInput
                  autoComplete='new-password'
                  placeholder={t('auth.confirmPassword', 'Confirm new password')}
                  {...field}
                />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />
        <Button
          type='submit'
          className='mt-2 w-full'
          disabled={mutation.isPending}
        >
          {mutation.isPending ? (
            <>
              <Loader2 className='mr-2 h-4 w-4 animate-spin' />
              {t('auth.setPassword', 'Set password')}
            </>
          ) : (
            <>
              <KeyRound className='mr-2 h-4 w-4' />
              {t('auth.setPassword', 'Set password')}
            </>
          )}
        </Button>
        <Button
          type='button'
          variant='ghost'
          className='w-full'
          disabled={mutation.isPending}
          onClick={onCancel}
        >
          {t('common.cancel', 'Cancel')}
        </Button>
      </form>
    </Form>
  )
}
