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


import { Fragment, useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import { ChevronDown, Loader2 } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { listRepairReports, type BlobRepairRecord } from '@/api/repair/api'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import {
    Table,
    TableBody,
    TableCell,
    TableHead,
    TableHeader,
    TableRow,
} from '@/components/ui/table'
import { cn } from '@/lib/utils'
import { RepairReportView } from './repair-report-view'
import { repairStatusLabel, repairStatusVariant } from './status'

/** The server's ring cap; asking for more is clamped anyway. */
const HISTORY_LIMIT = 200

interface RepairHistoryPanelProps {
    /** Fetch only while the surrounding drawer is open. */
    enabled: boolean
    /** Narrows the history to one account. Omit for every account the caller may repair. */
    accountId?: number
    /** First line of the panel, so the merged history can caption it. */
    showHint?: boolean
}

/**
 * The most recent repair reports, newest first. Read-only — repairs are
 * triggered from the row actions, not from here.
 *
 * The list is already filtered server-side to what the caller may repair, so a
 * non-admin can legitimately see fewer rows than the cap.
 */
export function RepairHistoryPanel({
    enabled,
    accountId,
    showHint = true,
}: RepairHistoryPanelProps) {
    const { t } = useTranslation()
    const [expanded, setExpanded] = useState<Record<string, boolean>>({})

    const { data: records = [], isLoading } = useQuery<BlobRepairRecord[]>({
        queryKey: ['repair-reports', accountId ?? null],
        queryFn: () => listRepairReports(HISTORY_LIMIT, accountId),
        enabled,
    })

    const toggle = (key: string) =>
        setExpanded((prev) => ({ ...prev, [key]: !prev[key] }))

    return (
        <div className='space-y-2'>
            {showHint && (
                <p className='text-xs text-muted-foreground'>
                    {t(
                        'repair_message.historyHint',
                        'The most recent {{limit}} repair reports.',
                        { limit: HISTORY_LIMIT }
                    )}
                </p>
            )}
            <div className='overflow-x-auto rounded-md border'>
                <Table>
                    <TableHeader>
                        <TableRow>
                            <TableHead className='w-10'>
                                <span className='sr-only'>
                                    {t('integrity.expand', 'Expand')}
                                </span>
                            </TableHead>
                            <TableHead className='text-xs'>
                                {t('repair_message.time', 'Time')}
                            </TableHead>
                            <TableHead className='text-xs'>
                                {t('mail.subject', 'Subject')}
                            </TableHead>
                            <TableHead className='text-xs'>
                                {t('repair_message.operator', 'Operator')}
                            </TableHead>
                            <TableHead className='text-xs'>
                                {t('repair_message.account', 'Account')}
                            </TableHead>
                            <TableHead className='text-xs'>
                                {t('repair_message.status', 'Result')}
                            </TableHead>
                        </TableRow>
                    </TableHeader>
                    <TableBody>
                        {isLoading ? (
                            <TableRow>
                                <TableCell colSpan={6} className='py-8 text-center'>
                                    <Loader2 className='mx-auto h-4 w-4 animate-spin text-muted-foreground' />
                                </TableCell>
                            </TableRow>
                        ) : records.length === 0 ? (
                            <TableRow>
                                <TableCell
                                    colSpan={6}
                                    className='py-8 text-center text-muted-foreground'
                                >
                                    {t('repair_message.historyEmpty', 'No repair reports yet.')}
                                </TableCell>
                            </TableRow>
                        ) : (
                            records.map((record) => {
                                const isOpen = !!expanded[record.id]
                                return (
                                    <Fragment key={record.id}>
                                        <TableRow>
                                            <TableCell className='w-10'>
                                                <Button
                                                    variant='ghost'
                                                    size='icon'
                                                    className='h-7 w-7'
                                                    aria-expanded={isOpen}
                                                    aria-label={
                                                        isOpen
                                                            ? t('integrity.collapse', 'Collapse')
                                                            : t('integrity.expand', 'Expand')
                                                    }
                                                    onClick={() => toggle(record.id)}
                                                >
                                                    <ChevronDown
                                                        className={cn(
                                                            'h-4 w-4 transition-transform',
                                                            isOpen && 'rotate-180'
                                                        )}
                                                    />
                                                </Button>
                                            </TableCell>
                                            <TableCell className='whitespace-nowrap text-xs'>
                                                {new Date(record.created_at).toLocaleString()}
                                            </TableCell>
                                            <TableCell className='max-w-md truncate text-xs'>
                                                {record.subject || '—'}
                                            </TableCell>
                                            <TableCell className='text-xs'>
                                                {record.operator}
                                            </TableCell>
                                            <TableCell className='text-xs text-muted-foreground'>
                                                {record.account_id}
                                            </TableCell>
                                            <TableCell>
                                                <Badge variant={repairStatusVariant(record.report.status)}>
                                                    {repairStatusLabel(t, record.report.status)}
                                                </Badge>
                                            </TableCell>
                                        </TableRow>
                                        {isOpen && (
                                            <TableRow className='bg-muted/40'>
                                                <TableCell colSpan={6} className='p-4'>
                                                    <RepairReportView report={record.report} />
                                                </TableCell>
                                            </TableRow>
                                        )}
                                    </Fragment>
                                )
                            })
                        )}
                    </TableBody>
                </Table>
            </div>
        </div>
    )
}
