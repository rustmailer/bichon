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

import React from "react";
import {
    Accordion,
    AccordionItem,
    AccordionTrigger,
    AccordionContent,
} from "@/components/ui/accordion";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Copy, Trash2 } from "lucide-react";
import { Separator } from "@/components/ui/separator";
import {
    Dialog,
    DialogContent,
    DialogFooter,
    DialogHeader,
    DialogTitle,
} from "@/components/ui/dialog";
import { format } from "date-fns";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { AccessToken, remove_access_token } from "@/api/users/api";
import { OP_TOKEN_REVOKE } from "@/api/approvals/api";
import { useGatedOp } from "@/features/approvals/use-gated-op";
import { useTranslation } from "react-i18next";
import { toast } from "@/hooks/use-toast";
import { copyToClipboard } from "@/lib/clipboard";

interface Props {
    tokens: AccessToken[];
    userId: number;
}

const isTokenExpired = (expireAt: number | null | undefined): boolean => {
    if (!expireAt) return false;
    return new Date(expireAt) < new Date();
};

export const TokenCardList: React.FC<Props> = ({ tokens, userId }) => {
    const { t } = useTranslation();
    const queryClient = useQueryClient();
    const { gated, announceIfQueued } = useGatedOp(OP_TOKEN_REVOKE);
    const [deleteTarget, setDeleteTarget] =
        React.useState<AccessToken | null>(null);

    const deleteMutation = useMutation({
        mutationFn: (token: string) => remove_access_token(token),
        onSuccess: (data) => {
            queryClient.invalidateQueries({ queryKey: ['user-tokens', userId] });
            queryClient.invalidateQueries({ queryKey: ['approvals'] });
            setDeleteTarget(null);
            // A queued revocation leaves the token working. Reporting success
            // would tell an operator a leaked credential is dead when it is
            // still live — the one mistake this dialog must not make.
            if (announceIfQueued(data)) return;
            toast({
                description: t('apiTokens.notifications.deleteSuccess'),
            });
        },
        onError: () => {
            toast({
                variant: "destructive",
                description: t('apiTokens.notifications.deleteFailed'),
            });
        }
    });

    const handleCopy = async (text: string) => {
        try {
            await copyToClipboard(text);
            toast({
                description: t('apiTokens.notifications.copied'),
            });
        } catch {
            toast({
                variant: "destructive",
                description: t('settings.failedToCopyText'),
            });
        }
    };

    return (
        <>
            <Accordion type="multiple" className="w-full space-y-4">
                {tokens.map((token) => {
                    const expired = isTokenExpired(token.expire_at);
                    const itemValue = token.token;

                    return (
                        <AccordionItem
                            key={itemValue}
                            value={itemValue}
                            className={`border rounded-lg shadow-md transition-all duration-300 ${expired
                                ? "border-red-400 bg-red-50/50"
                                : "border-gray-200"
                                }`}
                        >
                            <AccordionTrigger className="px-5 py-4 hover:no-underline">
                                <div className="flex items-center justify-between w-full gap-4">
                                    <div className="flex flex-col gap-1 min-w-0">
                                        <h3 className="text-sm font-semibold truncate">
                                            {token.name || t('apiTokens.list.unnamedToken')}
                                        </h3>

                                        <div className="flex items-center gap-2">
                                            <Badge
                                                variant={expired ? "destructive" : "secondary"}
                                                className="text-xs"
                                            >
                                                {expired ? t('apiTokens.list.statusExpired') : t('apiTokens.list.statusActive')}
                                            </Badge>
                                            {token.metrics_only && (
                                                <Badge variant="outline" className="text-xs">
                                                    {t('apiTokens.list.metricsOnlyBadge')}
                                                </Badge>
                                            )}
                                        </div>
                                    </div>

                                    <div className="text-xs text-muted-foreground whitespace-nowrap">
                                        {token.expire_at
                                            ? t('apiTokens.list.expiresOnShort', {
                                                date: format(new Date(token.expire_at), "yyyy-MM-dd")
                                            })
                                            : t('apiTokens.list.neverExpires')}
                                    </div>
                                </div>
                            </AccordionTrigger>

                            <AccordionContent className="px-5 pb-5 pt-2 bg-muted/40">
                                <Separator className="mb-4" />
                                <div className="space-y-4">
                                    <div>
                                        <div className="text-xs font-medium text-muted-foreground mb-1">
                                            {t('apiTokens.list.tokenLabel')}
                                        </div>
                                        <div className="flex items-center gap-2 bg-background border rounded-md px-3 py-2">
                                            <code className="font-mono text-xs truncate flex-1">
                                                {token.token}
                                            </code>
                                            <Button
                                                size="icon"
                                                variant="ghost"
                                                className="h-7 w-7"
                                                onClick={() => handleCopy(token.token)}
                                            >
                                                <Copy className="h-4 w-4" />
                                            </Button>
                                        </div>
                                    </div>
                                    <div className="grid grid-cols-1 sm:grid-cols-3 gap-3 text-xs text-muted-foreground">
                                        <div>
                                            <span className="block font-medium text-foreground">
                                                {t('apiTokens.list.createdLabel')}
                                            </span>
                                            {format(new Date(token.created_at), "yyyy-MM-dd HH:mm")}
                                        </div>

                                        <div>
                                            <span className="block font-medium text-foreground">
                                                {t('apiTokens.list.lastUsedLabel')}
                                            </span>
                                            {token.last_access_at > 0
                                                ? format(new Date(token.last_access_at), "yyyy-MM-dd HH:mm")
                                                : t('apiTokens.list.neverUsed')}
                                        </div>

                                        <div>
                                            <span className="block font-medium text-foreground">
                                                {t('apiTokens.list.expiresOnLabel')}
                                            </span>
                                            {token.expire_at
                                                ? format(new Date(token.expire_at), "yyyy-MM-dd HH:mm")
                                                : t('apiTokens.list.neverLabel')}
                                        </div>
                                    </div>
                                    <div className="pt-2 flex justify-end">
                                        <Button
                                            size="sm"
                                            variant="destructive"
                                            onClick={() => setDeleteTarget(token)}
                                        >
                                            <Trash2 className="h-4 w-4 mr-2" />
                                            {t('apiTokens.list.deleteBtn')}
                                        </Button>
                                    </div>
                                </div>
                            </AccordionContent>
                        </AccordionItem>
                    );
                })}
            </Accordion>
            <Dialog
                open={!!deleteTarget}
                onOpenChange={(open) => !open && setDeleteTarget(null)}
            >
                <DialogContent aria-describedby={undefined} className="max-w-sm">
                    <DialogHeader>
                        <DialogTitle>{t('apiTokens.deleteDialog.title')}</DialogTitle>
                    </DialogHeader>

                    <p className="text-sm text-muted-foreground">
                        {gated
                            ? t(
                                  'approvals.gatedTokenDesc',
                                  'This will NOT revoke the token now. It submits a request that a second person must approve, and only then is the token revoked. Until that happens the token keeps working — if it has leaked, treat it as live.'
                              )
                            : t('apiTokens.deleteDialog.description')}
                    </p>

                    <DialogFooter className="mt-4">
                        <Button
                            variant="outline"
                            onClick={() => setDeleteTarget(null)}
                        >
                            {t('apiTokens.deleteDialog.cancel')}
                        </Button>

                        <Button
                            variant="destructive"
                            disabled={deleteMutation.isPending}
                            onClick={() => {
                                if (!deleteTarget) return;
                                deleteMutation.mutate(deleteTarget.token);
                            }}
                        >
                            {deleteMutation.isPending
                                ? t('apiTokens.deleteDialog.deleting')
                                : gated
                                  ? t(
                                        'approvals.submitForApproval',
                                        'Submit for approval'
                                    )
                                  : t('apiTokens.deleteDialog.confirm')}
                        </Button>
                    </DialogFooter>
                </DialogContent>
            </Dialog>
        </>
    );
};