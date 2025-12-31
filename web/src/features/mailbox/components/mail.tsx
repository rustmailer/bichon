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


import * as React from "react"
import { cn } from "@/lib/utils"
import {
    ResizableHandle,
    ResizablePanel,
    ResizablePanelGroup,
} from "@/components/ui/resizable"
import { Separator } from "@/components/ui/separator"
import { TooltipProvider } from "@/components/ui/tooltip"
import { AccountSwitcher } from "./account-switcher"
import { ScrollArea } from "@/components/ui/scroll-area"
import { list_mailboxes, MailboxData } from "@/api/mailbox/api"
import { useQuery } from "@tanstack/react-query"
import { Skeleton } from "@/components/ui/skeleton"
import MailboxProvider, { MailboxDialogType } from "../context"
import useDialogState from "@/hooks/use-dialog-state"
import { MailboxDialog } from "./mailbox-detail"
import { MailList } from "./mail-list"
import { list_messages } from "@/api/mailbox/envelope/api"
import { MailDisplayDrawer } from "./mail-display-drawer"
import { toast } from "@/hooks/use-toast"
import { EnvelopeDeleteDialog } from "./delete-dialog"
import Logo from '@/assets/logo.svg'
import { EmailEnvelope } from "@/api"
import { EnvelopeListPagination } from "@/components/pagination"
import { RichTreeView, TreeItemCheckbox, TreeItemContent, TreeItemDragAndDropOverlay, TreeItemIcon, TreeItemIconContainer, TreeItemLabel, TreeItemProvider, TreeItemRoot, useTreeItem, useTreeItemModel, UseTreeItemParameters } from "@mui/x-tree-view"
import { buildTree, ExtendedTreeItemProps } from "@/lib/build-tree"
import { useTheme } from "@/context/theme-context"
import { styled } from "@mui/material/styles"
import { animated, useSpring } from "@react-spring/web"
import { TransitionProps } from "@mui/material/transitions"
import Collapse from "@mui/material/Collapse"
import { FolderIcon } from "lucide-react"
import { RestoreMessageDialog } from "./restore-message-dialog"


interface MailProps {
    defaultLayout: number[] | undefined
    defaultCollapsed?: boolean
    navCollapsedSize: number,
    lastSelectedAccountId?: number | undefined
}

interface ListMessagesOptions {
    accountId: number | undefined;
    mailboxId: number | undefined;
    page: number;
    page_size: number;
}

const useListMessages = ({ accountId, mailboxId, page, page_size }: ListMessagesOptions) => {
    return useQuery({
        queryKey: ['mailbox-list-messages', `${accountId}`, mailboxId, page, page_size],
        queryFn: () => {
            return list_messages(accountId!, mailboxId!, page, page_size);
        },
        enabled: !!accountId && !!mailboxId,
        retry: 0,
        staleTime: 1000,
    });
};




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
            <FolderIcon className="mr-2" />
            <span className="font-medium text-sm text-inherit">
                {children}
            </span>
            {/* <div className="flex gap-2 ml-auto mr-3 opacity-70 text-xs">
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
            )} */}
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



