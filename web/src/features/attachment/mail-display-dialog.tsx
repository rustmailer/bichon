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


import { useAttachmentContext } from './context'
import { Dialog, DialogContent, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { ScrollArea } from '@/components/ui/scroll-area'
import { useTranslation } from 'react-i18next'
import { MailMessageView } from './mail-message-view'
import { useEnvelope } from '@/hooks/use-envelope'


interface Props {
  open: boolean
  onOpenChange: (open: boolean) => void
}

export function MailDisplayDrawer({ open, onOpenChange }: Props) {
  const { t } = useTranslation()
  const { currentAttachment } = useAttachmentContext()

  const {
    data: envelope,
    isLoading,
    error
  } = useEnvelope(currentAttachment?.account_id, currentAttachment?.envelope_id);

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent aria-describedby={undefined} className='w-full md:max-w-6xl mx-auto h-full'>
        <div className="flex flex-col h-full overflow-hidden -m-6">
          <DialogHeader className="p-4 pb-3 border-b shrink-0 m-0">
            <DialogTitle>{t('mail.emailViewer')}</DialogTitle>
          </DialogHeader>
          <ScrollArea className="flex-1">
            <div className='m-5'>
              {isLoading ? (
                <div className="p-8 text-center text-muted-foreground">{t('common.loading')}...</div>
              ) : error ? (
                <div className="p-8 text-center text-red-500">{t('attachment.emailMessageNotFound')}</div>
              ) : envelope ? (
                <MailMessageView envelope={envelope} />
              ) : (
                <div className="p-8 text-center text-muted-foreground">{t('mail.noMessageSelected')}</div>
              )}
            </div>
          </ScrollArea>
        </div>
      </DialogContent>
    </Dialog>
  )
}