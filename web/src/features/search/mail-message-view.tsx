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


import { useEffect, useRef, useState } from 'react';
import { useMutation } from '@tanstack/react-query';
import { Loader, Download, Trash2, MessageSquareMore, FileText, FileImage, FileVideo, FileArchive, FileSpreadsheet, FileCode, FileIcon, FileAudio, Upload, ShieldCheck, Pencil, Copy, ChevronDown, Wrench, ExternalLink } from 'lucide-react';
import { Link } from '@tanstack/react-router';

import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Separator } from '@/components/ui/separator';
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip';
import { toast } from '@/hooks/use-toast';
import { cn, formatBytes, formatSender } from '@/lib/utils';
import { copyToClipboard } from '@/lib/clipboard';
import EmailIframe from '@/components/mail-iframe';
import {
  AttachmentInfo,
  download_attachment,
  download_message,
  getContent,
  load_message,
} from '@/api/mailbox/envelope/api';
import { AxiosError } from 'axios';
import { useSearchContext } from './context';
import { MailThreadDialog } from './thread-dialog';
import useMinimalAccountList from '@/hooks/use-minimal-account-list';
import { useTranslation } from 'react-i18next';
import { useCurrentUser } from '@/hooks/use-current-user';
import { NestedEmailDialog } from './nested-email-dialog';
import AttachmentPreview, { type PreviewAttachment } from '@/features/attachment/attachment-preview';


interface MailMessageViewProps {
  envelope: {
    id: string;
    account_id: number,
    from?: string;
    from_name?: string;
    wire_from?: string;
    to?: string[];
    cc?: string[];
    bcc?: string[];
    subject?: string;
    internal_date?: number;
    tags: string[];
  };
  showActions?: boolean;
  showHeader?: boolean;
  showAttachments?: boolean;
}

const Multilines: React.FC<{ title: string; lines: string[] }> = ({ title, lines }) => {
  const { t } = useTranslation()
  const [expanded, setExpanded] = useState(false);
  return (
    <div className="text-xs">
      <div className="flex items-start space-x-2">
        <span className="font-medium text-gray-400 whitespace-nowrap">{title}:</span>
        <div className="flex-1">
          <ul className="list-disc list-inside">
            {lines.slice(0, expanded ? lines.length : 3).map((ref, i) => (
              <li key={i} className="line-clamp-1">{ref}</li>
            ))}
          </ul>
          {lines.length > 3 && (
            <button
              className="text-blue-500 hover:underline text-xs"
              onClick={() => setExpanded(!expanded)}
            >
              {expanded ? t('common.showLess') : t('common.showMore')}
            </button>
          )}
        </div>
      </div>
    </div>
  );
};

export const getFileConfig = (mimeType: string) => {
  const type = mimeType.toLowerCase();
  if (type.includes('pdf')) {
    return { icon: <FileText className="h-4 w-4" />, color: 'text-red-600 bg-red-50 border-red-100' };
  }
  if (type.includes('image/')) {
    return { icon: <FileImage className="h-4 w-4" />, color: 'text-blue-600 bg-blue-50 border-blue-100' };
  }
  if (type.includes('audio/')) {
    return { icon: <FileAudio className="h-4 w-4" />, color: 'text-purple-600 bg-purple-50 border-purple-100' };
  }

  if (type.includes('video/')) {
    return { icon: <FileVideo className="h-4 w-4" />, color: 'text-indigo-600 bg-indigo-50 border-indigo-100' };
  }
  if (type.includes('spreadsheet') || type.includes('excel') || type.includes('csv')) {
    return { icon: <FileSpreadsheet className="h-4 w-4" />, color: 'text-green-600 bg-green-50 border-green-100' };
  }
  if (type.includes('zip') || type.includes('compressed') || type.includes('archive')) {
    return { icon: <FileArchive className="h-4 w-4" />, color: 'text-orange-600 bg-orange-50 border-orange-100' };
  }
  if (type.includes('text/') || type.includes('json') || type.includes('javascript')) {
    return { icon: <FileCode className="h-4 w-4" />, color: 'text-slate-600 bg-slate-50 border-slate-100' };
  }

  return { icon: <FileIcon className="h-4 w-4" />, color: 'text-gray-600 bg-gray-50 border-gray-100' };
};

