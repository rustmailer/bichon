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

import { useState } from 'react';
import { useInfiniteQuery } from '@tanstack/react-query';
import { Loader2, MessageSquareText } from 'lucide-react';

import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import { Button } from '@/components/ui/button';
import { Badge } from '@/components/ui/badge';
import { Card, CardContent, CardHeader } from '@/components/ui/card';
import { ScrollArea } from '@/components/ui/scroll-area';
import { Skeleton } from '@/components/ui/skeleton';
import { get_thread_messages } from '@/api/mailbox/envelope/api';
import { MailMessageView } from './mail-message-view';
import { useSearchContext } from './context';
import { useTranslation } from 'react-i18next'
import { formatSender } from '@/lib/utils';
import { format } from 'date-fns';

interface MailThreadDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

export function MailThreadDialog({ open, onOpenChange }: MailThreadDialogProps) {
  const { currentEnvelope } = useSearchContext();
  const [expandedIds, setExpandedIds] = useState<Set<string>>(new Set());
  const { t } = useTranslation();

  const threadId = currentEnvelope?.thread_id;
  const accountId = currentEnvelope?.account_id;

  const {
    data,
    fetchNextPage,
    hasNextPage,
    isFetchingNextPage,
    isLoading,
    isError,
    error,
  } = useInfiniteQuery({
    queryKey: ['thread', accountId, threadId],
    queryFn: ({ pageParam = 1 }) =>
      get_thread_messages(accountId!, threadId!, pageParam, 10),
    getNextPageParam: (lastPage) =>
      lastPage.current_page && lastPage.total_pages
        ? lastPage.current_page < lastPage.total_pages
          ? lastPage.current_page + 1
          : undefined
        : undefined,
    enabled: open && !!accountId && !!threadId,
    initialPageParam: 1,
  });

  const allMessages = data?.pages.flatMap((page) => page.items) ?? [];
  const totalCount = data?.pages[0]?.total_items ?? 0;
  const sortedMessages = [...allMessages].sort((a, b) => a.date - b.date);

  const toggleExpand = (id: string) => {
    setExpandedIds((prev) => {
      const next = new Set(prev);
      next.has(id) ? next.delete(id) : next.add(id);
      return next;
    });
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent aria-describedby={undefined} className="w-full max-width-full p-0 max-h-full flex flex-col md:max-w-3xl lg:max-w-4xl">
        <DialogHeader className="p-4 pb-3 border-b shrink-0">
          <DialogTitle className="flex items-center gap-2">
            <MessageSquareText className="w-5 h-5" />
            <span className="text-sm">
              {t('search.thread.title', { count: totalCount })}
            </span>
          </DialogTitle>
        </DialogHeader>

        <ScrollArea className="h-[calc(100vh-260px)] w-full pr-4 -mr-4 py-1">
          <div className="p-4 sm:p-6">
            {isLoading && <ThreadSkeleton />}

            {isError && (
              <div className="text-center text-destructive text-sm">
                {t('search.thread.error')}: {(error as Error)?.message}
              </div>
            )}

            {!isLoading && sortedMessages.length === 0 && (
              <div className="text-center text-muted-foreground text-sm">
                {t('search.thread.empty')}
              </div>
            )}

            {sortedMessages.length > 0 && (
              <div className="relative">
                <div className="absolute left-[19px] top-2 bottom-2 w-0.5 bg-destructive/20" />
                <div className="absolute left-[15px] bottom-0 w-0 h-0 border-l-[5px] border-r-[5px] border-t-[7px] border-l-transparent border-r-transparent border-t-destructive/40" />

                <div className="space-y-6">
                  {sortedMessages.map((msg, i) => {
                    const isExpanded = expandedIds.has(msg.id);
                    const isLatest = i === sortedMessages.length - 1;

                    const date = new Date(msg.date);
                    const formattedDate = isNaN(date.getTime())
                      ? t('search.thread.invalidDate')
                      : format(date, 'yyyy-MM-dd HH:mm:ss');

                    return (
                      <div key={msg.id} className={`relative pl-10 min-w-0 ${isLatest ? 'mt-6' : ''}`}>
                        <div className="absolute left-0 top-1.5 w-[40px] flex flex-col items-center gap-1">
                          {isLatest && (
                            <Badge className="bg-primary hover:bg-primary text-[9px] h-4 px-1 shrink-0 w-fit">
                              {t('search.thread.latest')}
                            </Badge>
                          )}
                          {isLatest ? (
                            <span className="relative flex h-3 w-3">
                              <span className="animate-ping absolute inline-flex h-full w-full rounded-full bg-primary opacity-75" />
                              <span className="relative inline-flex rounded-full h-3 w-3 bg-primary" />
                            </span>
                          ) : (
                            <div className="w-2.5 h-2.5 rounded-full bg-muted-foreground/30 mt-0.5" />
                          )}
                        </div>
                        <Card
                          className={`transition-all min-w-0 shadow-sm ${isLatest
                            ? 'border-primary/20 bg-primary/5 ring-1 ring-primary/10'
                            : 'border-border bg-card'
                            } ${isExpanded ? 'ring-2 ring-primary' : ''}`}
                        >
                          <CardHeader
                            className="cursor-pointer pb-3"
                            onClick={() => toggleExpand(msg.id)}
                          >
                            <div className="flex items-start justify-between gap-3 mb-1 min-w-0">
                              <div className="flex items-center gap-1.5 flex-wrap min-w-0">
                                <span className="font-medium truncate text-sm">{formatSender(msg.from_name, msg.from)}</span>
                                <span className="text-muted-foreground text-sm">→</span>
                                <span className="text-muted-foreground truncate text-sm">
                                  {msg.to.join(', ')}
                                </span>
                              </div>
                              <div className="text-[10px] font-mono font-bold text-muted-foreground bg-muted px-1.5 py-0.5 rounded shrink-0">
                                <span className="sm:hidden">
                                  {isNaN(date.getTime()) ? '' : format(date, 'HH:mm')}
                                </span>
                                <span className="hidden sm:inline">{formattedDate}</span>
                              </div>
                            </div>

                            <p className="font-medium text-sm">
                              {msg.subject || t('search.thread.noSubject')}
                            </p>

                            {!isExpanded && msg.preview && (
                              <p className="text-xs text-muted-foreground mt-1 line-clamp-2">
                                {msg.preview}
                              </p>
                            )}
                          </CardHeader>

                          {isExpanded && (
                            <CardContent className="p-0">
                              <div className="h-96 border-t m-5">
                                <MailMessageView
                                  envelope={msg}
                                  showActions={false}
                                  showAttachments={false}
                                  showHeader={false}
                                />
                              </div>
                            </CardContent>
                          )}
                        </Card>
                      </div>
                    );
                  })}
                </div>
              </div>
            )}

            {hasNextPage && (
              <div className="flex justify-center py-3 mt-4">
                <Button
                  onClick={() => fetchNextPage()}
                  disabled={isFetchingNextPage}
                  variant="outline"
                  size="sm"
                >
                  {isFetchingNextPage ? (
                    <>
                      <Loader2 className="w-4 h-4 mr-2 animate-spin" />
                      {t('search.thread.loadingMore')}
                    </>
                  ) : (
                    t('search.thread.loadMore')
                  )}
                </Button>
              </div>
            )}
          </div>
        </ScrollArea>
      </DialogContent>
    </Dialog>
  );
}

function ThreadSkeleton() {
  return (
    <div className="relative">
      <div className="absolute left-[19px] top-2 bottom-2 w-0.5 bg-destructive/20" />
      <div className="absolute left-[15px] bottom-0 w-0 h-0 border-l-[5px] border-r-[5px] border-t-[7px] border-l-transparent border-r-transparent border-t-destructive/40" />
      <div className="space-y-6">
        {[...Array(3)].map((_, i) => (
          <div key={i} className="relative pl-10 min-w-0">
            <div className="absolute left-0 top-1.5 w-[40px] flex justify-center">
              <div className="w-2.5 h-2.5 rounded-full bg-muted-foreground/30 mt-0.5" />
            </div>
            <Card>
              <CardHeader>
                <Skeleton className="h-4 w-48 mb-2" />
                <Skeleton className="h-5 w-64 mb-1" />
                <Skeleton className="h-4 w-full" />
                <Skeleton className="h-4 w-32 mt-2" />
              </CardHeader>
            </Card>
          </div>
        ))}
      </div>
    </div>
  );
}