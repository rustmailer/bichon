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

import { useCallback, useEffect, useMemo, useState } from 'react';
import { useMutation } from '@tanstack/react-query';
import {
  Download, FileIcon, ZoomIn, ZoomOut, RotateCcw,
  ChevronLeft, ChevronRight, X,
} from 'lucide-react';

import { Button } from '@/components/ui/button';
import { Dialog, DialogContent } from '@/components/ui/dialog';
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip';
import { Separator } from '@/components/ui/separator';
import { Skeleton } from '@/components/ui/skeleton';
import { toast } from '@/hooks/use-toast';
import { useTranslation } from 'react-i18next';

import { preview_attachment, download_attachment } from '@/api/mailbox/envelope/api';
import { getFileConfig } from './mail-message-view';

const PREVIEWABLE_IMAGE = /^image\/(png|jpeg|gif|webp|svg\+xml)$/;
const PREVIEWABLE_TEXT = /^(text\/(plain|csv|html|xml|css|javascript|markdown)|application\/(json|xml|javascript|x-httpd-php|x-sh|x-perl|x-python|x-ruby))$/;

const EXT_TO_MIME: Record<string, string> = {
  '.pdf': 'application/pdf',
  '.png': 'image/png',
  '.jpg': 'image/jpeg',
  '.jpeg': 'image/jpeg',
  '.gif': 'image/gif',
  '.webp': 'image/webp',
  '.svg': 'image/svg+xml',
  '.txt': 'text/plain',
  '.csv': 'text/csv',
  '.html': 'text/html',
  '.htm': 'text/html',
  '.xml': 'application/xml',
  '.css': 'text/css',
  '.js': 'application/javascript',
  '.mjs': 'application/javascript',
  '.md': 'text/markdown',
  '.json': 'application/json',
  '.php': 'application/x-httpd-php',
  '.sh': 'application/x-sh',
  '.pl': 'application/x-perl',
  '.py': 'application/x-python',
  '.rb': 'application/x-ruby',
};

function getExtensionMime(filename: string): string | null {
  const dot = filename.lastIndexOf('.');
  if (dot < 0) return null;
  const ext = filename.slice(dot).toLowerCase();
  return EXT_TO_MIME[ext] ?? null;
}

function resolveContentType(contentType: string, fileName: string): string {
  if (contentType && contentType !== 'application/octet-stream') return contentType;
  return getExtensionMime(fileName) ?? contentType;
}

export interface PreviewAttachment {
  content_hash: string;
  file_type: string;
  filename: string;
}

interface AttachmentPreviewProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  accountId: number;
  envelopeId: string;
  contentHash: string;
  contentType: string;
  fileName: string;
  /** Full attachment list for gallery navigation (optional). */
  attachments?: PreviewAttachment[];
  /** Index of the current attachment within `attachments`. */
  attachmentIndex?: number;
}

function isImagePreview(contentType: string, fileName?: string) {
  return PREVIEWABLE_IMAGE.test(resolveContentType(contentType, fileName ?? ''));
}

function isPdfPreview(contentType: string, fileName?: string) {
  return resolveContentType(contentType, fileName ?? '') === 'application/pdf';
}

function isTextPreview(contentType: string, fileName?: string) {
  return PREVIEWABLE_TEXT.test(resolveContentType(contentType, fileName ?? ''));
}

