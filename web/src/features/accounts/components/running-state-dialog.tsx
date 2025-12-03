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

import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { AccountModel } from '../data/schema'
import { useQuery } from '@tanstack/react-query'
import { account_state } from '@/api/account/api'
import { formatDistanceToNow, formatDuration, intervalToDuration } from 'date-fns'
import { ScrollArea } from '@/components/ui/scroll-area'
import { Skeleton } from '@/components/ui/skeleton'
import { CheckCircle, Clock, Loader2, PlayCircle, FolderSync, FolderCheck } from 'lucide-react'
import { FolderSyncProgress } from './folder-sync-progress'
import { useTranslation } from 'react-i18next'
import { dateFnsLocaleMap } from '@/lib/utils'
import { enUS } from 'date-fns/locale'

interface Props {
  open: boolean
  onOpenChange: (open: boolean) => void
  currentRow: AccountModel
}

export function RunningStateDialog({ currentRow, open, onOpenChange }: Props) {
  const { t, i18n } = useTranslation()
  const locale = dateFnsLocaleMap[i18n.language.toLowerCase()] ?? enUS;
  const { data: state, isLoading } = useQuery({
    queryKey: ['running-state', currentRow.id],
    queryFn: () => account_state(currentRow.id),
    retry: 0,
    refetchOnWindowFocus: false,
    refetchInterval: 5000,
  })

  const calculateDuration = (start?: number, end?: number) => {
    if (!start) {
      return (
        <span className="text-yellow-600 flex items-center gap-1">
          <Clock className="w-4 h-4" /> {t('accounts.runningState.notStarted')}
        </span>
      )
    }
    if (!end) {
      return (
        <span className="text-blue-600 flex items-center gap-1">
          <PlayCircle className="w-4 h-4" /> {t('accounts.runningState.inProgress')}
        </span>
      )
    }
    const duration = intervalToDuration({ start: new Date(start), end: new Date(end) })
    return (
      <span className="text-green-600 flex items-center gap-1">
        <CheckCircle className="w-4 h-4" /> {formatDuration(duration, { format: ['hours', 'minutes', 'seconds'] })}
      </span>
    )
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="w-full max-w-7xl sm:rounded-xl p-0 overflow-hidden">
        <DialogHeader className="text-left space-y-2 px-4 sm:px-6 pt-4 sm:pt-6">
          <DialogTitle className="flex flex-wrap items-center gap-2 text-base sm:text-lg">
            <span className="text-blue-500 font-medium truncate">{currentRow.email}</span>
          </DialogTitle>
        </DialogHeader>
        <ScrollArea className="max-h-[85vh] px-4 sm:px-6 pb-6">
          {isLoading && (
            <div className="space-y-4 py-6">
              <Skeleton className="h-6 w-1/2" />
              <Skeleton className="h-6 w-1/3" />
              <div className="flex justify-center items-center py-4">
                <Loader2 className="h-8 w-8 animate-spin text-primary" />
              </div>
            </div>
          )}

          {!isLoading && state && (
            <div className="grid grid-cols-1 xl:grid-cols-3 gap-4 sm:gap-6 mt-4">
              <div className="xl:col-span-2 space-y-1 sm:space-y-1">
                {/* Initial Sync */}
                <div className="p-4 border rounded-lg bg-card">
                  <div className="flex flex-wrap items-center justify-between mb-3 gap-2">
                    <h3 className="text-base sm:text-lg font-semibold flex items-center gap-2">
                      {state.is_initial_sync_completed ? (
                        <FolderCheck className="w-5 h-5 text-green-500" />
                      ) : (
                        <FolderSync className="w-5 h-5 text-blue-500" />
                      )}
                      {t('accounts.runningState.initialSync')}
                    </h3>
                    {state.is_initial_sync_completed ? (
                      <span className="text-xs bg-green-100 text-green-800 px-2 py-1 rounded-full">
                        {t('accounts.runningState.completed')}
                      </span>
                    ) : (
                      <span className="text-xs bg-blue-100 text-blue-800 px-2 py-1 rounded-full">
                        {t('accounts.runningState.inProgress')}
                      </span>
                    )}
                  </div>
                  <div className="grid grid-cols-3 gap-6 w-full">
                    {/* Start Time */}
                    <div className="flex flex-col text-sm">
                      <span className="text-muted-foreground">{t('accounts.runningState.startTime')}</span>
                      <span className="font-medium">
                        {state.initial_sync_start_time ? (
                          <span className="text-green-600">
                            {formatDistanceToNow(new Date(state.initial_sync_start_time), { addSuffix: true, locale })}
                          </span>
                        ) : (
                          <span className="flex items-center gap-1 text-yellow-600">
                            <span className="relative flex h-2 w-2">
                              <span className="animate-ping absolute inline-flex h-full w-full rounded-full bg-yellow-400 opacity-75"></span>
                              <span className="relative inline-flex rounded-full h-2 w-2 bg-yellow-500"></span>
                            </span>
                            {t('accounts.runningState.notStarted')}
                          </span>
                        )}
                      </span>
                    </div>
                    {/* End Time */}
                    <div className="flex flex-col text-sm">
                      <span className="text-muted-foreground">{t('accounts.runningState.endTime')}</span>
                      <span className="font-medium">
                        {state.initial_sync_end_time ? (
                          <span className="text-green-600">
                            {formatDistanceToNow(new Date(state.initial_sync_end_time), { addSuffix: true, locale })}
                          </span>
                        ) : state.initial_sync_start_time ? (
                          <span className="flex items-center gap-1 text-blue-600">
                            {t('accounts.runningState.inProgress')}
                          </span>
                        ) : (
                          <span className="text-yellow-600">{t('accounts.runningState.notStarted')}</span>
                        )}
                      </span>
                    </div>
                    {/* Duration */}
                    <div className="flex flex-col text-sm">
                      <span className="text-muted-foreground">{t('accounts.runningState.duration')}</span>
                      <span className="font-medium">
                        {(() => {
                          if (!state.initial_sync_start_time) return <span className="text-yellow-600">{t('accounts.runningState.notStarted')}</span>
                          if (!state.initial_sync_end_time) return <span className="text-blue-600 animate-pulse">{t('accounts.runningState.calculating')}</span>
                          const diff = new Date(state.initial_sync_end_time).getTime() - new Date(state.initial_sync_start_time).getTime()
                          const h = Math.floor(diff / 3600000)
                          const m = Math.floor((diff % 3600000) / 60000)
                          const s = Math.floor((diff % 60000) / 1000)
                          return <span className="text-foreground">{h}h {m}m {s}s</span>
                        })()}
                      </span>
                    </div>
                  </div>
                </div>

                <div className="p-4 border rounded-lg bg-card">
                  {state && (
                    <div>
                      <h3 className="text-base sm:text-lg font-semibold mb-3">{t('accounts.runningState.syncProgress')}</h3>
                      <ScrollArea className="h-70 sm:h-70 border rounded-md p-2">
                        <FolderSyncProgress progressMap={state.progress} />
                      </ScrollArea>
                    </div>
                  )}
                </div>

                {/* Incremental Sync */}
                <div className="p-4 border rounded-lg bg-card">
                  <h3 className="text-base sm:text-lg font-semibold mb-3">{t('accounts.runningState.incrementalSync')}</h3>
                  <div className="grid grid-cols-1 sm:grid-cols-2 gap-4">
                    <div className="space-y-3 text-sm">
                      <div className="flex justify-between">
                        <span className="text-muted-foreground">{t('accounts.runningState.startTime')}</span>
                        <span className="font-medium">
                          {state.last_incremental_sync_start ? (
                            formatDistanceToNow(new Date(state.last_incremental_sync_start), { addSuffix: true, locale })
                          ) : (
                            <span className="text-yellow-600">{t('accounts.runningState.notStarted')}</span>
                          )}
                        </span>
                      </div>
                      <div className="flex justify-between">
                        <span className="text-muted-foreground">{t('accounts.runningState.endTime')}</span>
                        <span className="font-medium">
                          {state.last_incremental_sync_end ? (
                            formatDistanceToNow(new Date(state.last_incremental_sync_end), { addSuffix: true, locale })
                          ) : state.last_incremental_sync_start ? (
                            <span className="text-blue-600">{t('accounts.runningState.inProgress')}</span>
                          ) : (
                            <span className="text-yellow-600">{t('accounts.runningState.notStarted')}</span>
                          )}
                        </span>
                      </div>
                    </div>
                    <div className="space-y-3 text-sm">
                      <div className="flex justify-between">
                        <span className="text-muted-foreground">{t('accounts.runningState.duration')}</span>
                        <span className="font-medium">
                          {calculateDuration(state.last_incremental_sync_start, state.last_incremental_sync_end)}
                        </span>
                      </div>
                      <div className="flex justify-between">
                        <span className="text-muted-foreground">{t('accounts.runningState.syncInterval')}</span>
                        <span className="font-medium">
                          {t('accounts.runningState.everyMinutes', { minutes: currentRow.sync_interval_min })}
                        </span>
                      </div>
                    </div>
                  </div>
                </div>
              </div>

              <div className="p-4 border rounded-lg bg-card">
                <h3 className="text-base sm:text-lg font-semibold mb-3">{t('accounts.runningState.errorLogs')}</h3>
                <ScrollArea className="h-[20rem] sm:h-[32rem]">
                  <div className="space-y-3">
                    {state.errors.length ? (
                      state.errors
                        .sort((a, b) => b.at - a.at)
                        .map((item, index) => (
                          <div
                            key={index}
                            className="flex flex-col items-start gap-2 rounded-lg border p-3 text-left text-xs sm:text-sm transition-all hover:bg-accent"
                          >
                            <div className="flex w-full flex-col gap-1">
                              <div className="text-xs font-medium text-muted-foreground">
                                {formatDistanceToNow(new Date(item.at), { addSuffix: true, locale })}
                              </div>
                              <div className="font-medium break-words">{item.error}</div>
                            </div>
                          </div>
                        ))
                    ) : (
                      <div className="h-full flex justify-center items-center py-8">
                        <p className="text-sm text-muted-foreground">{t('accounts.runningState.noErrorLogs')}</p>
                      </div>
                    )}
                  </div>
                </ScrollArea>
              </div>
            </div>
          )}

          {!isLoading && !state && (
            <div className="h-full flex flex-col justify-center items-center py-8 text-center space-y-2">
              <p className="text-sm text-muted-foreground">{t('accounts.runningState.noActiveState')}</p>
              <p className="text-xs text-muted-foreground">{t('accounts.runningState.accountNotStarted')}</p>
            </div>
          )}
        </ScrollArea>

        <DialogFooter className="pt-4 px-4 sm:px-6 pb-4 border-t">
          <DialogClose asChild>
            <Button variant="outline" className="w-full sm:w-auto">{t('accounts.runningState.close')}</Button>
          </DialogClose>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
