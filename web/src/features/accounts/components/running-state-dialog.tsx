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

import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { useQuery } from '@tanstack/react-query'
import { useEffect, useState } from 'react'
import { download_state, gap_fill_state, DownloadStatus, AccountModel, FolderProgress, GapFillRun, GapFillStatus } from '@/api/account/api'
import { format } from 'date-fns'
import { ScrollArea } from '@/components/ui/scroll-area'
import { Badge } from '@/components/ui/badge'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import {
  Accordion,
  AccordionContent,
  AccordionItem,
  AccordionTrigger,
} from "@/components/ui/accordion"
import {
  Clock,
  Loader2,
  Activity,
  AlertTriangle,
  Info,
} from 'lucide-react'
import LongText from '@/components/long-text'
import { useTranslation } from 'react-i18next'

interface Props {
  open: boolean
  onOpenChange: (open: boolean) => void
  currentRow: AccountModel
}

function StatusBadge({ status }: { status: string }) {
  const map: Record<string, string> = {
    Running: 'bg-blue-500/10 text-blue-600',
    Downloading: 'bg-blue-500/10 text-blue-600',
    Success: 'bg-green-500/10 text-green-600',
    Failed: 'bg-red-500/10 text-red-600',
    Cancelled: 'bg-muted text-muted-foreground',
    Pending: 'bg-amber-500/10 text-amber-600',
  }
  return (
    <Badge variant="outline" className={`${map[status] || ''} border-none font-medium text-[11px] px-1.5 h-5 shrink-0`}>
      {status}
    </Badge>
  )
}

function TriggerBadge({ trigger }: { trigger: string }) {
  const isScheduled = trigger === 'Scheduled'
  return (
    <Badge variant="secondary" className={`${isScheduled ? 'bg-purple-500/10 text-purple-600' : 'bg-orange-500/10 text-orange-600'} font-normal text-xs shrink-0 border-none`}>
      {trigger}
    </Badge>
  )
}

function sumFiltered(folders: Record<string, { filtered?: number }> | undefined): number {
  return Object.values(folders || {}).reduce((sum, f) => sum + (f.filtered || 0), 0)
}

function FolderDetailItem({ f, t }: { f: FolderProgress, t: (key: string) => string }) {
  const percentage = Math.min(Math.round((f.current / f.planned) * 100), 100) || 0;

  return (
    <div className="border rounded-xl bg-card overflow-hidden mb-3 shadow-sm">
      <div className="flex flex-col sm:flex-row sm:items-center p-4 gap-3 sm:gap-4">
        <div className="flex-1 min-w-0">
          <LongText className="text-xs font-bold text-foreground tracking-tight">
            {f.folder_name}
          </LongText>
          {(f.filtered || 0) > 0 && (
            <p className="text-[10px] font-medium text-muted-foreground mt-0.5">
              {f.filtered} {t('accounts.runningState.filtered_suffix')}
            </p>
          )}
        </div>
        <div className="flex items-center justify-between sm:justify-end gap-4">
          <div className="flex items-center gap-2 flex-1 sm:flex-initial">
            <div className="flex-1 sm:w-40 lg:w-48 bg-secondary rounded-full h-1.5 overflow-hidden">
              <div
                className="h-full rounded-full transition-all duration-500 bg-primary"
                style={{ width: `${percentage}%` }}
              />
            </div>
            <span className="text-[10px] font-medium text-muted-foreground w-7 shrink-0 text-right">{percentage}%</span>
          </div>
          <div className="flex items-center gap-4 shrink-0">
            <div className="w-20 sm:w-24 text-right font-mono text-xs sm:text-sm text-muted-foreground">
              <span className="font-bold text-foreground">{f.current}</span>
              <span className="mx-0.5 opacity-50">/</span>
              {f.planned}
            </div>
            <div className="w-16 sm:w-20 flex justify-end">
              <StatusBadge status={f.status} />
            </div>
          </div>
        </div>
      </div>

      {f.message && (
        <div className="px-4 pb-4">
          <div className={`border rounded-lg p-3 flex gap-3 items-start ${f.message.toLowerCase().includes('slow') || f.message.toLowerCase().includes('limiting')
            ? 'bg-amber-500/10 border-amber-500/30'
            : 'bg-muted/50'}`}>
            {f.message.toLowerCase().includes('slow') || f.message.toLowerCase().includes('limiting') ? (
              <AlertTriangle className="w-4 h-4 text-amber-600 mt-0.5 shrink-0" />
            ) : (
              <Info className="w-4 h-4 text-muted-foreground mt-0.5 shrink-0" />
            )}
            <div className="space-y-0.5">
              <p className="text-[10px] font-bold text-foreground">{t('accounts.runningState.message')}:</p>
              <p className={`text-[10px] font-medium leading-relaxed ${f.message.toLowerCase().includes('slow') || f.message.toLowerCase().includes('limiting')
                ? 'text-amber-700'
                : 'text-muted-foreground'}`}>{f.message}</p>
            </div>
          </div>
        </div>
      )}
    </div>
  )
}