export default function AttachmentPreview({
  open,
  onOpenChange,
  accountId,
  envelopeId,
  contentHash,
  contentType,
  fileName,
  attachments,
  attachmentIndex,
}: AttachmentPreviewProps) {
  const { t } = useTranslation();
  const [blobUrl, setBlobUrl] = useState<string | null>(null);
  const [textContent, setTextContent] = useState<string | null>(null);
  const [imageZoom, setImageZoom] = useState(1);

  // ── Gallery state ──────────────────────────────────────────────
  // When attachments list is provided, compute image-only indices for navigation.
  const imageIndices = useMemo(() => {
    if (!attachments) return [];
    return attachments
      .map((a, i) => (isImagePreview(a.file_type, a.filename) ? i : -1))
      .filter((i) => i >= 0);
  }, [attachments]);

  const [currentIndex, setCurrentIndex] = useState(attachmentIndex ?? 0);

  // Reset to the clicked attachment every time the dialog opens.
  useEffect(() => {
    if (open) {
      setCurrentIndex(attachmentIndex ?? 0);
    }
  }, [open, attachmentIndex]);

  // Resolve which attachment to display.
  const resolved = useMemo(() => {
    if (attachments && currentIndex < attachments.length) {
      const a = attachments[currentIndex];
      return {
        contentHash: a.content_hash,
        contentType: a.file_type,
        fileName: a.filename,
      };
    }
    return { contentHash, contentType, fileName };
  }, [attachments, currentIndex, contentHash, contentType, fileName]);

  // Position within image-only list (for "3 / 12" counter).
  const imagePos = imageIndices.indexOf(currentIndex); // -1 if not an image
  const imageTotal = imageIndices.length;

  const goPrev = useCallback(() => {
    if (imagePos > 0) setCurrentIndex(imageIndices[imagePos - 1]);
  }, [imagePos, imageIndices]);

  const goNext = useCallback(() => {
    if (imagePos < imageTotal - 1) setCurrentIndex(imageIndices[imagePos + 1]);
  }, [imagePos, imageTotal, imageIndices]);

  // Keyboard navigation
  useEffect(() => {
    if (!open) return;
    const handler = (e: KeyboardEvent) => {
      if (e.key === 'ArrowLeft') goPrev();
      else if (e.key === 'ArrowRight') goNext();
    };
    window.addEventListener('keydown', handler);
    return () => window.removeEventListener('keydown', handler);
  }, [open, goPrev, goNext]);

  const effectiveType = resolveContentType(resolved.contentType, resolved.fileName);

  // ── Fetch preview blob ─────────────────────────────────────────
  const previewMutation = useMutation({
    mutationFn: () => preview_attachment(accountId, envelopeId, resolved.contentHash),
    onSuccess: (blob) => {
      if (isTextPreview(resolved.contentType, resolved.fileName)) {
        blob.text().then(setTextContent);
      } else {
        const typedBlob = new Blob([blob], { type: effectiveType });
        setBlobUrl(URL.createObjectURL(typedBlob));
      }
    },
    onError: (error: any) => {
      toast({
        title: t('attachment_preview.failedToLoad'),
        description: error.message,
        variant: 'destructive',
      });
    },
  });

  useEffect(() => {
    if (open) {
      setBlobUrl(null);
      setTextContent(null);
      setImageZoom(1);
      previewMutation.mutate();
    }
  }, [open, resolved.contentHash]);

  useEffect(() => {
    return () => {
      if (blobUrl) URL.revokeObjectURL(blobUrl);
    };
  }, [blobUrl]);

  const handleDownload = () => {
    download_attachment(accountId, envelopeId, resolved.contentHash, resolved.fileName);
  };

  const { icon } = useMemo(() => getFileConfig(resolved.contentType), [resolved.contentType]);

  const isImage = isImagePreview(resolved.contentType, resolved.fileName);
  const isPdf = isPdfPreview(resolved.contentType, resolved.fileName);
  const isText = isTextPreview(resolved.contentType, resolved.fileName);
  const showArrows = imageTotal > 1 && isImage;

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent aria-describedby={undefined}
        className="w-screen h-screen max-w-none rounded-none p-0 gap-0 border-0 bg-slate-700/10"
        hideClose
        hideFullscreen
        onInteractOutside={(e) => {
          if (isPdf) e.preventDefault();
        }}
      >
        {/* Toolbar — hidden for PDF (browser's native viewer has its own controls) */}
        {!isPdf && (
          <div className="absolute top-0 left-0 right-0 z-10 flex items-center justify-between px-4 py-2 bg-gradient-to-b from-black/70 to-transparent text-white">
            <div className="flex items-center gap-2 min-w-0">
              {icon}
              <span className="text-sm font-medium truncate max-w-[400px]">
                {resolved.fileName}
              </span>
              {imagePos >= 0 && imageTotal > 1 && (
                <span className="text-xs text-white/60 ml-1">
                  {imagePos + 1} / {imageTotal}
                </span>
              )}
            </div>
            <div className="flex items-center gap-1 pr-12">
              {isImage && blobUrl && (
                <>
                  <Tooltip>
                    <TooltipTrigger asChild>
                      <Button
                        variant="ghost"
                        size="icon"
                        className="h-8 w-8 text-white hover:bg-white/20"
                        onClick={() => setImageZoom((z) => Math.min(z + 0.25, 3))}
                      >
                        <ZoomIn className="h-4 w-4" />
                      </Button>
                    </TooltipTrigger>
                    <TooltipContent>{t('attachment_preview.zoomIn')}</TooltipContent>
                  </Tooltip>
                  <Tooltip>
                    <TooltipTrigger asChild>
                      <Button
                        variant="ghost"
                        size="icon"
                        className="h-8 w-8 text-white hover:bg-white/20"
                        onClick={() => setImageZoom((z) => Math.max(z - 0.25, 0.25))}
                      >
                        <ZoomOut className="h-4 w-4" />
                      </Button>
                    </TooltipTrigger>
                    <TooltipContent>{t('attachment_preview.zoomOut')}</TooltipContent>
                  </Tooltip>
                  <Tooltip>
                    <TooltipTrigger asChild>
                      <Button
                        variant="ghost"
                        size="icon"
                        className="h-8 w-8 text-white hover:bg-white/20"
                        onClick={() => setImageZoom(1)}
                      >
                        <RotateCcw className="h-4 w-4" />
                      </Button>
                    </TooltipTrigger>
                    <TooltipContent>{t('attachment_preview.resetZoom')}</TooltipContent>
                  </Tooltip>
                  <Separator orientation="vertical" className="h-5 mx-1 bg-white/20" />
                </>
              )}
              <Tooltip>
                <TooltipTrigger asChild>
                  <Button
                    variant="ghost"
                    size="icon"
                    className="h-8 w-8 text-white hover:bg-white/20"
                    onClick={handleDownload}
                  >
                    <Download className="h-4 w-4" />
                  </Button>
                </TooltipTrigger>
                <TooltipContent>{t('attachment.download')}</TooltipContent>
              </Tooltip>
            </div>
          </div>
        )}

        {/* Close button — positioned below browser PDF toolbar */}
        <Button
          variant="ghost"
          size="icon"
          className={isPdf
            ? 'absolute top-12 right-4 z-50 h-10 w-10 rounded-full text-white bg-black/50 hover:bg-black/70'
            : 'absolute top-2 right-4 z-50 h-8 w-8 rounded-full text-white hover:bg-white/20'
          }
          onClick={() => onOpenChange(false)}
        >
          <X className={isPdf ? 'h-5 w-5' : 'h-4 w-4'} />
        </Button>

        {/* Navigation arrows */}
        {showArrows && (
          <>
            <Button
              variant="ghost"
              size="icon"
              disabled={imagePos <= 0}
              className="absolute left-2 top-1/2 -translate-y-1/2 z-10 h-10 w-10 rounded-full text-white hover:bg-white/20 disabled:opacity-30 disabled:hover:bg-transparent"
              onClick={goPrev}
            >
              <ChevronLeft className="h-6 w-6" />
            </Button>
            <Button
              variant="ghost"
              size="icon"
              disabled={imagePos >= imageTotal - 1}
              className="absolute right-2 top-1/2 -translate-y-1/2 z-10 h-10 w-10 rounded-full text-white hover:bg-white/20 disabled:opacity-30 disabled:hover:bg-transparent"
              onClick={goNext}
            >
              <ChevronRight className="h-6 w-6" />
            </Button>
          </>
        )}

        {/* Preview body */}
        <div className="w-full h-full flex items-center justify-center">
          {previewMutation.isPending ? (
            <div className="flex flex-col items-center gap-3">
              <Skeleton className="w-64 h-4 bg-white/10" />
              <Skeleton className="w-48 h-4 bg-white/10" />
              <Skeleton className="w-56 h-4 bg-white/10" />
            </div>
          ) : isImage && blobUrl ? (
            <div className="w-full h-full overflow-auto flex items-center justify-center">
              <img
                src={blobUrl}
                alt={resolved.fileName}
                className="max-w-full max-h-full object-contain"
                style={{
                  transform: `scale(${imageZoom})`,
                  transformOrigin: 'center center',
                }}
              />
            </div>
          ) : isPdf && blobUrl ? (
            <iframe
              src={blobUrl}
              className="w-full h-full border-0"
              title={resolved.fileName}
            />
          ) : isText && textContent !== null ? (
            <pre className="w-full h-full overflow-auto whitespace-pre-wrap text-sm font-mono p-6 text-white/90">
              {textContent}
            </pre>
          ) : !previewMutation.isPending ? (
            <div className="flex flex-col items-center gap-4 text-white/60">
              <FileIcon className="h-16 w-16 opacity-30" />
              <p className="text-sm">{t('attachment_preview.notAvailable')}</p>
              <p className="text-xs text-center max-w-md">
                {t('attachment_preview.notAvailableDesc', {
                  type: resolved.contentType || 'unknown',
                })}
              </p>
              <Button variant="outline" size="sm" onClick={handleDownload} className="text-white border-white/20 hover:bg-white/10">
                <Download className="h-4 w-4 mr-2" />
                {t('attachment.download')}
              </Button>
            </div>
          ) : null}
        </div>
      </DialogContent>
    </Dialog>
  );
}