export function MailMessageView({
  envelope,
  showActions = true,
  showAttachments = true,
  showHeader = true
}: MailMessageViewProps) {
  const { t } = useTranslation()
  const { setToDelete, setOpen, setSelected, setEditTagsOpen } = useSearchContext();
  // Actions are gated by the permission the server demands, on this message's
  // own account. Missing ones stay visible but disabled, with the reason in
  // the tooltip.
  const { require_any_permission } = useCurrentUser();
  const canEditTags = require_any_permission(
    ['system:root', 'data:manage'],
    envelope.account_id
  );
  const canDelete = require_any_permission(
    ['system:root', 'data:delete', 'data:delete:all'],
    envelope.account_id
  );
  const canDownload = require_any_permission(
    ['system:root', 'data:raw:download', 'data:raw:download:all'],
    envelope.account_id
  );
  const canThread = require_any_permission(
    ['system:root', 'data:read', 'data:read:all'],
    envelope.account_id
  );
  const canRestore = require_any_permission(
    ['system:root', 'data:export:batch', 'data:export:batch:all'],
    envelope.account_id
  );
  // Repair is non-destructive (hash-verified re-fetch), so it uses the same
  // permission as metadata management rather than the delete/restore gates.
  const canRepair = require_any_permission(
    ['system:root', 'data:manage'],
    envelope.account_id
  );
  const noPermission = t('common.noPermission', 'No permission');
  const [content, setContent] = useState<string | null>(null);
  const [contentType, setContentType] = useState<'Plain' | 'Html' | null>(null);
  const [attachments, setAttachments] = useState<AttachmentInfo[] | null>(null);
  const [loading, setLoading] = useState(true);
  const [downloadingAttachmentFileName, setDownloadingAttachmentFileName] = useState<string | null>(null);
  const [nestedEmlFile, setNestedEmlFile] = useState<AttachmentInfo | null>(null);
  const { getEmailById } = useMinimalAccountList();
  const [threadOpen, setThreadOpen] = useState(false);
  const [blockRemote, setBlockRemote] = useState(true);
  // Header details (account, id, recipients, tags) are collapsed by default
  // so the message body gets the vertical space.
  const [detailsOpen, setDetailsOpen] = useState(false);

  useEffect(() => {
    setDetailsOpen(false);
  }, [envelope.id]);
  const [hasRemoteContent, setHasRemoteContent] = useState(false);
  const [previewAttachment, setPreviewAttachment] = useState<{ attachments: PreviewAttachment[]; index: number } | null>(null);

  const toggleBlockRemote = () => {
    setBlockRemote((prev) => !prev);
  };

  const downloadAttachmentMutation = useMutation({
    mutationFn: ({ content_hash }: { content_hash: string }) =>
      download_attachment(envelope.account_id, envelope.id, content_hash, downloadingAttachmentFileName!),
    onSuccess: () => setDownloadingAttachmentFileName(null),
    onError: (error: Error) => {
      setDownloadingAttachmentFileName(null);
      toast({
        title: t('mail.failedToDownloadFile'),
        description: error.message,
        variant: 'destructive',
      });
    },
  });

  const loadMessageMutation = useMutation({
    mutationFn: () => load_message(envelope.account_id, envelope.id, blockRemote),
    onSuccess: (data) => {
      setLoading(false);
      setContent(getContent(data));
      if (data.attachments) setAttachments(data.attachments);
      setContentType(data.html ? 'Html' : 'Plain');
      setHasRemoteContent(!!data.has_remote_content);
    },
    onError: (error: Error) => {
      setLoading(false);
      toast({
        title: t('mail.failedToLoadEmail'),
        description: error.message,
        variant: 'destructive',
      });
    },
  });

  useEffect(() => {
    setBlockRemote(true);
  }, [envelope.id]);

  const loadedKeyRef = useRef('');
  useEffect(() => {
    const key = `${envelope.account_id}:${envelope.id}:${blockRemote}`;
    if (loadedKeyRef.current === key) return;
    loadedKeyRef.current = key;
    setLoading(true);
    loadMessageMutation.mutate();
  }, [envelope.id, blockRemote]);


  const handleViewNestedEml = (attachment: AttachmentInfo) => {
    setNestedEmlFile(attachment);
  };

  const toggleToDelete = (accountId: number, mailId: string) => {
    setToDelete(prev => {
      const next = new Map(prev);
      const set = new Set(next.get(accountId) || []);

      if (set.has(mailId)) {
        set.delete(mailId);
        if (set.size === 0) next.delete(accountId);
        else next.set(accountId, set);
      } else {
        set.add(mailId);
        next.set(accountId, set);
      }

      return next;
    });
  };

  const handleDelete = () => {
    if (envelope) {
      toggleToDelete(envelope.account_id, envelope.id)
      setOpen("delete")
    }
  }


  const downloadEmlFile = async () => {
    try {
      toast({ title: t('mail.downloadStarted'), description: t('mail.isBeingDownloaded', { id: envelope.id }) });
      await download_message(envelope.account_id, envelope.id);
      toast({ title: t('mail.downloadComplete'), description: t('mail.downloaded', { id: envelope.id }) });
    } catch (error) {
      let msg = t('mail.downloadFailed');
      if (error instanceof AxiosError) {
        msg = error.response?.data?.message || error.response?.data?.error || error.message;
        if (error.response?.status) msg = `${error.response.status}: ${msg}`;
      } else if (error instanceof Error) {
        msg = error.message;
      }
      toast({ title: t('mail.downloadFailed'), description: msg, variant: 'destructive' });
    }
  };

  // The id is what the "Advanced" popover searches on, so the header is where
  // a user picks it up — the row is a dense one-liner, hence the icon button.
  const copyEnvelopeId = async () => {
    try {
      await copyToClipboard(envelope.id);
      toast({ title: t('mail.copied') });
    } catch {
      toast({ title: t('mail.copyFailed'), variant: 'destructive' });
    }
  };

  // Inline attachments are part of the rendered body; only regular ones get a
  // row. When there are none, the whole section is omitted so the message body
  // gets the space instead of an empty "No attachments" line.
  const nonInlineAttachments = (attachments ?? []).filter(
    (attachment) => !attachment.inline
  )

  const actionButtons = (
    <>
      <Tooltip delayDuration={800}>
        <TooltipTrigger asChild>
          <Button
            variant="ghost"
            size="icon"
            onClick={handleDelete}
            disabled={!canDelete}
            className="h-6 w-6 hover:text-destructive"
          >
            <Trash2 className="h-3.5 w-3.5" />
          </Button>
        </TooltipTrigger>
        <TooltipContent>
          {canDelete ? t('mail.delete') : noPermission}
        </TooltipContent>
      </Tooltip>
      <Tooltip delayDuration={800}>
        <TooltipTrigger asChild>
          <Button
            variant="ghost"
            size="icon"
            onClick={downloadEmlFile}
            disabled={!canDownload}
            className="h-6 w-6"
          >
            <Download className="h-3.5 w-3.5" />
          </Button>
        </TooltipTrigger>
        <TooltipContent>
          {canDownload ? t('mail.download') : noPermission}
        </TooltipContent>
      </Tooltip>
      <Tooltip delayDuration={800}>
        <TooltipTrigger asChild>
          <Button
            variant="ghost"
            size="icon"
            onClick={() => setThreadOpen(true)}
            disabled={!canThread}
            className="h-6 w-6"
          >
            <MessageSquareMore className="h-3.5 w-3.5" />
          </Button>
        </TooltipTrigger>
        <TooltipContent>
          {canThread ? t('mail.viewThread') : noPermission}
        </TooltipContent>
      </Tooltip>
      <Tooltip delayDuration={800}>
        <TooltipTrigger asChild>
          <Button
            variant="ghost"
            size="icon"
            disabled={!canRestore}
            onClick={() => {
              setSelected(new Map())
              setOpen('restore')
            }}
            className="h-6 w-6"
          >
            <Upload className="h-3.5 w-3.5" />
          </Button>
        </TooltipTrigger>
        <TooltipContent>
          {canRestore
            ? t('restore_message.restore_to_imap', 'Restore Mail')
            : noPermission}
        </TooltipContent>
      </Tooltip>
      <Tooltip delayDuration={800}>
        <TooltipTrigger asChild>
          <Button
            variant="ghost"
            size="icon"
            disabled={!canRepair}
            onClick={() => {
              setSelected(new Map())
              setOpen('repair')
            }}
            className="h-6 w-6"
          >
            <Wrench className="h-3.5 w-3.5" />
          </Button>
        </TooltipTrigger>
        <TooltipContent>
          {canRepair
            ? t('repair_message.repairAction', 'Repair message')
            : noPermission}
        </TooltipContent>
      </Tooltip>
      <Button asChild variant="ghost" size="icon" className="h-6 w-6">
        <Link
          to="/search"
          search={{
            q: JSON.stringify({ id: envelope.id }),
            page: 1,
            pageSize: 30,
            sortOrder: 'desc',
          }}
          title={t('mail.openInSearch', 'Open in search')}
          // Close the reader drawer (if any) before showing the search page.
          onClick={() => setOpen(null)}
        >
          <ExternalLink className="h-3.5 w-3.5" />
        </Link>
      </Button>
    </>
  )

  return (
    <div className="flex flex-col">
      {showHeader && (
        <div className="text-xs">
          {/* Compact summary: subject, sender and timestamp stay visible;
              account, id, recipients and tags collapse behind the chevron. */}
          <div className="flex max-w-full items-start gap-2 overflow-hidden">
            <div className="min-w-0 flex-1 overflow-hidden">
              <div
                className="max-w-full truncate text-sm font-semibold text-foreground"
                title={envelope.subject}
              >
                {envelope.subject || t('mail.noSubject')}
              </div>
              <div className="mt-0.5 flex items-baseline gap-2 text-[11px] text-muted-foreground">
                {envelope.from && (
                  <span
                    className="min-w-0 truncate"
                    title={envelope.wire_from ? `via ${envelope.wire_from}` : envelope.from}
                  >
                    {formatSender(envelope.from_name, envelope.from)}
                  </span>
                )}
                {envelope.wire_from && (
                  <span
                    className="min-w-0 truncate text-[10px] text-muted-foreground/70"
                    title={`Original From: ${envelope.wire_from}`}
                  >
                    via {envelope.wire_from}
                  </span>
                )}
                {envelope.internal_date && (
                  <span className="shrink-0 whitespace-nowrap tabular-nums">
                    {formatTimestamp(envelope.internal_date)}
                  </span>
                )}
              </div>
            </div>

            <div className="flex shrink-0 items-center gap-0.5">
              {showActions && actionButtons}
              <Button
                variant="ghost"
                size="icon"
                className="h-6 w-6"
                onClick={() => setDetailsOpen((open) => !open)}
                title={
                  detailsOpen
                    ? t('mail.hideDetails', 'Hide details')
                    : t('mail.showDetails', 'Show details')
                }
              >
                <ChevronDown
                  className={cn(
                    'h-4 w-4 transition-transform',
                    detailsOpen && 'rotate-180'
                  )}
                />
              </Button>
            </div>
          </div>

          {detailsOpen && (
            <div className="mt-2 grid gap-1 border-t pt-2">
              <div className="flex space-x-2">
                <span className="font-medium text-gray-400">
                  {t('mail.account')}:
                </span>
                <span>{getEmailById(envelope.account_id)}</span>
              </div>
              <div className="flex min-w-0 items-center space-x-2">
                <span className="font-medium text-gray-400">{t('mail.id')}:</span>
                <span className="min-w-0 truncate">{envelope.id}</span>
                <Tooltip delayDuration={800}>
                  <TooltipTrigger asChild>
                    <Button
                      variant="ghost"
                      size="icon"
                      onClick={copyEnvelopeId}
                      className="h-5 w-5"
                    >
                      <Copy className="h-3 w-3" />
                    </Button>
                  </TooltipTrigger>
                  <TooltipContent>{t('mail.copyId')}</TooltipContent>
                </Tooltip>
              </div>
              {envelope.to && envelope.to.length > 0 && (
                <Multilines title={t('mail.to')} lines={envelope.to} />
              )}
              {envelope.cc && envelope.cc.length > 0 && (
                <Multilines title={t('mail.cc')} lines={envelope.cc} />
              )}
              {envelope.bcc && envelope.bcc.length > 0 && (
                <Multilines title={t('mail.bcc')} lines={envelope.bcc} />
              )}
              <div className="flex items-center space-x-2">
                <span className="font-medium text-gray-400">
                  {t('mail.tags')}:
                </span>
                <div className="flex flex-wrap gap-1 items-center">
                  {envelope.tags && envelope.tags.length > 0 ? (
                    envelope.tags.map((tag) => (
                      <Badge
                        key={tag}
                        variant="secondary"
                        className="text-[10px] h-5 px-1.5"
                      >
                        {tag}
                      </Badge>
                    ))
                  ) : (
                    <span className="text-gray-400 italic">
                      {t('mail.noTagsYet')}
                    </span>
                  )}
                  <Button
                    variant="ghost"
                    size="icon"
                    className="h-5 w-5 ml-0.5"
                    disabled={!canEditTags}
                    title={!canEditTags ? noPermission : undefined}
                    onClick={() => setEditTagsOpen(true)}
                  >
                    <Pencil className="h-3 w-3" />
                  </Button>
                </div>
              </div>
            </div>
          )}
        </div>
      )}

      {!showHeader && showActions && (
        <div className="flex items-center justify-start gap-1 text-xs text-gray-500">
          {actionButtons}
        </div>
      )}
      {showAttachments && !loading && nonInlineAttachments.length > 0 && (
        <>
          <Separator className="my-2" />
          <div className="mb-2 space-y-2">
            {nonInlineAttachments.map((attachment, i) => {
              const { icon, color } = getFileConfig(attachment.file_type);
              const is_message = attachment.is_message;

              return <div key={i} className="flex items-center">
                <div className="group flex items-center gap-2 p-1 hover:bg-muted/60 rounded transition-colors min-w-0 w-full">
                  <div className={`flex-shrink-0 ${color}`}>
                    {icon}
                  </div>
                  <div className="flex items-center justify-between min-w-0 flex-1 gap-2">
                    <button
                      type="button"
                      className="truncate text-xs font-medium text-foreground/90 cursor-pointer hover:text-primary hover:underline transition-colors text-left"
                      title={attachment.filename}
                      onClick={() =>
                        setPreviewAttachment({
                          attachments: nonInlineAttachments.map((a) => ({
                            content_hash: a.content_hash,
                            file_type: a.file_type,
                            filename: a.filename,
                          })),
                          index: i,
                        })
                      }
                    >
                      {attachment.filename}
                    </button>
                    <span className="flex-shrink-0 text-[9px] font-bold text-muted-foreground/60 bg-muted px-1 py-0.5 rounded uppercase">
                      {attachment.file_type.split('/').pop()}
                    </span>
                  </div>
                </div>
                <div className="flex items-center space-x-3 ml-auto pr-1">
                  {is_message && (
                    <Tooltip>
                      <TooltipTrigger asChild>
<span className="inline-flex">
                        <Button
                          variant="ghost"
                          size="sm"
                          className="h-7 w-7 p-0 text-orange-600 hover:text-orange-700 hover:bg-orange-50"
                          onClick={() => {
                            handleViewNestedEml(attachment);
                          }}
                        >
                          <MessageSquareMore className="h-4 w-4" />
                        </Button>
                      </span>
</TooltipTrigger>
                      <TooltipContent>{t('mail.viewNestedEmail', 'View Embedded Email')}</TooltipContent>
                    </Tooltip>
                  )}
                  <span className="text-gray-500 text-xs shrink-0">
                    {formatBytes(attachment.size)}
                  </span>
                  {downloadingAttachmentFileName === attachment.filename ? (
                    <Loader className="w-5 h-5 animate-spin" />
                  ) : (
                    <Download
                      className="w-5 h-5 cursor-pointer"
                      onClick={() => {
                        setDownloadingAttachmentFileName(attachment.filename);
                        downloadAttachmentMutation.mutate({ content_hash: attachment.content_hash });
                      }}
                    />
                  )}
                </div>
              </div>
            })}
          </div>
          <Separator className="mb-2" />
        </>
      )}
      {hasRemoteContent && (
        <div className="mt-3 flex items-center justify-between bg-muted border px-3 py-1.5 mb-3 text-xs">
          <div className="flex items-center gap-1.5 min-w-0">
            <ShieldCheck className="h-3.5 w-3.5 text-muted-foreground shrink-0" />
            {blockRemote ? (
              <span className="text-muted-foreground truncate">
                {t('mail.remoteBlocked', 'To protect your privacy, Bichon has blocked remote content in this message.')}
              </span>
            ) : (
              <span className="text-muted-foreground truncate">
                {t('mail.remoteShown', 'Remote content is now shown.')}
              </span>
            )}
          </div>
          <span
            className="underline cursor-pointer hover:no-underline text-muted-foreground text-[11px] font-medium shrink-0 ml-2 select-none"
            onClick={toggleBlockRemote}
          >
            {blockRemote
              ? t('mail.showRemoteContent', 'Show remote content')
              : t('mail.blockRemoteAgain', 'Block again')}
          </span>
        </div>
      )}
      <div className="mt-3">
        {loading ? (
          <div className="flex justify-center items-center py-8">
            <Loader className="w-6 h-6 animate-spin" />
            <span className="ml-2 text-sm text-muted-foreground">loading...</span>
          </div>
        ) : content ? (
          <div className="bg-gray-100 rounded-lg border border-gray-300 p-4">
            {contentType === 'Html' ? (
              <EmailIframe emailHtml={content} />
            ) : (
              <pre className="whitespace-pre-wrap text-gray-800 text-sm font-sans">{content}</pre>
            )}
          </div>
        ) : (
          <div className="text-center text-muted-foreground text-sm">No content available</div>
        )}
      </div>

      <MailThreadDialog open={threadOpen} onOpenChange={setThreadOpen} />
      <NestedEmailDialog
        open={!!nestedEmlFile}
        onOpenChange={(open: boolean) => !open && setNestedEmlFile(null)}
        accountId={envelope.account_id}
        envelopeId={envelope.id}
        fileName={nestedEmlFile?.filename || ''}
        content_hash={nestedEmlFile?.content_hash}
      />
      {previewAttachment?.attachments?.[previewAttachment.index] && (
        <AttachmentPreview
          open={!!previewAttachment}
          onOpenChange={(open) => !open && setPreviewAttachment(null)}
          accountId={envelope.account_id}
          envelopeId={envelope.id}
          contentHash={previewAttachment.attachments[previewAttachment.index].content_hash}
          contentType={previewAttachment.attachments[previewAttachment.index].file_type}
          fileName={previewAttachment.attachments[previewAttachment.index].filename}
          attachments={previewAttachment.attachments}
          attachmentIndex={previewAttachment.index}
        />
      )}
    </div>
  );
}

export function formatTimestamp(milliseconds: number): string {
  const date = new Date(milliseconds);
  const year = date.getFullYear();
  const month = String(date.getMonth() + 1).padStart(2, '0');
  const day = String(date.getDate()).padStart(2, '0');
  const hours = String(date.getHours()).padStart(2, '0');
  const minutes = String(date.getMinutes()).padStart(2, '0');
  const seconds = String(date.getSeconds()).padStart(2, '0');
  const timezoneOffset = date.getTimezoneOffset();
  const offsetSign = timezoneOffset > 0 ? '-' : '+';
  const offsetHours = String(Math.floor(Math.abs(timezoneOffset) / 60)).padStart(2, '0');
  const offsetMinutes = String(Math.abs(timezoneOffset) % 60).padStart(2, '0');
  return `${year}-${month}-${day}T${hours}:${minutes}:${seconds}${offsetSign}${offsetHours}:${offsetMinutes}`;
}