function GapFillRunDetail({ run, t }: { run: GapFillRun, t: (key: string) => string }) {
  const folderEntries = Object.entries(run.folders)
  if (folderEntries.length === 0) {
    return (
      <div className="text-center py-8 text-muted-foreground italic text-xs">
        {t('accounts.runningState.empty.no_gap_fill_folders')}
      </div>
    )
  }
  const isActive = run.status === GapFillStatus.Running
  return (
    <div className="space-y-3">
      {folderEntries.map(([name, stats]) => {
        // Filtered candidates are finished work even though no body was
        // downloaded, so they count toward the progress bar.
        const processed = stats.downloaded + (stats.filtered || 0)
        const pct = stats.candidate_count > 0
          ? Math.min(100, Math.round((processed / stats.candidate_count) * 100))
          : 0
        return (
          <div key={name} className="py-1.5 border-b border-border/50 last:border-b-0">
            <div className="flex items-center justify-between gap-2">
              <span className="text-xs font-bold text-foreground truncate">{name}</span>
              <span className="text-[10px] font-bold text-muted-foreground whitespace-nowrap">
                {isActive && stats.candidate_count > 0 ? (
                  <span className="text-blue-600">
                    {processed} <span className="opacity-50">/</span> {stats.candidate_count}
                  </span>
                ) : (
                  <>
                    <span className="text-blue-600">{stats.downloaded} {t('accounts.runningState.gap_fill_downloaded_suffix')}</span>
                    {(stats.filtered || 0) > 0 && (
                      <>
                        <span className="mx-1 opacity-30">·</span>
                        <span className="text-muted-foreground">{stats.filtered} {t('accounts.runningState.filtered_suffix')}</span>
                      </>
                    )}
                    {stats.failed > 0 && (
                      <>
                        <span className="mx-1 opacity-30">·</span>
                        <span className="text-destructive">{stats.failed} {t('accounts.runningState.gap_fill_failed_suffix')}</span>
                      </>
                    )}
                  </>
                )}
              </span>
            </div>
            {isActive && stats.candidate_count > 0 && (
              <div className="mt-1.5 h-1.5 w-full rounded-full bg-muted overflow-hidden">
                <div className="h-full rounded-full bg-blue-500 transition-all duration-500" style={{ width: `${pct}%` }} />
              </div>
            )}
            {isActive && (stats.filtered || 0) > 0 && (
              <p className="text-[10px] font-medium text-muted-foreground mt-1">
                {stats.filtered} {t('accounts.runningState.filtered_suffix')}
              </p>
            )}
            {stats.message && (
              <div className="mt-1.5 flex items-start gap-1.5">
                <AlertTriangle className="w-3 h-3 text-amber-600 mt-0.5 shrink-0" />
                <p className="text-[10px] font-medium text-amber-700 leading-relaxed">{stats.message}</p>
              </div>
            )}
          </div>
        )
      })}
    </div>
  )
}

