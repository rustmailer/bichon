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



import useMinimalAccountList from "@/hooks/use-minimal-account-list";
import { VirtualizedSelect } from "@/components/virtualized-select";
import { Button } from "@/components/ui/button";
import { useNavigate } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";


interface AccountSwitcherProps {
    onAccountSelect: (accountId: number) => void,
    defaultAccountId?: number,
}

export function AccountSwitcher({
    onAccountSelect,
    defaultAccountId
}: AccountSwitcherProps) {
    const { accountsOptions, isLoading } = useMinimalAccountList();
    const navigate = useNavigate()
    const { t } = useTranslation();
    if (isLoading) {
        return <div>Loading...</div>;
    }

    return (
        <VirtualizedSelect
            className='w-full mr-8'
            isLoading={isLoading}
            options={accountsOptions}
            defaultValue={`${defaultAccountId}`}
            onSelectOption={(values) => onAccountSelect(parseInt(values[0], 10))}
            placeholder={t('oauth2.selectAnAccount')}
            noItemsComponent={<div className='space-y-2'>
                <p>No active email account.</p>
                <Button variant={'outline'} className="py-1 px-3 text-xs" onClick={() => navigate({ to: '/accounts' })}>Add Email Account</Button>
            </div>}
        />
    );
}