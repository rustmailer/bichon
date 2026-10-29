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


import { useSearchContext } from './context'
import { Dialog, DialogContent, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { ScrollArea } from '@/components/ui/scroll-area'
import { MailMessageView } from './mail-message-view'
import { useTranslation } from 'react-i18next'


interface Props {
  open: boolean
  onOpenChange: (open: boolean) => void
}

export function MailDisplayDrawer({ open, onOpenChange }: Props) {
  const { t } = useTranslation()
  const { currentEnvelope } = useSearchContext()


  return (
    <Dialog
      open={open}
      onOpenChange={onOpenChange}
    >
      <DialogContent aria-describedby={undefined} className='w-full md:max-w-6xl mx-auto h-full'>
        <div className="flex flex-col h-full overflow-hidden -m-6">
          <DialogHeader className="p-4 pb-3 border-b shrink-0 m-0">
            <div className="flex items-center justify-between">
              <DialogTitle className="flex items-center gap-2">
                {t('mail.emailViewer')}
              </DialogTitle>
            </div>
          </DialogHeader>
          {/* Force Radix's `display: table` content wrapper back to a block so
              the compact header's truncation works in the drawer too. */}
          <ScrollArea className="flex-1 [&_[data-radix-scroll-area-viewport]>div]:!block">
            <div className='m-5'>
              {currentEnvelope ? (
                <MailMessageView envelope={currentEnvelope} />
              ) : (
                <div className="p-8 text-center text-muted-foreground">{t('mail.noMessageSelected')}</div>
              )}
            </div>
          </ScrollArea>
        </div>
      </DialogContent>
    </Dialog>)
}