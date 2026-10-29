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

import { EmailEnvelope } from '@/api';
import { AttachmentInfo, download_nested_attachment, load_nested_message } from '@/api/mailbox/envelope/api';
import EmailIframe from '@/components/mail-iframe';
import { Dialog, DialogContent } from '@/components/ui/dialog';
import { Separator } from '@/components/ui/separator';
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip';
import { formatBytes, formatSender, formatTimestamp } from '@/lib/utils';
import { useQuery } from '@tanstack/react-query';
import { Download, Loader, Mail } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { getFileConfig } from './mail-message-view';

const MessageHeader = ({
    envelope,
    attachments,
    onDownload
}: {
    envelope: EmailEnvelope,
    attachments?: AttachmentInfo[],
    onDownload: (nested_content_hash: string, fileName: string) => void
}) => {
    const { t } = useTranslation();
    const displayAttachments = attachments || [];

    return (
        <div className="space-y-4 mb-4 bg-white p-5 rounded-xl border shadow-sm">
            <div className="space-y-1">
                <h1 className="text-lg font-bold text-slate-900 leading-snug">
                    {envelope.subject || `(${t('mail.noSubject')})`}
                </h1>
                <div className="text-[11px] text-slate-400">
                    {formatTimestamp(envelope.date)}
                </div>
            </div>

            <Separator className="opacity-50" />
            <div className="grid grid-cols-1 gap-y-3">
                {/* From */}
                <div className="flex items-baseline gap-2">
                    <span className="w-12 text-[10px] font-bold uppercase text-slate-400 shrink-0">
                        {t('mail.from')}
                    </span>
                    <span
                        className="text-sm font-medium text-slate-700 truncate"
                        title={envelope.wire_from ? `via ${envelope.wire_from}` : undefined}
                    >
                        {formatSender(envelope.from_name, envelope.from)}
                    </span>
                </div>

                {envelope.to && envelope.to.length > 0 && (
                    <div className="flex items-baseline gap-2">
                        <span className="w-12 text-[10px] font-bold uppercase text-slate-400 shrink-0">
                            {t('mail.to')}
                        </span>
                        <div className="flex flex-wrap gap-x-2 gap-y-1">
                            {envelope.to.map((addr, i) => (
                                <span key={i} className="text-sm text-slate-600">
                                    {addr}{i < envelope.to.length - 1 ? ',' : ''}
                                </span>
                            ))}
                        </div>
                    </div>
                )}

                {envelope.cc && envelope.cc.length > 0 && (
                    <div className="flex items-baseline gap-2">
                        <span className="w-12 text-[10px] font-bold uppercase text-slate-400 shrink-0">
                            {t('mail.cc')}
                        </span>
                        <div className="flex flex-wrap gap-x-2 gap-y-1 text-slate-500 italic">
                            {envelope.cc.map((addr, i) => (
                                <span key={i} className="text-xs">
                                    {addr}{i < envelope.cc.length - 1 ? ',' : ''}
                                </span>
                            ))}
                        </div>
                    </div>
                )}

                {envelope.bcc && envelope.bcc.length > 0 && (
                    <div className="flex items-baseline gap-2">
                        <span className="w-12 text-[10px] font-bold uppercase text-slate-400 shrink-0">
                            {t('mail.bcc')}
                        </span>
                        <div className="flex flex-wrap gap-x-2 gap-y-1 text-slate-500 italic">
                            {envelope.bcc.map((addr, i) => (
                                <span key={i} className="text-xs">
                                    {addr}{i < envelope.bcc.length - 1 ? ',' : ''}
                                </span>
                            ))}
                        </div>
                    </div>
                )}
            </div>

            {displayAttachments.length > 0 && (
                <div className="pt-2 border-t border-dashed">
                    <div className="flex flex-wrap gap-2">
                        {displayAttachments.map((att, i) => {
                            const { icon, color } = getFileConfig(att.file_type);
                            return (
                                <Tooltip key={i}>
                                    <TooltipTrigger asChild>
                                        <button
                                            onClick={() => onDownload(att.content_hash, att.filename)}
                                            className="group flex items-center gap-2 px-3 py-1.5 bg-slate-50 border border-slate-200 rounded-lg hover:bg-blue-50 hover:border-blue-200 transition-all text-slate-600 hover:text-blue-700"
                                        >
                                            <span className={`${color} p-0.5 rounded`}>{icon}</span>
                                            <span className="text-xs font-medium truncate max-w-[180px]">
                                                {att.filename}
                                            </span>
                                            <span className="text-[9px] text-slate-400 group-hover:text-blue-400">
                                                ({formatBytes(att.size)})
                                            </span>
                                            <Download className="h-3 w-3 ml-1 opacity-0 group-hover:opacity-100 transition-opacity" />
                                        </button>
                                    </TooltipTrigger>
                                    <TooltipContent>{t('mail.clickToDownload')}</TooltipContent>
                                </Tooltip>
                            );
                        })}
                    </div>
                </div>
            )}
        </div>
    );
};



export function NestedEmailDialog({ open, onOpenChange, accountId, envelopeId, fileName, content_hash }: any) {

    const { data, isLoading } = useQuery({
        queryKey: ['nested-message', accountId, envelopeId, content_hash],
        queryFn: () => load_nested_message(accountId, envelopeId, content_hash, true),
        enabled: open && !!content_hash,
    });

    return (
        <Dialog open={open} onOpenChange={onOpenChange}>
            <DialogContent className="max-w-4xl h-[90vh] flex flex-col p-0 overflow-hidden border-none shadow-2xl">
                <div className="text-white px-4 py-3 flex items-center justify-between">
                    <div className="flex items-center gap-2">
                        <Mail className="h-4 w-4 text-blue-400" />
                        <span className="text-sm font-medium truncate max-w-[400px] opacity-90">{fileName}</span>
                    </div>
                </div>

                <div className="flex-1 overflow-auto bg-white p-8">
                    {isLoading ? (
                        <div className="h-full flex items-center justify-center"><Loader className="animate-spin" /></div>
                    ) : data && (
                        <div className="max-w-3xl mx-auto">
                            <MessageHeader
                                envelope={data.envelope}
                                attachments={data.attachments}
                                onDownload={(nested_content_hash, fileName) => download_nested_attachment(accountId, envelopeId, content_hash, nested_content_hash, fileName)}
                            />

                            <div className="mt-8 pt-8 border-t border-slate-100">
                                {data.html ? (
                                    <EmailIframe emailHtml={data.html} />
                                ) : (
                                    <pre className="whitespace-pre-wrap font-sans text-sm text-slate-800 leading-relaxed">
                                        {data.text}
                                    </pre>
                                )}
                            </div>
                        </div>
                    )}
                </div>
            </DialogContent>
        </Dialog>
    );
}
