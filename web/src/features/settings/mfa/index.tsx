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


import { useCallback, useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useTranslation } from 'react-i18next'
import { AxiosError } from 'axios'
import { Copy, Loader2, ShieldCheck, ShieldOff } from 'lucide-react'
import * as QRCode from 'qrcode'
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from '@/components/ui/card'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { OtpInput } from '@/components/otp-input'
import { toast } from '@/hooks/use-toast'
import { useEdition } from '@/hooks/use-edition'
import { copyToClipboard } from '@/lib/clipboard'
import { mfaConfirm, mfaDisable, mfaEnroll, mfaStatus } from '@/api/users/api'
import { Alert, AlertDescription } from '@/components/ui/alert'
import { Info } from 'lucide-react'

interface EnrollState {
  secret: string
  otpauth_uri: string
}

export function MfaSettings() {
  const { t } = useTranslation()
  const queryClient = useQueryClient()
  const { ssoEnabled } = useEdition()

  const { data: status } = useQuery({
    queryKey: ['mfa-status'],
    queryFn: mfaStatus,
  })

  const [enroll, setEnroll] = useState<EnrollState | null>(null)
  const [qrDataUrl, setQrDataUrl] = useState<string | null>(null)
  const [code, setCode] = useState('')
  const [recoveryCodes, setRecoveryCodes] = useState<string[] | null>(null)
  const [disableMode, setDisableMode] = useState(false)
  const [disableWithRecovery, setDisableWithRecovery] = useState(false)

  const enrollMutation = useMutation({
    mutationFn: mfaEnroll,
    onSuccess: async (data) => {
      setEnroll({ secret: data.secret, otpauth_uri: data.otpauth_uri })
      setRecoveryCodes(null)
      setCode('')
      try {
        setQrDataUrl(
          await QRCode.toDataURL(data.otpauth_uri, {
            margin: 1,
            width: 240,
            errorCorrectionLevel: 'M',
          }),
        )
      } catch {
        setQrDataUrl(null)
      }
    },
    onError: (error) => {
      const apiError = (error as AxiosError).response?.data as
        | { message?: string }
        | undefined
      toast({
        variant: 'destructive',
        title:
          apiError?.message ||
          t('settings.mfa.error', 'Something went wrong. Please try again.'),
      })
    },
  })

  const confirmMutation = useMutation({
    mutationFn: () => mfaConfirm(code.trim()),
    onSuccess: (data) => {
      setRecoveryCodes(data.recovery_codes)
      setEnroll(null)
      setQrDataUrl(null)
      setCode('')
      queryClient.invalidateQueries({ queryKey: ['mfa-status'] })
    },
    onError: (error) => {
      const apiError = (error as AxiosError).response?.data as
        | { message?: string }
        | undefined
      toast({
        variant: 'destructive',
        title:
          apiError?.message ||
          t('settings.mfa.invalidCode', 'Invalid verification code'),
      })
    },
  })

  const disableMutation = useMutation({
    mutationFn: () => mfaDisable(code.trim()),
    onSuccess: () => {
      setDisableMode(false)
      setCode('')
      queryClient.invalidateQueries({ queryKey: ['mfa-status'] })
      toast({ title: t('settings.mfa.disabled', 'Two-factor authentication disabled') })
    },
    onError: (error) => {
      const apiError = (error as AxiosError).response?.data as
        | { message?: string }
        | undefined
      toast({
        variant: 'destructive',
        title:
          apiError?.message ||
          t('settings.mfa.invalidCode', 'Invalid verification code'),
      })
    },
  })

  const copyText = useCallback(
    async (text: string) => {
      try {
        await copyToClipboard(text)
        toast({ title: t('settings.mfa.copied', 'Copied to clipboard') })
      } catch {
        toast({
          variant: 'destructive',
          title: t('settings.mfa.copyFailed', 'Failed to copy'),
        })
      }
    },
    [t],
  )

  return (
    <div className='w-full space-y-6 text-xs'>
      <div className='space-y-0.5'>
        <h2 className='text-xs font-bold tracking-tight'>
          {t('settings.mfa.title', 'Two-factor authentication')}
        </h2>
        <p className='text-xs text-muted-foreground'>
          {t(
            'settings.mfa.description',
            'Add a one-time code from your authenticator app as an extra layer of security when signing in with a password.',
          )}
        </p>
      </div>

      {ssoEnabled && (
        <Alert>
          <Info size={16} className='mr-2' />
          <AlertDescription className='text-xs'>
            {t(
              'settings.mfa.ssoNote',
              'Your organization uses single sign-on (SSO). Two-factor authentication here only applies to password sign-in; SSO sessions are secured by your identity provider.',
            )}
          </AlertDescription>
        </Alert>
      )}

      {recoveryCodes ? (
        <Card>
          <CardHeader>
            <CardTitle className='text-xs'>{t('settings.mfa.recoveryTitle', 'Recovery codes')}</CardTitle>
            <CardDescription className='text-xs'>
              {t(
                'settings.mfa.recoveryWarning',
                'Save these one-time codes somewhere safe. Each code can be used once to sign in if you lose access to your authenticator app.',
              )}
            </CardDescription>
          </CardHeader>
          <CardContent className='space-y-4'>
            <ul className='grid grid-cols-1 gap-2 font-mono text-xs sm:grid-cols-2'>
              {recoveryCodes.map((c) => (
                <li key={c} className='rounded border bg-muted px-2 py-1.5'>
                  {c}
                </li>
              ))}
            </ul>
            <div className='flex gap-2'>
              <Button
                variant='outline'
                size='sm'
                onClick={() => copyText(recoveryCodes.join('\n'))}
              >
                <Copy size={14} className='mr-2' />
                {t('settings.mfa.copy', 'Copy')}
              </Button>
              <Button size='sm' onClick={() => setRecoveryCodes(null)}>
                {t('settings.mfa.done', 'Done')}
              </Button>
            </div>
          </CardContent>
        </Card>
      ) : enroll ? (
        <Card>
          <CardHeader>
            <CardTitle className='text-xs'>{t('settings.mfa.enrollTitle', 'Scan with your authenticator app')}</CardTitle>
            <CardDescription className='text-xs'>
              {t(
                'settings.mfa.enrollDesc',
                'Scan the QR code with your authenticator app (Google Authenticator, 1Password, etc.), then enter the 6-digit code to confirm.',
              )}
            </CardDescription>
          </CardHeader>
          <CardContent className='space-y-4'>
            {qrDataUrl ? (
              <div className='flex justify-center'>
                <img
                  src={qrDataUrl}
                  alt='QR code'
                  width={240}
                  height={240}
                  className='rounded border p-2'
                />
              </div>
            ) : (
              <p className='text-xs text-muted-foreground'>
                {t(
                  'settings.mfa.qrUnavailable',
                  'QR code could not be generated. Use the secret below for manual entry.',
                )}
              </p>
            )}

            <div className='space-y-1'>
              <Label className='text-xs'>{t('settings.mfa.manualSecret', 'Manual entry secret')}</Label>
              <div className='flex items-center gap-2'>
                <code className='flex-1 rounded border bg-muted px-2 py-1 text-xs'>
                  {enroll.secret}
                </code>
                <Button variant='outline' size='sm' onClick={() => copyText(enroll.secret)}>
                  <Copy size={14} />
                </Button>
              </div>
            </div>

            <div className='space-y-1'>
              <Label className='text-xs'>{t('settings.mfa.otpauthUri', 'otpauth URI')}</Label>
              <div className='flex items-center gap-2'>
                <code className='flex-1 overflow-x-auto whitespace-nowrap rounded border bg-muted px-2 py-1 text-xs'>
                  {enroll.otpauth_uri}
                </code>
                <Button variant='outline' size='sm' onClick={() => copyText(enroll.otpauth_uri)}>
                  <Copy size={14} />
                </Button>
              </div>
            </div>

            <div className='space-y-1'>
              <Label htmlFor='mfa-confirm-code' className='text-xs'>
                {t('settings.mfa.codeLabel', 'Verification code')}
              </Label>
              <OtpInput
                id='mfa-confirm-code'
                value={code}
                onChange={setCode}
                aria-label={t('settings.mfa.codeLabel', 'Verification code')}
              />
            </div>

            <div className='flex gap-2'>
                <Button
                  className='text-xs'
                  disabled={confirmMutation.isPending || code.trim().length !== 6}
                  onClick={() => confirmMutation.mutate()}
                >
                  {confirmMutation.isPending && <Loader2 className='mr-2 h-4 w-4 animate-spin' />}
                  {t('settings.mfa.confirm', 'Confirm & enable')}
                </Button>
              <Button
                variant='outline'
                className='text-xs'
                onClick={() => {
                  setEnroll(null)
                  setQrDataUrl(null)
                  setCode('')
                }}
              >
                {t('settings.mfa.cancel', 'Cancel')}
              </Button>
            </div>
          </CardContent>
        </Card>
      ) : status?.enabled ? (
        <Card>
          <CardHeader>
            <CardTitle className='flex items-center gap-2 text-xs'>
              <ShieldCheck size={18} className='text-primary' />
              {t('settings.mfa.enabled', 'Two-factor authentication is enabled')}
            </CardTitle>
            <CardDescription className='text-xs'>
              {t(
                'settings.mfa.enabledDesc',
                'Your account requires a verification code from your authenticator app when signing in with a password.',
              )}
            </CardDescription>
          </CardHeader>
          <CardContent className='space-y-4'>
            {disableMode ? (
              <>
                <div className='space-y-2'>
                  <Label htmlFor='mfa-disable-code' className='text-xs'>
                    {t(
                      'settings.mfa.disableCode',
                      'Enter your current code or a recovery code to disable',
                    )}
                  </Label>
                  {disableWithRecovery ? (
                    <Input
                      id='mfa-disable-code'
                      autoComplete='one-time-code'
                      maxLength={12}
                      value={code}
                      onChange={(e) =>
                        setCode(
                          e.target.value.replace(/[^0-9A-Za-z]/g, '').toUpperCase(),
                        )
                      }
                      placeholder={t('settings.mfa.codePlaceholder', '000000')}
                      className='w-56 font-mono text-xs md:text-xs'
                    />
                  ) : (
                    <OtpInput
                      id='mfa-disable-code'
                      value={code}
                      onChange={setCode}
                      aria-label={t('settings.mfa.codeLabel', 'Verification code')}
                    />
                  )}
                  <button
                    type='button'
                    className='block text-xs text-muted-foreground underline-offset-4 transition-colors hover:text-foreground hover:underline'
                    onClick={() => {
                      setDisableWithRecovery((value) => !value)
                      setCode('')
                    }}
                  >
                    {disableWithRecovery
                      ? t('settings.mfa.useAuthenticatorCode', 'Use an authenticator code')
                      : t('settings.mfa.useRecoveryCode', 'Use a recovery code instead')}
                  </button>
                </div>
                <div className='flex gap-2'>
                    <Button
                      variant='destructive'
                      className='text-xs'
                      disabled={disableMutation.isPending || code.trim().length < 6}
                      onClick={() => disableMutation.mutate()}
                    >
                      {disableMutation.isPending && (
                        <Loader2 className='mr-2 h-4 w-4 animate-spin' />
                      )}
                      {t('settings.mfa.confirmDisable', 'Confirm disable')}
                    </Button>
                  <Button
                    variant='outline'
                    className='text-xs'
                    onClick={() => {
                      setDisableMode(false)
                      setDisableWithRecovery(false)
                      setCode('')
                    }}
                  >
                    {t('settings.mfa.cancel', 'Cancel')}
                  </Button>
                </div>
              </>
            ) : (
              <div className='flex gap-2'>
                <Button
                  variant='outline'
                  className='text-xs'
                  disabled={enrollMutation.isPending}
                  onClick={() => enrollMutation.mutate()}
                >
                  {enrollMutation.isPending && <Loader2 className='mr-2 h-4 w-4 animate-spin' />}
                  {t('settings.mfa.reEnroll', 'Re-enroll')}
                </Button>
                <Button
                  variant='destructive'
                  className='text-xs'
                  onClick={() => {
                    setDisableMode(true)
                    setDisableWithRecovery(false)
                    setCode('')
                  }}
                >
                  <ShieldOff size={16} className='mr-2' />
                  {t('settings.mfa.disable', 'Disable two-factor authentication')}
                </Button>
              </div>
            )}
          </CardContent>
        </Card>
      ) : (
        <Card>
          <CardHeader>
            <CardTitle className='text-xs'>{t('settings.mfa.notEnabled', 'Two-factor authentication is not enabled')}</CardTitle>
            <CardDescription className='text-xs'>
              {t(
                'settings.mfa.notEnabledDesc',
                'Require a 6-digit code from your authenticator app in addition to your password when signing in.',
              )}
            </CardDescription>
          </CardHeader>
          <CardContent>
            <Button className='text-xs' disabled={enrollMutation.isPending} onClick={() => enrollMutation.mutate()}>
              {enrollMutation.isPending && <Loader2 className='mr-2 h-4 w-4 animate-spin' />}
              {t('settings.mfa.enable', 'Enable two-factor authentication')}
            </Button>
          </CardContent>
        </Card>
      )}
    </div>
  )
}