export function RunningStateDialog({ currentRow, open, onOpenChange }: Props) {
  const { t } = useTranslation();

  const { data: state, isLoading } = useQuery({
    queryKey: ['running-state', currentRow.id],
    queryFn: () => download_state(currentRow.id),
    refetchInterval: (query) => {
      const s = query.state.data?.active_session
      return s && s.status === DownloadStatus.Running ? 5000 : false
    },
    enabled: open && !!currentRow.id,
  })

  const { data: gapFillData } = useQuery({
    queryKey: ['gap-fill-state', currentRow.id],
    queryFn: () => gap_fill_state(currentRow.id),
    refetchInterval: (query) => {
      const a = query.state.data?.active
      return a && a.status === GapFillStatus.Running ? 5000 : false
    },
    enabled: open && !!currentRow.id,
  })

  const session = state?.active_session
  const history = state?.history || []
  const isRunning = !!session && session.status === DownloadStatus.Running

  // When the poll is active, tick once per second so the "elapsed" and
  // "last updated Xs ago" readouts stay fresh between refetches.
  const [, setTick] = useState(0)
  useEffect(() => {
    if (!isRunning) return
    const id = setInterval(() => setTick((v) => v + 1), 1000)
    return () => clearInterval(id)
  }, [isRunning])

  const elapsedSec = session && isRunning
    ? Math.max(0, Math.floor((Date.now() - new Date(session.start_time).getTime()) / 1000))
    : 0
  const formatDur = (s: number) => {
    const h = Math.floor(s / 3600)
    const m = Math.floor((s % 3600) / 60)
    const sec = s % 60
    if (h > 0) return `${h}h ${m}m ${sec}s`
    if (m > 0) return `${m}m ${sec}s`
    return `${sec}s`
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent aria-describedby={undefined} className="max-w-5xl w-[95vw] sm:w-full p-0 flex flex-col h-[90vh] sm:h-[85vh] overflow-hidden gap-0 rounded-t-2xl sm:rounded-xl">
        <DialogHeader className="px-4 py-3 sm:px-6 sm:py-4 border-b bg-muted/20 shrink-0">
          <div className="space-y-0.5">
            <DialogTitle className="text-base sm:text-xl font-bold flex items-center gap-2 text-primary truncate">
              <Activity className="w-4 h-4 sm:w-5 sm:h-5 shrink-0" />
              <span className="truncate">{currentRow.email}</span>
            </DialogTitle>
            <p className="text-[9px] sm:text-[11px] text-muted-foreground font-mono uppercase tracking-widest opacity-70">
              {t('accounts.runningState.account.id')}: {currentRow.id}
            </p>
          </div>
        </DialogHeader>

        <Tabs defaultValue="active" className="flex-1 flex flex-col min-h-0">
          <div className="px-4 sm:px-6 border-b bg-card shrink-0 overflow-x-auto no-scrollbar">
            <TabsList className="h-12 w-full justify-start bg-transparent p-0 gap-6 sm:gap-8 flex-nowrap">
              <TabsTrigger value="active" className="whitespace-nowrap data-[state=active]:border-b-2 data-[state=active]:border-primary rounded-none h-full bg-transparent shadow-none px-0 text-xs sm:text-sm font-bold">
                {t('accounts.runningState.tabs.active_session')}
              </TabsTrigger>
              <TabsTrigger value="history" className="whitespace-nowrap data-[state=active]:border-b-2 data-[state=active]:border-primary rounded-none h-full bg-transparent shadow-none px-0 text-xs sm:text-sm font-bold">
                {t('accounts.runningState.tabs.history')}
                <Badge variant="secondary" className="ml-2 h-4 px-1 text-[10px] font-bold">{history.length}</Badge>
              </TabsTrigger>
              <TabsTrigger value="gapfill" className="whitespace-nowrap data-[state=active]:border-b-2 data-[state=active]:border-primary rounded-none h-full bg-transparent shadow-none px-0 text-xs sm:text-sm font-bold">
                {t('accounts.runningState.tabs.gap_fill')}
                {gapFillData?.active && <Badge variant="secondary" className="ml-2 h-4 px-1 text-[10px] font-bold animate-pulse">{t('accounts.runningState.syncing')}</Badge>}
              </TabsTrigger>
            </TabsList>
          </div>

          <div className="flex-1 bg-background min-h-0 overflow-hidden relative">
            {isLoading ? (
              <div className="h-full flex flex-col items-center justify-center">
                <Loader2 className="w-8 h-8 animate-spin text-primary mb-2" />
                <p className="text-sm text-muted-foreground font-medium italic">{t('accounts.runningState.loading.fetching_account_state')}</p>
              </div>
            ) : (
              <>
                <TabsContent value="active" className="h-full m-0 data-[state=active]:flex flex-col">
                  <ScrollArea className="flex-1">
                    <div className="p-4 sm:p-6">
                      {!session ? (
                        <div className="flex flex-col items-center justify-center py-24 text-muted-foreground">
                          <Clock className="w-12 h-12 opacity-10 mb-4" />
                          <p className="italic text-sm">{t('accounts.runningState.empty.no_active_download')}</p>
                        </div>
                      ) : (
                        <div className="space-y-4">
                          <div className="grid grid-cols-1 sm:grid-cols-3 gap-3 mb-2">
                            <div className="p-3 sm:p-4 rounded-xl border bg-card shadow-sm flex items-center justify-between sm:block">
                              <p className="text-[10px] font-bold text-muted-foreground uppercase mb-1">{t('accounts.runningState.session.status')}</p>
                              <StatusBadge status={session.status} />
                            </div>
                            <div className="p-3 sm:p-4 rounded-xl border bg-card shadow-sm flex items-center justify-between sm:block">
                              <p className="text-[10px] font-bold text-muted-foreground uppercase mb-1">{t('accounts.runningState.session.trigger')}</p>
                              <TriggerBadge trigger={session.trigger} />
                            </div>
                            <div className="p-3 sm:p-4 rounded-xl border bg-card shadow-sm flex items-center justify-between sm:block">
                              <p className="text-[10px] font-bold text-muted-foreground uppercase mb-1">{t('accounts.runningState.session.started_at')}</p>
                              <div className="text-sm font-bold font-mono text-foreground">
                                {format(new Date(session.start_time), 'yyyy-MM-dd HH:mm:ss')}
                              </div>
                            </div>
                          </div>
                          {isRunning && (
                            <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
                              <div className="p-3 sm:p-4 rounded-xl border bg-card shadow-sm flex items-center justify-between sm:block">
                                <p className="text-[10px] font-bold text-muted-foreground uppercase mb-1">{t('accounts.runningState.session.elapsed')}</p>
                                <div className="text-sm font-bold font-mono text-blue-600 flex items-center gap-1.5">
                                  <Loader2 className="h-3.5 w-3.5 animate-spin" />
                                  {formatDur(elapsedSec)}
                                </div>
                              </div>
                              {session.current_folder && (
                                <div className="p-3 sm:p-4 rounded-xl border bg-card shadow-sm flex items-center justify-between sm:block">
                                  <p className="text-[10px] font-bold text-muted-foreground uppercase mb-1">{t('accounts.runningState.session.current_folder')}</p>
                                  <div className="text-sm font-bold text-foreground truncate">{session.current_folder}</div>
                                </div>
                              )}
                            </div>
                          )}
                          <Tabs defaultValue="folders" className="w-full">
                            <TabsList className="bg-muted mb-3 h-8">
                              <TabsTrigger value="folders" className="text-[11px] font-bold">
                                {t('accounts.runningState.tabs.folders')}
                              </TabsTrigger>
                              <TabsTrigger
                                value="errors"
                                className="text-[11px] font-bold text-destructive"
                              >
                                {t('accounts.runningState.tabs.errors')} ({session.errors?.length || 0})
                              </TabsTrigger>
                            </TabsList>
                            <TabsContent value="folders" className="space-y-1">
                              {Object.values(session.folder_details).map((f: FolderProgress) => (
                                <FolderDetailItem key={f.folder_name} f={f} t={t} />
                              ))}
                            </TabsContent>
                            <TabsContent value="errors">
                              {(!session.errors || session.errors.length === 0) ? (
                                <div className="text-center py-10 text-muted-foreground italic text-xs">
                                  {t('accounts.runningState.empty.no_errors_current')}
                                </div>
                              ) : (
                                <div className="relative">
                                  <div className="absolute left-[14px] top-1 bottom-1 w-0.5 bg-destructive/20" />
                                  <div className="space-y-4">
                                    {[...session.errors]
                                      .sort((a, b) => new Date(b.at).getTime() - new Date(a.at).getTime())
                                      .map((err, ei) => (
                                        <div key={ei} className="relative pl-8 min-w-0">
                                          <div className="absolute left-0 top-2 w-[28px] flex justify-center">
                                            {ei === 0 ? (
                                              <span className="relative flex h-2.5 w-2.5">
                                                <span className="animate-ping absolute inline-flex h-full w-full rounded-full bg-destructive opacity-75" />
                                                <span className="relative inline-flex rounded-full h-2.5 w-2.5 bg-destructive" />
                                              </span>
                                            ) : (
                                              <div className="w-2 h-2 rounded-full bg-destructive/20" />
                                            )}
                                          </div>
                                          <div
                                            className={`
                                              p-3 border rounded-lg flex flex-col gap-2 min-w-0
                                              ${ei === 0
                                                ? 'bg-destructive/10 border-destructive/20'
                                                : 'bg-card border-destructive/10'}
                                            `}
                                          >
                                            <div className="flex justify-between items-start gap-2 min-w-0">
                                              <div className="flex items-center gap-2 flex-wrap min-w-0">
                                                {ei === 0 && (
                                                  <Badge className="bg-destructive text-[9px] h-4 px-1">
                                                    {t('accounts.runningState.latest')}
                                                  </Badge>
                                                )}
                                                <span className="text-[10px] font-mono text-destructive font-bold break-all">
                                                  {format(new Date(err.at), 'yyyy-MM-dd HH:mm:ss')}
                                                </span>
                                              </div>
                                              <AlertTriangle
                                                className={`w-4 h-4 shrink-0 ${ei === 0 ? 'text-destructive' : 'text-destructive/50'
                                                  }`}
                                              />
                                            </div>
                                            <p className="text-xs font-bold text-foreground whitespace-pre-wrap break-all">
                                              {err.error}
                                            </p>
                                          </div>
                                        </div>
                                      ))}
                                  </div>
                                </div>
                              )}
                            </TabsContent>
                          </Tabs>
                        </div>
                      )}
                    </div>
                  </ScrollArea>
                </TabsContent>
                <TabsContent value="history" className="h-full m-0 data-[state=active]:flex flex-col">
                  <ScrollArea className="flex-1">
                    <div className="p-4 sm:p-6">
                      {history.length === 0 ? (
                        <div className="text-center py-20 text-muted-foreground italic text-sm">{t('accounts.runningState.empty.no_history')}</div>
                      ) : (
                        <Accordion type="single" collapsible className="space-y-3">
                          {[...history].reverse().map((h, i) => (
                            <AccordionItem key={i} value={`history-${i}`} className="border rounded-xl bg-card shadow-sm px-4 border-border overflow-hidden">
                              <AccordionTrigger className="hover:no-underline py-4">
                                <div className="flex flex-col sm:flex-row sm:items-center justify-between w-full pr-4 gap-2">
                                  <div className="flex items-center gap-3">
                                    <div className="text-xs sm:text-xs font-bold font-mono text-foreground">
                                      {format(new Date(h.start_time), 'yyyy-MM-dd HH:mm:ss')}
                                    </div>
                                    <StatusBadge status={h.status} />
                                    <div className="xs:block"><TriggerBadge trigger={h.trigger} /></div>
                                  </div>
                                  <span className="text-[10px] font-bold text-muted-foreground bg-muted px-2 py-0.5 rounded-full self-start sm:self-auto">
                                    {Object.keys(h.folder_details).length} {t('accounts.runningState.folders')}
                                    <span className="mx-1 opacity-30">·</span>
                                    {Object.values(h.folder_details).reduce((sum, f) => sum + (f.current || 0), 0)}
                                    <span className="mx-0.5 opacity-50">/</span>
                                    {Object.values(h.folder_details).reduce((sum, f) => sum + (f.planned || 0), 0)}
                                    {sumFiltered(h.folder_details) > 0 && (
                                      <>
                                        <span className="mx-1 opacity-30">·</span>
                                        {sumFiltered(h.folder_details)} {t('accounts.runningState.filtered_suffix')}
                                      </>
                                    )}
                                  </span>
                                </div>
                              </AccordionTrigger>
                              <AccordionContent className="pb-4 border-t pt-4 mt-1 border-border">
                                <Tabs defaultValue="h-folders" className="w-full">
                                  <TabsList className="bg-muted mb-4 h-8">
                                    <TabsTrigger value="h-folders" className="text-[11px] font-bold">{t('accounts.runningState.tabs.folders')}</TabsTrigger>
                                    <TabsTrigger value="h-errors" className="text-[11px] font-bold text-destructive">
                                      {t('accounts.runningState.tabs.errors')} ({h.errors?.length || 0})
                                    </TabsTrigger>
                                  </TabsList>
                                  <TabsContent value="h-folders" className="space-y-1">
                                    {Object.values(h.folder_details).map((f: any, idx) => (
                                      <FolderDetailItem key={idx} f={f} t={t} />
                                    ))}
                                  </TabsContent>
                                  <TabsContent value="h-errors">
                                    {(!h.errors || h.errors.length === 0) ? (
                                      <div className="text-center py-10 text-muted-foreground italic text-xs">
                                        {t('accounts.runningState.empty.no_errors_session')}
                                      </div>
                                    ) : (
                                      <div className="relative">
                                        <div className="absolute left-[14px] top-1 bottom-1 w-0.5 bg-destructive/20" />
                                        <div className="space-y-4">
                                          {[...h.errors]
                                            .sort((a, b) => new Date(b.at).getTime() - new Date(a.at).getTime())
                                            .map((err, ei) => (
                                              <div key={ei} className="relative pl-8 min-w-0">
                                                <div className="absolute left-0 top-2 w-[28px] flex justify-center">
                                                  {ei === 0 ? (
                                                    <span className="relative flex h-2.5 w-2.5">
                                                      <span className="animate-ping absolute inline-flex h-full w-full rounded-full bg-destructive opacity-75"></span>
                                                      <span className="relative inline-flex rounded-full h-2.5 w-2.5 bg-destructive"></span>
                                                    </span>
                                                  ) : (
                                                    <div className="w-2 h-2 rounded-full bg-destructive/20" />
                                                  )}
                                                </div>
                                                <div
                                                  className={`
                                                    p-3 border rounded-lg flex flex-col gap-2 min-w-0
                                                    ${ei === 0
                                                      ? 'bg-destructive/10 border-destructive/20'
                                                      : 'bg-card border-destructive/10'}
                                                  `}
                                                >
                                                  <div className="flex justify-between items-start gap-2 min-w-0">
                                                    <div className="flex items-center gap-2 flex-wrap min-w-0">
                                                      {ei === 0 && (
                                                        <Badge className="bg-destructive text-[9px] h-4 px-1">
                                                          {t('accounts.runningState.latest')}
                                                        </Badge>
                                                      )}
                                                      <span className="text-[10px] font-mono text-destructive font-bold break-all">
                                                        {format(new Date(err.at), 'yyyy-MM-dd HH:mm:ss')}
                                                      </span>
                                                    </div>
                                                    <AlertTriangle
                                                      className={`w-4 h-4 shrink-0 ${ei === 0 ? 'text-destructive' : 'text-destructive/50'
                                                        }`}
                                                    />
                                                  </div>
                                                  <p className="text-xs font-bold text-foreground whitespace-pre-wrap break-all">
                                                    {err.error}
                                                  </p>
                                                </div>
                                              </div>
                                            ))}
                                        </div>
                                      </div>
                                    )}
                                  </TabsContent>
                                </Tabs>
                              </AccordionContent>
                            </AccordionItem>
                          ))}
                        </Accordion>
                      )}
                    </div>
                  </ScrollArea>
                </TabsContent>
                <TabsContent value="gapfill" className="h-full m-0 data-[state=active]:flex flex-col">
                  <ScrollArea className="flex-1">
                    <div className="p-4 sm:p-6 space-y-4">
                      {gapFillData?.active && (
                        <div className="rounded-xl border bg-card shadow-sm p-4">
                          <div className="flex items-center justify-between mb-3">
                            <p className="text-[10px] font-bold text-muted-foreground uppercase">{t('accounts.runningState.gap_fill_active')}</p>
                            <StatusBadge status={gapFillData.active.status} />
                          </div>
                          <GapFillRunDetail run={gapFillData.active} t={t} />
                        </div>
                      )}
                      {(!gapFillData?.history || gapFillData.history.length === 0) ? (
                        <div className="text-center py-20 text-muted-foreground italic text-sm">
                          {t('accounts.runningState.empty.no_gap_fill_history')}
                        </div>
                      ) : (
                        <Accordion type="single" collapsible className="space-y-3">
                          {[...gapFillData.history].reverse().map((run, i) => (
                            <AccordionItem key={i} value={`gapfill-${i}`} className="border rounded-xl bg-card shadow-sm px-4 border-border overflow-hidden">
                              <AccordionTrigger className="hover:no-underline py-4">
                                <div className="flex flex-col sm:flex-row sm:items-center justify-between w-full pr-4 gap-2">
                                  <div className="flex items-center gap-3">
                                    <div className="text-xs sm:text-xs font-bold font-mono text-foreground">
                                      {format(new Date(run.started_at), 'yyyy-MM-dd HH:mm:ss')}
                                    </div>
                                    <StatusBadge status={run.status} />
                                  </div>
                                  <span className="text-[10px] font-bold text-muted-foreground bg-muted px-2 py-0.5 rounded-full self-start sm:self-auto">
                                    <span className="text-blue-600">{run.downloaded} {t('accounts.runningState.gap_fill_downloaded_suffix')}</span>
                                    {(run.filtered || 0) > 0 && (
                                      <>
                                        <span className="mx-1 opacity-30">·</span>
                                        <span className="text-muted-foreground">{run.filtered} {t('accounts.runningState.filtered_suffix')}</span>
                                      </>
                                    )}
                                    {run.failed > 0 && (
                                      <>
                                        <span className="mx-1 opacity-30">·</span>
                                        <span className="text-destructive">{run.failed} {t('accounts.runningState.gap_fill_failed_suffix')}</span>
                                      </>
                                    )}
                                  </span>
                                </div>
                              </AccordionTrigger>
                              <AccordionContent className="pb-4 border-t pt-4 mt-1 border-border">
                                <GapFillRunDetail run={run} t={t} />
                              </AccordionContent>
                            </AccordionItem>
                          ))}
                        </Accordion>
                      )}
                    </div>
                  </ScrollArea>
                </TabsContent>
              </>
            )}
          </div>
        </Tabs>
        <DialogFooter className="px-4 py-3 sm:px-6 sm:py-4 border-t bg-card shrink-0">
          <DialogClose asChild>
            <Button variant="outline" className="w-full sm:w-24 font-bold border">{t('common.close')}</Button>
          </DialogClose>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}