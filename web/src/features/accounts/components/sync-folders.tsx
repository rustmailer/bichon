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
    DialogContent,
    DialogHeader,
    DialogTitle,
    DialogDescription,
    DialogFooter,
} from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { useMutation, useQueryClient } from '@tanstack/react-query'
import { Loader2, CheckSquare, Square } from 'lucide-react'
import { useCallback, useEffect, useMemo, useState } from 'react'
import { toast } from '@/hooks/use-toast'
import { list_mailboxes, MailboxData } from '@/api/mailbox/api'
import { buildTree, ExtendedTreeItemProps } from '@/lib/build-tree'
import { Skeleton } from '@/components/ui/skeleton'
import { AccountModel, update_account } from '@/api/account/api'
import { ToastAction } from '@/components/ui/toast'
import axios, { AxiosError } from 'axios'
import { ScrollArea } from '@/components/ui/scroll-area'
import { useTranslation } from 'react-i18next'
import { RichTreeView } from '@mui/x-tree-view/RichTreeView';
import { useTheme } from '@/context/theme-context'
import React from 'react'
import Collapse from '@mui/material/Collapse';
import { styled } from '@mui/material/styles';
import { TreeItemCheckbox, TreeItemContent, TreeItemIconContainer, TreeItemLabel, TreeItemRoot } from '@mui/x-tree-view/TreeItem'
import { TreeItemDragAndDropOverlay, TreeItemIcon, TreeItemProvider, TreeViewBaseItem, TreeViewSelectionPropagation, useTreeItem, useTreeItemModel, UseTreeItemParameters } from '@mui/x-tree-view'
import { animated, useSpring } from '@react-spring/web';
import { TransitionProps } from '@mui/material/transitions'


function getParentIds(tree: TreeViewBaseItem[]): string[] {
    const result: string[] = [];

    function traverse(nodes: TreeViewBaseItem[]) {
        for (const node of nodes) {
            if (node.children && node.children.length > 0) {
                result.push(node.id);
                traverse(node.children);
            }
        }
    }

    traverse(tree);
    return result;
}


interface CustomLabelProps {
    exists?: number;
    attributes?: { attr: string; extension: string | null }[],
    children: React.ReactNode;
    icon?: React.ElementType;
    expandable?: boolean;
}

function CustomLabel({
    expandable,
    exists,
    attributes,
    children,
    ...other
}: CustomLabelProps) {
    return (
        <TreeItemLabel
            {...other}
            sx={{
                display: 'flex',
                alignItems: 'center',
            }}
        >
            <span className="font-medium text-sm text-inherit">
                {children}
            </span>
            <div className="flex gap-2 ml-auto mr-3 opacity-70 text-xs">
                {attributes?.map((attr) => {
                    const text =
                        attr.attr === 'Extension'
                            ? attr.extension
                            : attr.attr;

                    return (
                        <span key={attr.attr} className="text-inherit">
                            {text}
                        </span>
                    );
                })}
            </div>
            {exists !== undefined && (
                <span
                    className="text-sm opacity-60 min-w-[40px] text-right text-inherit"
                >
                    {exists}
                </span>
            )}
        </TreeItemLabel>
    );
}

const CustomCollapse = styled(Collapse)({
    padding: 0,
});

const AnimatedCollapse = animated(CustomCollapse);

function TransitionComponent(props: TransitionProps) {
    const style = useSpring({
        to: {
            opacity: props.in ? 1 : 0,
            transform: `translate3d(0,${props.in ? 0 : 20}px,0)`,
        },
    });

    return <AnimatedCollapse style={style} {...props} />;
}

interface CustomTreeItemProps
    extends Omit<UseTreeItemParameters, 'rootRef'>,
    Omit<React.HTMLAttributes<HTMLLIElement>, 'onFocus'> { }


interface Props {
    open: boolean
    onOpenChange: (open: boolean) => void
    currentRow: AccountModel
}

