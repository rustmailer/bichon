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
import { Loader, Download, Trash2, MessageSquareMore, FileText, FileImage, FileVideo, FileArchive, FileSpreadsheet, FileCode, FileIcon, FileAudio, Upload, ShieldCheck } from 'lucide-react';

import { Button } from '@/components/ui/button';
import { Separator } from '@/components/ui/separator';
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip';
import { toast } from '@/hooks/use-toast';
import { formatBytes, formatSender } from '@/lib/utils';
import EmailIframe from '@/components/mail-iframe';
import {
  AttachmentInfo,
  download_attachment,
  download_message,
  getContent,
  load_message,
} from '@/api/mailbox/envelope/api';
import { AxiosError } from 'axios';
import { useAttachmentContext } from './context';
import { MailThreadDialog } from './thread-dialog';
import useMinimalAccountList from '@/hooks/use-minimal-account-list';
import { useTranslation } from 'react-i18next';
import { NestedEmailDialog } from './nested-email-dialog';
import AttachmentPreview, { type PreviewAttachment } from './attachment-preview';
import { EmailEnvelope } from '@/api';


interface MailMessageViewProps {
  envelope: EmailEnvelope;
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
  const { setToDelete, setOpen, setSelected } = useAttachmentContext();
  const [content, setContent] = useState<string | null>(null);
  const [contentType, setContentType] = useState<'Plain' | 'Html' | null>(null);
  const [attachments, setAttachments] = useState<AttachmentInfo[] | null>(null);
  const [loading, setLoading] = useState(true);
  const [downloadingAttachmentFileName, setDownloadingAttachmentFileName] = useState<string | null>(null);
  const [nestedEmlFile, setNestedEmlFile] = useState<AttachmentInfo | null>(null);
  const { getEmailById } = useMinimalAccountList();
  const [threadOpen, setThreadOpen] = useState(false);
  const [blockRemote, setBlockRemote] = useState(true);
  const [hasRemoteContent, setHasRemoteContent] = useState(false);
  const [previewAttachment, setPreviewAttachment] = useState<{ attachments: PreviewAttachment[]; index: number } | null>(null);

  const toggleBlockRemote = () => {
    setBlockRemote((prev) => !prev);
  };

  const downloadAttachmentMutation = useMutation({
    mutationFn: ({ content_hash }: { content_hash: string }) =>
      download_attachment(envelope.account_id, envelope.id, content_hash, downloadingAttachmentFileName!),
    onSuccess: () => setDownloadingAttachmentFileName(null),
    onError: (error: any) => {
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
    onError: (error: any) => {
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

  return (
    <div className="flex flex-col">
      {showHeader && <div className="grid gap-1 text-xs">
        <div className="flex space-x-2">
          <span className="font-medium text-gray-400">{t('mail.account')}:</span>
          <span>{getEmailById(envelope.account_id)}</span>
        </div>
        <div className="flex space-x-2">
          <span className="font-medium text-gray-400">{t('mail.id')}:</span>
          <span>{envelope.id}</span>
        </div>
        {envelope.from && (
          <div className="flex space-x-2">
            <span className="font-medium text-gray-400">{t('mail.from')}:</span>
            <span title={envelope.wire_from ? `via ${envelope.wire_from}` : undefined}>{formatSender(envelope.from_name, envelope.from)}</span>
            {envelope.wire_from && (
              <span className="text-gray-400 text-xs">via {envelope.wire_from}</span>
            )}
          </div>
        )}
        {envelope.to && envelope.to.length > 0 && <Multilines title={t('mail.to')} lines={envelope.to} />}
        {envelope.cc && envelope.cc.length > 0 && <Multilines title={t('mail.cc')} lines={envelope.cc} />}
        {envelope.bcc && envelope.bcc.length > 0 && <Multilines title={t('mail.bcc')} lines={envelope.bcc} />}
        {envelope.subject && (
          <div className="flex space-x-2">
            <span className="font-medium text-gray-400">{t('mail.subject')}:</span>
            <span>{envelope.subject}</span>
          </div>
        )}
        {envelope.internal_date && (
          <div className="flex space-x-2">
            <span className="font-medium text-gray-400">{t('mail.date')}:</span>
            <span>{formatTimestamp(envelope.internal_date)}</span>
          </div>
        )}
      </div>}

      {showActions && (
        <>
          <div className="flex items-center mt-2 space-x-2">
            <Separator orientation="horizontal" className="flex-1 bg-border" />
          </div>
          <div className="flex items-center justify-start gap-3 text-xs text-gray-500">
            <Tooltip>
              <TooltipTrigger asChild>
                <Button variant="ghost" size="icon" onClick={handleDelete} className="hover:text-destructive">
                  <Trash2 className="h-4 w-4" />
                </Button>
              </TooltipTrigger>
              <TooltipContent>{t('mail.delete')}</TooltipContent>
            </Tooltip>
            <Separator orientation="vertical" className="h-5" />
            <Tooltip>
              <TooltipTrigger asChild>
                <Button variant="ghost" size="icon" onClick={downloadEmlFile}>
                  <Download className="h-4 w-4" />
                </Button>
              </TooltipTrigger>
              <TooltipContent>{t('mail.download')}</TooltipContent>
            </Tooltip>
            <Separator orientation="vertical" className="h-5" />
            <Tooltip>
              <TooltipTrigger asChild>
                <Button
                  variant="ghost"
                  size="icon"
                  onClick={() => setThreadOpen(true)}
                >
                  <MessageSquareMore className="h-4 w-4" />
                </Button>
              </TooltipTrigger>
              <TooltipContent>{t('mail.viewThread')}</TooltipContent>
            </Tooltip>
            <Separator orientation="vertical" className="h-5" />
            <Tooltip>
              <TooltipTrigger asChild>
                <Button
                  variant="ghost"
                  size="icon"
                  onClick={() => {
                    setSelected(new Map())
                    setOpen('restore')
                  }}
                >
                  <Upload className="h-4 w-4" />
                </Button>
              </TooltipTrigger>
              <TooltipContent>{t('restore_message.restore_to_imap', 'Restore Mail')}</TooltipContent>
            </Tooltip>
          </div>
        </>
      )}
      {showAttachments && <Separator className="my-2" />}
      {showAttachments && (
        <div className="mb-2">
          {loading ? (
            <span className="text-gray-500 text-xs" />
          ) : attachments && attachments.length > 0 ? (
            (() => {
              const nonInline = attachments.filter((a) => !a.inline);

              return nonInline.length > 0 ? (
                <div className="space-y-2">
                  {nonInline.map((attachment, i) => {
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
                                attachments: nonInline.map((a) => ({
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
              ) : (
                <span className="text-gray-500 text-xs italic">
                  {t('mail.onlyNonInlineAttachments')}
                </span>
              );
            })()
          ) : (
            <span className="text-gray-500 text-xs">{t('mail.noAttachments')}</span>
          )}
        </div>
      )}
      {showAttachments && <Separator className="mb-2" />}
      {hasRemoteContent && (
        <div className="flex items-center justify-between bg-muted border px-3 py-1.5 mb-3 text-xs">
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
      <div>
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

      <MailThreadDialog open={threadOpen} onOpenChange={setThreadOpen} currentEnvelope={envelope} />
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