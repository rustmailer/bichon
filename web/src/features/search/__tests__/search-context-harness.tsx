import * as React from 'react'
import SearchProvider from '../context'

/**
 * Minimal `SearchProvider` for the filter-popover tests: real `filter` state so
 * a click in a popover can be read back, stubs for everything a popover does
 * not touch. The filter is exposed through an `<output>` rather than a spy so
 * the assertions see exactly what the app would send to the API.
 */
export function SearchHarness({
    children,
    initial = {},
}: {
    children: React.ReactNode
    initial?: Record<string, unknown>
}) {
    const [filter, setFilter] = React.useState<Record<string, unknown>>(initial)
    const noop = () => { }

    return (
        <SearchProvider
            value={{
                open: null,
                setOpen: noop,
                currentEnvelope: undefined,
                setCurrentEnvelope: noop,
                toDelete: new Map(),
                setToDelete: noop,
                selected: new Map(),
                setSelected: noop,
                selectAllScope: null,
                setSelectAllScope: noop,
                deleteMailboxId: undefined,
                setDeleteMailboxId: noop,
                selectedAccountId: undefined,
                setSelectedAccountId: noop,
                selectedTags: [],
                sorting: [],
                setSorting: noop,
                filter,
                setFilter,
                refetch: async () => { },
                isFetching: false,
                effectiveSort: 'DATE',
                handleTagToggle: noop,
                editTagsOpen: false,
                setEditTagsOpen: noop,
                dateDisplay: 'absolute',
                setDateDisplay: noop,
            }}
        >
            {children}
            <output data-testid='filter'>{JSON.stringify(filter)}</output>
        </SearchProvider>
    )
}
