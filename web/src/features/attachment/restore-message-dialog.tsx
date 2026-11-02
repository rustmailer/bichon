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


import { restore_message } from '@/api/mailbox/envelope/api'
import { ConfirmDialog } from '@/components/confirm-dialog'
import { toast } from '@/hooks/use-toast'
import { useMutation } from '@tanstack/react-query'
import { AxiosError } from 'axios'
import { useTranslation } from 'react-i18next'
import { ToastAction } from '@/components/ui/toast'
import { EmailEnvelope } from '@/api'
import { useAttachmentContext } from './context'
import { useEnvelope } from '@/hooks/use-envelope'

function MessageSummary({ envelope, t }: { envelope: EmailEnvelope, t: (key: string) => string }) {
    return (
        <div className="mt-3 rounded-md border bg-muted/20 p-3 text-sm overflow-hidden">
            <div className="grid grid-cols-[auto_1fr] gap-x-2 gap-y-1.5">

                <span className="font-medium text-muted-foreground">{t("mail.subject")}:</span>
                <div className="break-words font-medium">
                    {envelope.subject || <em className="italic opacity-70">(No subject)</em>}
                </div>

                <span className="font-medium text-muted-foreground">{t("mail.from")}:</span>
                <div className="break-all text-foreground/90">
                    {envelope.from}
                </div>

                {envelope.to?.length > 0 && (
                    <>
                        <span className="font-medium text-muted-foreground">{t("mail.to")}:</span>
                        <div className="break-all text-foreground/90">
                            {envelope.to.slice(0, 2).join(", ")}
                            {envelope.to.length > 2 && " …"}
                        </div>
                    </>
                )}

                <span className="font-medium text-muted-foreground">{t("mail.date")}:</span>
                <div className="text-foreground/90">
                    {new Date(envelope.date).toLocaleString()}
                </div>

                {envelope.mailbox_name && (
                    <>
                        <span className="font-medium text-muted-foreground">{t("search.mailbox")}:</span>
                        <div className="truncate text-foreground/90" title={envelope.mailbox_name}>
                            {envelope.mailbox_name}
                        </div>
                    </>
                )}
            </div>
        </div>
    );
}



interface RestoreMessageDialogProps {
    open: boolean
    onOpenChange: (open: boolean) => void
}

export function RestoreMessageDialog({
    open,
    onOpenChange
}: RestoreMessageDialogProps) {
    const { t } = useTranslation()
    const { currentAttachment } = useAttachmentContext()

    const {
        data: currentEnvelope,
    } = useEnvelope(currentAttachment?.account_id, currentAttachment?.envelope_id);


    const restoreMutation = useMutation({
        mutationFn: async () => {
            if (currentEnvelope) {
                return restore_message(currentEnvelope.account_id, [currentEnvelope.id]);
            }
        },
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
            title={t('restore_message.title', 'Restore message')}
            desc={<div className="space-y-3">
                <p className="text-sm text-muted-foreground">
                    {t(
                        'restore_message.desc',
                        'This action will append the selected messages to their corresponding mailboxes on the IMAP server.'
                    )}
                </p>

                {currentEnvelope && <MessageSummary envelope={currentEnvelope} t={t} />}
            </div>}
            confirmText={t('restore_message.confirm', 'Restore')}
            handleConfirm={() => restoreMutation.mutate()}
            className="sm:max-w-sm"
            isLoading={restoreMutation.isPending}
            disabled={restoreMutation.isPending || !currentEnvelope}
        />
    )
}
