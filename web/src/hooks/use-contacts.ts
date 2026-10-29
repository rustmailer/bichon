import { useMemo } from 'react';
import { useQuery } from '@tanstack/react-query';
import { Contact, get_contacts } from '@/api/search/api';

export const useContacts = (searchTerm: string = "") => {
    const { data: allContacts = [], isLoading, isError } = useQuery({
        queryKey: ['contacts', 'all'],
        queryFn: get_contacts,
        staleTime: 1000 * 60 * 10,
        gcTime: 1000 * 60 * 30,
    });

    const filtered = useMemo(() => {
        const term = searchTerm.trim().toLowerCase();
        if (!term) return allContacts;
        // Match the display name too: users look for "Alice", not necessarily
        // the address behind it.
        return allContacts.filter((contact: Contact) =>
            contact.email.toLowerCase().includes(term) ||
            (contact.name ?? '').toLowerCase().includes(term)
        );
    }, [allContacts, searchTerm]);

    return {
        contacts: filtered,
        isLoading,
        isError
    };
};
