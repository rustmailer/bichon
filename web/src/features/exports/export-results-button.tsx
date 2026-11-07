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
import React from 'react'
import { Download } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import type { ExportRequest } from '@/api/export/api'
import { Button } from '@/components/ui/button'
import { ExportDialog } from './export-dialog'

interface ExportResultsButtonProps {
  /** Attachment searches export a zip of attachments; email searches mbox. */
  kind: 'attachment' | 'email'
  /** The page's current filter, sent as-is as the ad-hoc selection. */
  filter: Record<string, unknown>
}

/**
 * "Download what I am looking at" — exports the current filter's results
 * without requiring a saved search first.
 *
 * The selection is the filter itself, so it is as generic as the search page:
 * `attachment_category: image` downloads every image, `attachment_extension:
 * pdf` (or any other filter combination) every matching PDF.
 *
 * With no filter there is nothing being "looked at": the selection would be
 * the whole archive, which is exactly the action a batch export must not
 * become one toolbar button away from. The button therefore does not render
 * until the page narrows the result set — the same rule the saved-search
 * export always had. Exporting everything stays possible by saving a search
 * first (or through the API), i.e. as a deliberate, named action.
 */
export function ExportResultsButton({
  kind,
  filter,
}: ExportResultsButtonProps) {
  const { t } = useTranslation()
  const [request, setRequest] = React.useState<ExportRequest | null>(null)
  const attachments = kind === 'attachment'

  if (Object.keys(filter).length === 0) {
    return null
  }

  const open = () => {
    setRequest({
      filter: attachments ? { attachment: filter } : { email: filter },
      label: t('exports.currentFilterLabel'),
      format: attachments ? 'Attachments' : 'Mbox',
    })
  }

  return (
    <>
      <Button
        variant='ghost'
        size='sm'
        className='h-7 px-2 text-xs gap-1.5 font-medium rounded-md text-foreground/70 hover:text-foreground hover:bg-accent transition-all duration-200'
        title={t('exports.downloadAll')}
        onClick={open}
      >
        <Download className='h-4 w-4' />
        <span>{t('exports.downloadAll')}</span>
      </Button>
      <ExportDialog
        request={request}
        title={
          attachments ? t('exports.titleAttachments') : t('exports.title')
        }
        onClose={() => setRequest(null)}
      />
    </>
  )
}
