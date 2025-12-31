//
// Copyright (c) 2025 rustmailer.com (https://rustmailer.com)
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


import { restore_message } from '@/api/mailbox/envelope/api'
import { ConfirmDialog } from '@/components/confirm-dialog'
import { toast } from '@/hooks/use-toast'
import { useMutation } from '@tanstack/react-query'
import { AxiosError } from 'axios'
import { useTranslation } from 'react-i18next'
import { useMailboxContext } from '../context'
import { ToastAction } from '@/components/ui/toast'

interface RestoreMessageDialogProps {
    open: boolean
    onOpenChange: (open: boolean) => void
}

export function RestoreMessageDialog({
    open,
    onOpenChange
}: RestoreMessageDialogProps) {
    const { t } = useTranslation()
    const { selectedAccountId, selected, setSelected } = useMailboxContext();


    const restoreMutation = useMutation({
        mutationFn: (messageIds: number[]) =>
            restore_message(selectedAccountId!, messageIds),
        onSuccess: handleRestoreSuccess,
        onError: handleRestoreError,
    });

    function handleRestoreSuccess() {
        toast({
            title: t('restore_message.success', 'Messages restored'),
            description: t(
                'restore_message.successDesc',
                'The selected messages have been restored to the IMAP server.'
            ),
            action: (
                <ToastAction altText={t('common.close')}>
                    {t('common.close')}
                </ToastAction>
            ),
        });
        setSelected(new Set());
        onOpenChange(false);
    }

    function handleRestoreError(error: AxiosError) {
        const errorMessage =
            (error.response?.data as { message?: string })?.message ||
            error.message ||
            t('restore_message.failed', 'Failed to restore messages');

        toast({
            variant: 'destructive',
            title: t(
                'restore_message.failedTitle',
                'Restore failed'
            ),
            description: errorMessage,
            action: (
                <ToastAction altText={t('common.tryAgain')}>
                    {t('common.tryAgain')}
                </ToastAction>
            ),
        });

        console.error(error);
    }


    return (
        <ConfirmDialog
            open={open}
            onOpenChange={onOpenChange}
            title={t('restore_message.title', 'Restore messages')}
            desc={t(
                'restore_message.desc',
                'This action will append the selected messages from Bichon to their corresponding mailboxes on the IMAP server.'
            )}
            confirmText={t('restore_message.confirm', 'Restore')}
            handleConfirm={() => restoreMutation.mutate(Array.from(selected))}
            className="sm:max-w-sm"
            isLoading={restoreMutation.isPending}
            disabled={restoreMutation.isPending}
        />
    )
}
