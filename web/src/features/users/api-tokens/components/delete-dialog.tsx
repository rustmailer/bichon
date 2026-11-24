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


import { useState } from 'react'
import { IconAlertTriangle } from '@tabler/icons-react'
import { toast } from '@/hooks/use-toast'
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { ConfirmDialog } from '@/components/confirm-dialog'
import { useMutation, useQueryClient } from '@tanstack/react-query'
import { ToastAction } from '@/components/ui/toast'
import { AxiosError } from 'axios'
import { AccessToken, remove_access_token } from '@/api/users/api'
import { OP_TOKEN_REVOKE } from '@/api/approvals/api'
import { useGatedOp } from '@/features/approvals/use-gated-op'
import { useTranslation } from 'react-i18next'

interface Props {
  open: boolean
  onOpenChange: (open: boolean) => void
  currentRow: AccessToken
}

export function ApiTokenDeleteDialog({ open, onOpenChange, currentRow }: Props) {
  const { t } = useTranslation()
  const [value, setValue] = useState('')
  const queryClient = useQueryClient()
  const { gated, announceIfQueued } = useGatedOp(OP_TOKEN_REVOKE)

  function handleSuccess(data: unknown) {
    queryClient.invalidateQueries({ queryKey: ['access-token-list'] })
    queryClient.invalidateQueries({ queryKey: ['approvals'] })
    onOpenChange(false)
    setValue('')

    // A queued revocation must not report "token deleted": the token still
    // works, and a leaked credential the operator believes is dead is worse
    // than one they know is still live.
    if (announceIfQueued(data)) return

    toast({
      title: t('users.api_tokens.toast.deleted_title'),
      description: t('users.api_tokens.toast.deleted_desc', {
        name: currentRow.name || t('users.api_tokens.unnamed'),
      }),
      action: (
        <ToastAction altText={t('common.close')}>
          {t('common.close')}
        </ToastAction>
      ),
    })
  }

  function handleError(error: AxiosError) {
    const errorMessage =
      (error.response?.data as { message?: string })?.message ||
      error.message ||
      t('users.api_tokens.toast.delete_failed')

    toast({
      variant: 'destructive',
      title: t('users.api_tokens.toast.delete_failed_title'),
      description: errorMessage,
      action: (
        <ToastAction altText={t('common.tryAgain')}>
          {t('common.tryAgain')}
        </ToastAction>
      ),
    })
  }

  const deleteMutation = useMutation({
    mutationFn: (tokenId: string) => remove_access_token(tokenId),
    onSuccess: handleSuccess,
    onError: handleError,
  })

  const handleDelete = () => {
    if (value.toLowerCase() !== 'delete') return
    deleteMutation.mutate(currentRow.token_id)
  }

  return (
    <ConfirmDialog
      open={open}
      onOpenChange={onOpenChange}
      handleConfirm={handleDelete}
      disabled={value.toLowerCase() !== 'delete' || deleteMutation.isPending}
      className="max-w-2xl"
      title={
        <span className="text-destructive">
          <IconAlertTriangle
            className="mr-1 inline-block stroke-destructive"
            size={18}
          />{' '}
          {t('users.api_tokens.delete.title')}
        </span>
      }
      desc={
        <div className="space-y-4 text-sm">
          <div className="mb-2">
            <p>
              {t('users.api_tokens.delete.desc')}：
              <span className="ml-1 font-mono font-bold text-foreground">
                "{currentRow.name || t('users.api_tokens.unnamed')}"
              </span>
            </p>
            <p className="mt-1 text-muted-foreground">
              {t('users.api_tokens.delete.prefix')}:{' '}
              <span className="font-mono">
                {currentRow.token_prefix}...
              </span>
            </p>
            <p className="mt-2 font-bold text-destructive">
              {t('users.api_tokens.delete.irreversible')}
            </p>
          </div>

          <div className="space-y-2">
            <Label>
              {t('users.api_tokens.delete.confirm_label', { word: 'delete' })}
            </Label>
            <Input
              value={value}
              onChange={(e) => setValue(e.target.value)}
              placeholder={t('users.api_tokens.delete.confirm_placeholder', {
                word: 'delete',
              })}
              className="mt-2"
              autoFocus
            />
          </div>

          {gated ? (
            <Alert>
              <AlertTitle>
                {t(
                  'approvals.gatedTokenTitle',
                  'Dual control is on for this operation'
                )}
              </AlertTitle>
              <AlertDescription>
                {t(
                  'approvals.gatedTokenDesc',
                  'This will NOT revoke the token now. It submits a request that a second person must approve, and only then is the token revoked. Until that happens the token keeps working — if it has leaked, treat it as live.'
                )}
              </AlertDescription>
            </Alert>
          ) : (
            <Alert variant="destructive">
              <AlertTitle>
                {t('users.api_tokens.delete.warning_title')}
              </AlertTitle>
              <AlertDescription>
                {t('users.api_tokens.delete.warning_desc')}
              </AlertDescription>
            </Alert>
          )}
        </div>
      }
      confirmText={
        deleteMutation.isPending
          ? t('users.api_tokens.delete.deleting')
          : gated
            ? t('approvals.submitForApproval', 'Submit for approval')
            : t('users.api_tokens.delete.confirm_button')
      }
      destructive
    />
  )
}
