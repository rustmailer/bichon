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

import { formatBytes, useFileUpload, type FileWithPreview } from '@/hooks/use-file-upload';
import { Button } from '@/components/ui/button';
import { TriangleAlert, User, X } from 'lucide-react';
import { cn } from '@/lib/utils';
import { AVATAR_ACCEPT, AVATAR_PICKER_MAX_BYTES } from '@/lib/avatar';
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert';
import { useTranslation } from 'react-i18next';

interface AvatarUploadProps {
    maxSize?: number;
    className?: string;
    onFileChange?: (file: FileWithPreview | null) => void;
    defaultAvatar?: string;
    disabled: boolean;
}

export default function AvatarUpload({
    maxSize = AVATAR_PICKER_MAX_BYTES,
    className,
    onFileChange,
    defaultAvatar,
    disabled
}: AvatarUploadProps) {
    const { t } = useTranslation();

    const [
        { files, isDragging, errors },
        { removeFile, handleDragEnter, handleDragLeave, handleDragOver, handleDrop, openFileDialog, getInputProps },
    ] = useFileUpload({
        maxFiles: 1,
        maxSize,
        accept: AVATAR_ACCEPT,
        multiple: false,
        onFilesChange: (files) => {
            onFileChange?.(files[0] || null);
        },
    });

    const currentFile = files[0];
    const previewUrl = currentFile?.preview || defaultAvatar;

    const handleRemove = (e: React.MouseEvent) => {
        e.stopPropagation();
        if (currentFile) {
            removeFile(currentFile.id);
        }
    };

    return (
        <div className={cn('flex flex-col items-center gap-4', className)}>
            <div className="relative">
                <div
                    className={cn(
                        'group/avatar relative h-24 w-24 cursor-pointer overflow-hidden rounded-full border border-dashed transition-colors',
                        disabled
                            ? 'cursor-not-allowed opacity-60'
                            : 'cursor-pointer border-dashed hover:border-muted-foreground/20',
                        isDragging ? 'border-primary bg-primary/5' : 'border-muted-foreground/25 hover:border-muted-foreground/20',
                        previewUrl && 'border-solid',
                    )}
                    onDragEnter={handleDragEnter}
                    onDragLeave={handleDragLeave}
                    onDragOver={handleDragOver}
                    onDrop={handleDrop}
                    onClick={openFileDialog}
                >
                    <input {...getInputProps({ disabled })} className="sr-only" />

                    {previewUrl ? (
                        <img src={previewUrl} alt={t('settings.profile.avatarAlt')} className="h-full w-full object-cover" />
                    ) : (
                        <div className="flex h-full w-full items-center justify-center">
                            <User className="size-6 text-muted-foreground" />
                        </div>
                    )}
                </div>
                {currentFile && (
                    <Button
                        size="icon"
                        variant="outline"
                        onClick={handleRemove}
                        className="size-6 absolute end-0 top-0 rounded-full"
                        aria-label={t('settings.profile.removeAvatar')}
                        disabled={disabled}
                    >
                        <X className="size-3.5" />
                    </Button>
                )}
            </div>
            <div className="text-center space-y-0.5">
                <p className="text-sm font-medium">{t('settings.profile.uploadTitle')}</p>
                <p className="text-xs text-muted-foreground">
                    {t('settings.profile.maxSize', { size: formatBytes(maxSize) })}
                </p>
                <p className="text-xs text-muted-foreground">
                    {t('settings.profile.avatarResizeHint')}
                </p>
            </div>

            {errors.length > 0 && (
                <Alert variant="destructive" className="mt-5">
                    <AlertTitle className="flex items-center gap-2 font-semibold">
                        <TriangleAlert className="h-4 w-4 text-destructive" />
                        <span>{t('settings.profile.uploadError')}</span>
                    </AlertTitle>
                    <AlertDescription>
                        {errors.map((error, index) => (
                            <p key={index} className="last:mb-0">
                                {error}
                            </p>
                        ))}
                    </AlertDescription>
                </Alert>
            )}
        </div>
    );
}