export function Mail({
    defaultLayout = [20, 80],
    defaultCollapsed = false,
    navCollapsedSize,
    lastSelectedAccountId,
}: MailProps) {
    const [open, setOpen] = useDialogState<MailboxDialogType>(null)
    const [isCollapsed, setIsCollapsed] = React.useState(defaultCollapsed)
    const [selectedMailbox, setSelectedMailbox] = React.useState<MailboxData | undefined>(undefined);
    const [selectedAccountId, setSelectedAccountId] = React.useState<number | undefined>(lastSelectedAccountId);
    const [selectedEvelope, setSelectedEvelope] = React.useState<EmailEnvelope | undefined>(undefined);
    const [page, setPage] = React.useState(0);
    const [pageSize, setPageSize] = React.useState(30);
    const [deleteIds, setDeleteIds] = React.useState<Set<number>>(() => new Set());
    const [selected, setSelected] = React.useState<Set<number>>(() => new Set());
    const { theme } = useTheme()

    const { data: mailboxes, isLoading: isMailboxesLoading } = useQuery({
        queryKey: ['account-mailboxes', `${selectedAccountId}`],
        queryFn: () => list_mailboxes(selectedAccountId!, false),
        enabled: !!selectedAccountId,
    })


    const tree = buildTree(mailboxes ?? []);

    const { data: envelopes, isLoading: isMessagesLoading, isError, error } = useListMessages({
        accountId: selectedAccountId,
        mailboxId: selectedMailbox?.id,
        page: page + 1,
        page_size: pageSize
    });

    const hasNextPage = () => {
        return page + 1 < envelopes?.total_pages!;
    }

    const handlePageChange = (newPage: number) => {
        setPage(newPage);
    }


    const handlePageSizeChange = (newSize: number) => {
        setPage(0);
        setPageSize(newSize);
    }

    React.useEffect(() => {
        if (isError && error) {
            toast({
                variant: "destructive",
                title: "Failed to load messages",
                description: error.message || "An unknown error occurred. Please try again.",
            });
        }
    }, [isError, error]);

    const handleItemSelectionToggle = (
        _event: React.SyntheticEvent | null,
        itemId: string,
        isSelected: boolean,
    ) => {
        if (isSelected) {
            setSelectedMailbox(mailboxes?.find(m => String(m.id) === itemId))
            setPage(0);
        }
    };

    const CustomTreeItem = React.useMemo(() => {
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
                            <TreeItemCheckbox {...getCheckboxProps()} />
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




    return (
        <MailboxProvider value={{ open, setOpen, currentMailbox: selectedMailbox, selectedAccountId, setCurrentMailbox: setSelectedMailbox, currentEnvelope: selectedEvelope, setCurrentEnvelope: setSelectedEvelope, deleteIds, setDeleteIds, selected, setSelected }}>
            <TooltipProvider delayDuration={0}>
                <ResizablePanelGroup
                    direction="horizontal"
                    onLayout={(sizes: number[]) => {
                        localStorage.setItem('react-resizable-panels:layout:mail', JSON.stringify(sizes));
                    }}
                    className="items-stretch"
                >
                    <ResizablePanel
                        defaultSize={defaultLayout[0]}
                        collapsedSize={navCollapsedSize}
                        minSize={navCollapsedSize}
                        collapsible={true}
                        onCollapse={() => {
                            setIsCollapsed(true);
                            localStorage.setItem('react-resizable-panels:collapsed', JSON.stringify(true));
                        }}
                        onResize={() => {
                            setIsCollapsed(false);
                            localStorage.setItem('react-resizable-panels:collapsed', JSON.stringify(false));
                        }}
                        className={cn(
                            isCollapsed &&
                            "min-w-[50px] transition-all duration-300 ease-in-out"
                        )}
                    >
                        <Separator className="mb-2" />
                        <ScrollArea className='h-[50rem] w-full pr-4 -mr-4 py-1'>
                            <div>
                                <AccountSwitcher onAccountSelect={(accountId) => {
                                    localStorage.setItem('mailbox:selectedAccountId', `${accountId}`);
                                    setSelectedAccountId(accountId);
                                    setSelectedMailbox(undefined);
                                }} defaultAccountId={lastSelectedAccountId} />
                            </div>
                            <Separator className="mt-2" />
                            {isMailboxesLoading ? (
                                <div className="space-y-2 p-4">
                                    {Array.from({ length: 5 }).map((_, index) => (
                                        <div key={index} className="space-y-2">
                                            <div className="flex items-center space-x-2">
                                                <Skeleton className="h-4 w-4 rounded-full" />
                                                <Skeleton className="h-4 w-[200px]" />
                                            </div>
                                            <div className="pl-6 space-y-2">
                                                {Array.from({ length: 3 }).map((_, subIndex) => (
                                                    <div key={subIndex} className="flex items-center space-x-2">
                                                        <Skeleton className="h-4 w-4 rounded-full" />
                                                        <Skeleton className="h-4 w-[150px]" />
                                                    </div>
                                                ))}
                                            </div>
                                        </div>
                                    ))}
                                </div>
                            ) : (
                                <RichTreeView
                                    //checkboxSelection
                                    items={tree}
                                    onItemSelectionToggle={handleItemSelectionToggle}
                                    slots={{ item: CustomTreeItem }}
                                />
                            )}
                        </ScrollArea>
                    </ResizablePanel>
                    <ResizableHandle withHandle className="h-[calc(100vh-7rem)]" />
                    <ResizablePanel defaultSize={defaultLayout[1]}>
                        {selectedMailbox && <div>
                            <Separator />
                            <div className="flex items-center px-4 py-2">
                                <h2 className="text-xl font-bold cursor-pointer hover:underline" onClick={() => setOpen("mailbox")}>
                                    {selectedMailbox?.name}
                                </h2>
                            </div>
                            <Separator />
                            <div className="mt-2">
                                <ScrollArea className='h-[40rem] w-full pr-4 -mr-4 py-1'>
                                    <MailList
                                        isLoading={isMessagesLoading}
                                        items={(envelopes?.items ?? []).sort((a, b) => {
                                            const dateA = a.date;
                                            const dateB = b.date;
                                            return dateB - dateA;
                                        })}
                                    />
                                </ScrollArea>
                                {selectedMailbox && <div className="flex justify-center mt-4">
                                    <EnvelopeListPagination
                                        totalItems={envelopes?.total_items ?? 0}
                                        hasNextPage={hasNextPage}
                                        pageIndex={page}
                                        pageSize={pageSize}
                                        setPageIndex={handlePageChange}
                                        setPageSize={handlePageSizeChange}
                                    />
                                </div>}
                            </div>
                        </div>}
                        {!selectedMailbox && <div className="flex h-[750px] shrink-0 items-center justify-center rounded-md border border-dashed">
                            <div className="mx-auto flex max-w-[420px] flex-col items-center justify-center text-center">
                                <img
                                    src={Logo}
                                    className='mb-6 opacity-20 saturate-0 transition-all duration-300 hover:opacity-100 hover:saturate-100'
                                    width={350}
                                    height={350}
                                    alt='Bichon Logo'
                                />
                            </div>
                        </div>
                        }
                    </ResizablePanel>
                </ResizablePanelGroup>
            </TooltipProvider>
            <MailboxDialog
                key='mailbox-detail'
                open={open === 'mailbox'}
                onOpenChange={() => setOpen('mailbox')}
            />
            <MailDisplayDrawer
                key='mail-display'
                open={open === 'display'}
                onOpenChange={() => setOpen('display')}
            />
            <EnvelopeDeleteDialog
                key='envelope-move-to-trash'
                open={open === 'move-to-trash'}
                onOpenChange={() => setOpen('move-to-trash')}
            />

            <RestoreMessageDialog
                key='envelope-restore'
                open={open === 'restore'}
                onOpenChange={() => setOpen('restore')}
            />

        </MailboxProvider >
    )
}