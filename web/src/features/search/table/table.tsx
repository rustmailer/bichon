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


import { useState, MouseEvent as ReactMouseEvent, useEffect } from 'react'
import {
  ColumnDef,
  ColumnFiltersState,
  Row,
  RowData,
  flexRender,
  getCoreRowModel,
  getFacetedRowModel,
  getFacetedUniqueValues,
  getFilteredRowModel,
  getSortedRowModel,
  useReactTable,
} from '@tanstack/react-table'
import { type Table } from '@tanstack/react-table'
import {
  Table as ShadcnTable,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import { useTranslation } from 'react-i18next'
import { EmailEnvelope } from '@/api'
import { cn } from '@/lib/utils'
import { useSearchContext } from '../context'
import { ScrollArea } from '@/components/ui/scroll-area'



declare module '@tanstack/react-table' {
  // eslint-disable-next-line @typescript-eslint/no-unused-vars
  interface ColumnMeta<TData extends RowData, TValue> {
    className: string
  }
}

interface DataTableProps {
  columns: ColumnDef<EmailEnvelope>[]
  data: EmailEnvelope[]
  onRowClick: (e: ReactMouseEvent<HTMLTableRowElement, MouseEvent>, row: Row<EmailEnvelope>) => void
  setSortBy: (sortBy: "DATE" | "SIZE" | "RELEVANCE") => void
  setSortOrder: (value: "desc" | "asc") => void
  children?: (table: Table<EmailEnvelope>) => React.ReactNode
}

export function SearchTable({ columns, data, onRowClick, setSortBy, setSortOrder, children }: DataTableProps) {
  const { sorting, setSorting } = useSearchContext()
  const { t } = useTranslation()
  const [rowSelection, setRowSelection] = useState({})
  const [columnFilters, setColumnFilters] = useState<ColumnFiltersState>([])

  useEffect(() => {
    // With no column sorted we leave the URL sort params unset, so the
    // backend default applies (RELEVANCE when the query has a text term,
    // DATE otherwise).
    const [value] = sorting
    if (!value) return
    setSortBy(value.id.toUpperCase() as "DATE" | "SIZE")
    setSortOrder(value.desc ? "desc" : "asc")
  }, [sorting])

  const table = useReactTable({
    data,
    columns,
    state: {
      sorting,
      rowSelection,
      columnFilters,
    },
    enableRowSelection: true,
    onRowSelectionChange: setRowSelection,
    onSortingChange: setSorting,
    onColumnFiltersChange: setColumnFilters,
    getCoreRowModel: getCoreRowModel(),
    getFilteredRowModel: getFilteredRowModel(),
    getSortedRowModel: getSortedRowModel(),
    getFacetedRowModel: getFacetedRowModel(),
    getFacetedUniqueValues: getFacetedUniqueValues(),
  })

  return (
    <div className="flex flex-1 flex-col gap-0.5">
      {children && (<>{children(table)}</>)}
      <ScrollArea className='h-[calc(100vh-18rem)] rounded-md border' orientation='both'>
        <ShadcnTable className='text-xs'>
          <TableHeader>
            {table.getHeaderGroups().map((headerGroup) => (
              <TableRow key={headerGroup.id} className='group/row h-7'>
                {headerGroup.headers.map((header) => {
                  return (
                    <TableHead
                      key={header.id}
                      colSpan={header.colSpan}
                      className={cn('h-7 py-1', header.column.columnDef.meta?.className ?? '')}
                    >
                      {header.isPlaceholder
                        ? null
                        : flexRender(
                          header.column.columnDef.header,
                          header.getContext()
                        )}
                    </TableHead>
                  )
                })}
              </TableRow>
            ))}
          </TableHeader>
          <TableBody>
            {table.getRowModel().rows?.length ? (
              table.getRowModel().rows.map((row) => (
                <TableRow
                  key={row.id}
                  data-state={row.getIsSelected() && 'selected'}
                  className="group/row cursor-pointer transition-colors hover:bg-accent/50 h-7"
                  onClick={(e) => onRowClick(e, row)}
                >
                  {row.getVisibleCells().map((cell) => (
                    <TableCell
                      key={cell.id}
                      className={cn('py-1 px-2', cell.column.columnDef.meta?.className ?? '')}
                      style={{
                        width: cell.column.columnDef.size,
                        minWidth: cell.column.columnDef.minSize,
                        maxWidth: cell.column.columnDef.maxSize
                      }}
                    >
                      {flexRender(
                        cell.column.columnDef.cell,
                        cell.getContext()
                      )}
                    </TableCell>
                  ))}
                </TableRow>
              ))
            ) : (
              <TableRow>
                <TableCell
                  colSpan={columns.length}
                  className='h-24 text-center'
                >
                  {t('common.table.noResults')}
                </TableCell>
              </TableRow>
            )}
          </TableBody>
        </ShadcnTable>
      </ScrollArea>
    </div>
  )
}
