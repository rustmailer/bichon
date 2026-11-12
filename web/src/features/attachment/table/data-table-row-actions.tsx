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


import { Row } from '@tanstack/react-table'
import { useState } from 'react'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuShortcut,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { useTranslation } from 'react-i18next'
import { Copy, Download, Eye, MoreVertical, Wrench } from 'lucide-react'
import { AttachmentModel } from '@/api/attachment/api'
import { useSearchAttachments } from '@/hooks/use-search-attachments'
import { useToast } from '@/hooks/use-toast'
import { useMutation } from '@tanstack/react-query'
import { download_attachment } from '@/api/mailbox/envelope/api'
import AttachmentPreview from '@/features/attachment/attachment-preview'
import { useAttachmentContext } from '@/features/attachment/context'
import { useCurrentUser } from '@/hooks/use-current-user'

interface DataTableRowActionsProps {
  row: Row<AttachmentModel>
}

export function DataTableRowActions({ row }: DataTableRowActionsProps) {
  const { setFilter } = useSearchAttachments();
  const { setCurrentAttachment, setOpen } = useAttachmentContext();
  const { t } = useTranslation()
  const { toast } = useToast();
  const [previewOpen, setPreviewOpen] = useState(false);
  const { require_any_permission } = useCurrentUser()

  const canRepair = require_any_permission(
    ['system:root', 'data:manage', 'data:manage:all'],
    row.original.account_id
  )

  const downloadMutation = useMutation({
    mutationFn: (content_hash: string) =>
      download_attachment(
        row.original.account_id,
        row.original.envelope_id,
        content_hash,
        row.original.name ?? row.original.id
      ),
    onError: (error: any) => {
      toast({
        title: t('mail.failedToDownloadFile'),
        description: error.message,
        variant: 'destructive',
      });
    },
  });

  return (
    <>
      <DropdownMenu modal={false}>
        <DropdownMenuTrigger asChild>
          <Button
            variant='ghost'
            className='flex h-8 w-8 p-0 data-[state=open]:bg-muted'
          >
            <MoreVertical size={10} />
            <span className='sr-only'>Open menu</span>
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align='end' className='w-[180px]'>
          <DropdownMenuItem
            className='text-xs'
            onClick={(e) => {
              e.stopPropagation()
              setFilter((prev: any) => ({ ...prev, content_hash: row.original.content_hash }));
            }}
          >
            {t('attachment.showDuplicates')}
            <DropdownMenuShortcut>
              <Copy size={16} />
            </DropdownMenuShortcut>
          </DropdownMenuItem>
          <DropdownMenuSeparator />
          <DropdownMenuItem
            className='text-xs'
            onClick={(e) => {
              e.stopPropagation();
              setPreviewOpen(true);
            }}
          >
            {t('attachment.preview')}
            <DropdownMenuShortcut>
              <Eye size={16} />
            </DropdownMenuShortcut>
          </DropdownMenuItem>
          <DropdownMenuItem
            className='text-xs'
            disabled={downloadMutation.isPending}
            onClick={(e) => {
              e.stopPropagation()
              e.preventDefault();
              downloadMutation.mutate(row.original.content_hash);
            }}
          >
            {downloadMutation.isPending
              ? t('attachment.downloading')
              : t('attachment.download')}
            <DropdownMenuShortcut>
              <Download size={16} />
            </DropdownMenuShortcut>
          </DropdownMenuItem>
          {canRepair && (
            <>
              <DropdownMenuSeparator />
              <DropdownMenuItem
                className='text-xs'
                onClick={(e) => {
                  e.stopPropagation()
                  // Repairs the whole message, not just this attachment.
                  setCurrentAttachment(row.original)
                  setOpen('repair')
                }}
              >
                {t('repair_message.repairAction', 'Repair message')}
                <DropdownMenuShortcut>
                  <Wrench size={16} />
                </DropdownMenuShortcut>
              </DropdownMenuItem>
            </>
          )}
        </DropdownMenuContent>
      </DropdownMenu>
      <AttachmentPreview
        open={previewOpen}
        onOpenChange={setPreviewOpen}
        accountId={row.original.account_id}
        envelopeId={row.original.envelope_id}
        contentHash={row.original.content_hash}
        contentType={row.original.content_type}
        fileName={row.original.name ?? row.original.id}
      />
    </>
  )
}
