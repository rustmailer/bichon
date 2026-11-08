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
import { Bookmark, Download, Ellipsis, Trash2, Wrench } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import type { SavedSearchKind } from '@/api/saved-search/api'
import type { ExportRequest } from '@/api/export/api'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { ExportDialog } from '@/features/exports/export-dialog'
import {
  DeletionHistoryDrawer,
  RepairHistoryDrawer,
} from '@/features/history/history-drawers'
import { SavedSearchesDialog } from '@/features/saved-searches/saved-searches-dropdown'
import { useCurrentUser } from '@/hooks/use-current-user'

interface SearchActionsMenuProps {
  kind: SavedSearchKind
  /** The page's current filter, used by both the save and download actions. */
  filter: Record<string, unknown>
  onApply: (filter: Record<string, unknown>) => void
}

/**
 * The toolbar's secondary actions in one place: saved searches, "download
 * what I am looking at", and the operation history (deletion jobs and repair
 * reports). They used to sit as three separate icon buttons with duplicate
 * semantics; a menu keeps the search toolbar to the actions used constantly.
 */
export function SearchActionsMenu({
  kind,
  filter,
  onApply,
}: SearchActionsMenuProps) {
  const { t } = useTranslation()
  const { require_any_permission } = useCurrentUser()
  const [savedOpen, setSavedOpen] = useState(false)
  const [deletionsOpen, setDeletionsOpen] = useState(false)
  const [repairsOpen, setRepairsOpen] = useState(false)
  const [exportRequest, setExportRequest] = useState<ExportRequest | null>(null)
  // Repairs are an operator action; deletion jobs are visible to their creator.
  const canSeeRepairs = require_any_permission(['system:root', 'data:manage'])

  const attachments = kind === 'Attachment'
  const hasFilter = Object.keys(filter).length > 0

  const openExport = () => {
    setExportRequest({
      filter: attachments ? { attachment: filter } : { email: filter },
      label: t('exports.currentFilterLabel'),
      format: attachments ? 'Attachments' : 'Mbox',
    })
  }

  return (
    <>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button
            variant='outline'
            size='sm'
            className='h-6 gap-1 px-2 text-xs rounded-none'
          >
            <Ellipsis className='h-3.5 w-3.5' />
            {t('common.actions', 'Actions')}
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align='end' className='w-52 text-xs'>
          <DropdownMenuItem
            className='text-xs'
            onSelect={() => setSavedOpen(true)}
          >
            <Bookmark className='mr-2 h-4 w-4' />
            {t('saved_searches.button')}
          </DropdownMenuItem>
          {/* Same rule as the old button: with no filter there is no
              "current results" to download, only the whole archive. */}
          {hasFilter && (
            <DropdownMenuItem className='text-xs' onSelect={openExport}>
              <Download className='mr-2 h-4 w-4' />
              {t('exports.downloadAll')}
            </DropdownMenuItem>
          )}
          <DropdownMenuSeparator />
          <DropdownMenuItem
            className='text-xs'
            onSelect={() => setDeletionsOpen(true)}
          >
            <Trash2 className='mr-2 h-4 w-4' />
            {t('search.batchDelete.history', 'Deletion history')}
          </DropdownMenuItem>
          {canSeeRepairs && (
            <DropdownMenuItem
              className='text-xs'
              onSelect={() => setRepairsOpen(true)}
            >
              <Wrench className='mr-2 h-4 w-4' />
              {t('repair_message.history', 'Repair history')}
            </DropdownMenuItem>
          )}
        </DropdownMenuContent>
      </DropdownMenu>

      <SavedSearchesDialog
        open={savedOpen}
        onOpenChange={setSavedOpen}
        kind={kind}
        filter={filter}
        onApply={onApply}
      />
      <ExportDialog
        request={exportRequest}
        title={
          attachments
            ? t('exports.titleAttachments')
            : t('exports.title')
        }
        onClose={() => setExportRequest(null)}
      />
      <DeletionHistoryDrawer
        open={deletionsOpen}
        onOpenChange={setDeletionsOpen}
      />
      {canSeeRepairs && (
        <RepairHistoryDrawer open={repairsOpen} onOpenChange={setRepairsOpen} />
      )}
    </>
  )
}