export function SyncFoldersDialog({ currentRow, open, onOpenChange }: Props) {
    const [selectedItems, setSelectedItems] = React.useState<string[]>([]);
    const [isSubmitting, setIsSubmitting] = useState(false);

    const [allIds, setAllIds] = useState<string[]>([]);
    const [expandedItems, setExpandedItems] = useState<string[]>([]);
    const [itemsWithChildren, setItemsWithChildren] = useState<string[]>([]);
    const [mailboxes, setMailboxes] = useState<MailboxData[]>([]);
    const [selectionPropagation, setSelectionPropagation] =
        React.useState<TreeViewSelectionPropagation>({
            parents: false,
            descendants: true,
        });
    const [treeData, setTreeData] = useState<TreeViewBaseItem[]>([]);
    const [isLoading, setIsLoading] = useState(false);
    const [error, setError] = useState<string | undefined>(undefined);
    const queryClient = useQueryClient();
    const { t } = useTranslation()
    const { theme } = useTheme()

    useEffect(() => {
        if (!open) return;
        let cancelled = false;
        const fetchMailboxes = async () => {
            setIsLoading(true);
            try {
                const data = await list_mailboxes(currentRow.id, true);
                if (!cancelled) {
                    setMailboxes(data);
                    const allIds = data.map(mailbox => String(mailbox.id));
                    setAllIds(allIds);

                    const tree = buildTree(data);
                    setTreeData(tree);
                    const itemsWithChildren = getParentIds(tree);
                    setItemsWithChildren(itemsWithChildren);
                    setExpandedItems(itemsWithChildren);
                    const sync_folders = data
                        .filter(mailbox => currentRow.sync_folders.includes(mailbox.name))
                        .map(mailbox => mailbox.id.toString());
                    setSelectedItems(sync_folders);
                    setError(undefined);
                }
            } catch (err: any) {
                if (axios.isAxiosError(err)) {
                    const resData = err.response?.data;
                    if (resData) {
                        setError(`Error ${resData.code || ''}: ${resData.message || ''}`);
                    } else {
                        setError(err.message);
                    }
                }
            } finally {
                if (!cancelled) setIsLoading(false);
            }
        };
        fetchMailboxes();
        return () => {
            cancelled = true;
        };
    }, [currentRow, open]);



    const handleExpandedItemsChange = (
        _event: React.SyntheticEvent | null,
        itemIds: string[],
    ) => {
        setExpandedItems(itemIds);
    };

    const handleExpandClick = () => {
        setExpandedItems((oldExpanded) =>
            oldExpanded.length === 0 ? itemsWithChildren : [],
        );
    };

    const CustomTreeItem = useMemo(() => {
        return React.forwardRef(function CustomTreeItem(
            props: CustomTreeItemProps,
            ref: React.Ref<HTMLLIElement>,
        ) {
            const { id, itemId, label, disabled, children, ...other } = props;

            const {
                getContextProviderProps,
                getRootProps,
                getContentProps,
                getIconContainerProps,
                getCheckboxProps,
                getLabelProps,
                getGroupTransitionProps,
                getDragAndDropOverlayProps,
                status,
            } = useTreeItem({ id, itemId, children, label, disabled, rootRef: ref });

            const item = useTreeItemModel<ExtendedTreeItemProps>(itemId)!;

            return (
                <TreeItemProvider {...getContextProviderProps()}>
                    <TreeItemRoot {...getRootProps(other)}>
                        <TreeItemContent {...getContentProps()}>
                            <TreeItemIconContainer {...getIconContainerProps()}>
                                <TreeItemIcon status={status} />
                            </TreeItemIconContainer>
                            <TreeItemCheckbox {...getCheckboxProps()} sx={{
                                color: 'hsl(var(--muted-foreground) / 0.4)',
                                '&.Mui-checked': {
                                    color: 'hsl(var(--primary))',
                                },
                            }} />
                            <CustomLabel
                                {...getLabelProps({
                                    exists: item.exists,
                                    attributes: item.attributes,
                                    expandable: status.expandable && status.expanded,
                                })}
                            />
                            <TreeItemDragAndDropOverlay {...getDragAndDropOverlayProps()} />
                        </TreeItemContent>
                        {children && <TransitionComponent {...getGroupTransitionProps()} />}
                    </TreeItemRoot>
                </TreeItemProvider>
            );
        });
    }, [theme]);



    const handleSelectAll = useCallback(() => {
        const selectedSet = new Set(allIds);

        const selectedAllMailbox = mailboxes.find((mb) =>
            selectedSet.has(String(mb.id)) &&
            mb.attributes?.some(a => a.attr === 'All')
        );

        if (selectedAllMailbox) {
            toast({
                title: t('accounts.allMailFolderSelected'),
                description: t('accounts.allMailFolderSelectedDesc'),
                action: <ToastAction altText={t('common.ok')}>{t('common.ok')}</ToastAction>,
            });
        }


        setSelectedItems(allIds);
    }, [allIds]);

    const handleDeselectAll = useCallback(() => {
        setSelectedItems([]);
    }, []);


    const updateMutation = useMutation({
        mutationFn: (data: Record<string, any>) => update_account(currentRow?.id ?? '', data),
        onSuccess: handleSuccess,
        onError: handleError
    })

    function handleSuccess() {
        toast({
            title: t('accounts.accountSyncFoldersUpdated'),
            description: t('accounts.accountUpdatedDesc'),
            action: <ToastAction altText={t('common.close')}>{t('common.close')}</ToastAction>,
        });

        queryClient.invalidateQueries({ queryKey: ['account-list'] });
        setIsSubmitting(false);
        onOpenChange(false);
    }

    function handleError(error: AxiosError) {
        const errorMessage = (error.response?.data as { message?: string })?.message ||
            error.message ||
            t('accounts.updateFailed');

        toast({
            variant: "destructive",
            title: t('accounts.accountSyncFoldersUpdateFailed'),
            description: errorMessage as string,
            action: <ToastAction altText={t('common.tryAgain')}>{t('common.tryAgain')}</ToastAction>,
        });
        setIsSubmitting(false);
        console.error(error);
    }

    const handleSelectedItemsChange = (
        _event: React.SyntheticEvent | null,
        newSelectedItems: string[],
    ) => {
        const selectedSet = new Set(newSelectedItems);

        const selectedAllMailbox = mailboxes.find((mb) =>
            selectedSet.has(String(mb.id)) &&
            mb.attributes?.some(a => a.attr === 'All')
        );

        if (selectedAllMailbox) {
            toast({
                title: t('accounts.allMailFolderSelected'),
                description: t('accounts.allMailFolderSelectedDesc'),
                action: <ToastAction altText={t('common.ok')}>{t('common.ok')}</ToastAction>,
            });
        }

        setSelectedItems(newSelectedItems);
    };

    const handleSubmit = async () => {
        if (selectedItems.length === 0) {
            toast({
                title: t('common.error'),
                description: t('accounts.selectAtLeastOneFolder'),
                variant: 'destructive',
            });
            return;
        }
        setIsSubmitting(true);

        const selectedNames: string[] = [];
        const idSet = new Set(selectedItems);
        for (const mailbox of mailboxes) {
            if (idSet.has(String(mailbox.id))) {
                selectedNames.push(mailbox.name);
            }
        }

        updateMutation.mutate({
            sync_folders: selectedNames,
        });
    };

    return (
        <Dialog open={open} onOpenChange={onOpenChange}>
            <DialogContent className="sm:max-w-3xl max-h-[90vh] flex flex-col">
                <DialogHeader className="flex-shrink-0">
                    <DialogTitle>{t('accounts.selectSyncFolders')}</DialogTitle>
                    <DialogDescription>
                        {t('accounts.chooseFoldersToSync', { "email": currentRow.email })}
                    </DialogDescription>
                </DialogHeader>

                <div className="space-y-4">
                    <div className="flex flex-col pt-2 gap-2">
                        <div className="flex gap-2 flex-wrap">
                            <Button
                                variant="outline"
                                size="sm"
                                onClick={handleSelectAll}
                                disabled={isLoading || !allIds || allIds.length === 0}
                                className="h-8"
                            >
                                <CheckSquare className="w-4 h-4 mr-2" />
                                {t('common.selectAll')}
                            </Button>
                            <Button
                                variant="outline"
                                size="sm"
                                onClick={handleDeselectAll}
                                disabled={isLoading || allIds.length === 0}
                                className="h-8"
                            >
                                <Square className="w-4 h-4 mr-2" />
                                {t('common.deselectAll')}
                            </Button>
                            <Button
                                variant="outline"
                                size="sm"
                                onClick={() =>
                                    setSelectionPropagation(prev => ({
                                        ...prev,
                                        descendants: !prev.descendants,
                                    }))
                                }
                                disabled={isLoading}
                                className="h-8"
                            >
                                {selectionPropagation.descendants ? <CheckSquare className="w-4 h-4 mr-2" /> : <Square className="w-4 h-4 mr-2" />}
                                {t('accounts.folderSync.autoSelectDescendants')}
                            </Button>
                            <Button
                                variant="outline"
                                size="sm"
                                onClick={() =>
                                    setSelectionPropagation(prev => ({
                                        ...prev,
                                        parents: !prev.parents,
                                    }))
                                }
                                disabled={isLoading}
                                className="h-8"
                            >
                                {selectionPropagation.parents ? <CheckSquare className="w-4 h-4 mr-2" /> : <Square className="w-4 h-4 mr-2" />}
                                {t('accounts.folderSync.autoSelectParents')}
                            </Button>
                            <Button
                                variant="outline"
                                size="sm"
                                onClick={handleExpandClick}
                                disabled={isLoading}>
                                {expandedItems.length === 0 ? t('accounts.folderSync.expandAll') : t('accounts.folderSync.collapseAll')}
                            </Button>
                        </div>

                        <div className="text-sm text-muted-foreground">
                            {t('accounts.foldersSelected', { count: selectedItems.length })}
                        </div>
                    </div>

                    <ScrollArea className="flex-1 min-h-0 w-full pr-4 -mr-4 py-1">
                        {isLoading && (
                            <div className="p-8 space-y-8">
                                <div className="flex flex-col items-center gap-3 text-muted-foreground">
                                    <Loader2 className="h-6 w-6 animate-spin" />
                                    <span className="text-sm font-medium">{t('accounts.folderSync.loadingMailboxFolders')}</span>
                                </div>

                                <div className="space-y-2">
                                    {[...Array(8)].map((_, i) => (
                                        <Skeleton key={i} className="h-8 w-full" />
                                    ))}
                                </div>
                            </div>
                        )}
                        {!isLoading && (
                            <RichTreeView
                                multiSelect
                                checkboxSelection
                                items={treeData}
                                expandedItems={expandedItems}
                                onExpandedItemsChange={handleExpandedItemsChange}
                                selectionPropagation={selectionPropagation}
                                selectedItems={selectedItems}
                                onSelectedItemsChange={handleSelectedItemsChange}
                                slots={{ item: CustomTreeItem }}
                            />
                        )}
                        {error && (
                            <div className="mt-auto p-2 text-red-600 text-sm font-medium">
                                {error}
                            </div>
                        )}
                    </ScrollArea>
                </div>

                <DialogFooter>
                    <Button
                        variant="outline"
                        onClick={() => onOpenChange(false)}
                        disabled={isSubmitting}
                    >
                        {t('common.cancel')}
                    </Button>
                    <Button
                        onClick={handleSubmit}
                        disabled={isSubmitting || isLoading || !!error}
                    >
                        {isSubmitting && <Loader2 className="mr-2 h-4 w-4 animate-spin" />}
                        {t('common.save')}
                    </Button>
                </DialogFooter>
            </DialogContent>
        </Dialog>
    );
}