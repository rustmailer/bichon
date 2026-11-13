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


import { Avatar, AvatarFallback } from '@/components/ui/avatar'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'

import { useCurrentUser } from '@/hooks/use-current-user';
import useDialogState from '@/hooks/use-dialog-state';

import { useMemo } from 'react';
import { SignOutDialog } from './sign-out-dialog';
import { Link } from '@tanstack/react-router';
import { useTranslation } from 'react-i18next';

export function ProfileDropdown() {
  const [open, setOpen] = useDialogState()
  const { t } = useTranslation()

  const { data: user } = useCurrentUser()

  const avatarSrc = useMemo(() => {
    const base64 = user?.avatar;
    if (!base64 || base64.length === 0) return null;
    return `data:image/png;base64,${base64}`;
  }, [user]);

  const fallbackName = user?.username ? user.username.charAt(0).toUpperCase() : 'U';

  return (
    <>
      <DropdownMenu modal={false}>
        <DropdownMenuTrigger asChild>
          <Button variant="ghost" className="relative h-8 w-8 rounded-full">
            <Avatar className="h-8 w-8">
              {avatarSrc ? (
                <img src={avatarSrc} alt={t('profile.avatar_alt')} className="h-full w-full object-cover" />
              ) : (
                <AvatarFallback className="text-xs">{fallbackName}</AvatarFallback>
              )}
            </Avatar>
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent className='w-56' align='end' forceMount>
          <DropdownMenuLabel className='font-normal'>
            <div className='flex flex-col gap-1.5'>
              <p className='text-xs leading-none font-medium'>{user?.username}</p>
              <p className='text-muted-foreground text-xs leading-none'>
                {user?.email}
              </p>
            </div>
          </DropdownMenuLabel>

          <DropdownMenuSeparator />
          <DropdownMenuGroup>
            <DropdownMenuItem asChild className='text-xs'>
              <Link to='/settings/profile'>{t('profile.menu.profile')}</Link>
            </DropdownMenuItem>
            <DropdownMenuItem asChild className='text-xs'>
              <Link to='/settings'>{t('profile.menu.settings')}</Link>
            </DropdownMenuItem>
          </DropdownMenuGroup>
          <DropdownMenuSeparator />
          <DropdownMenuItem className='text-xs' onClick={() => setOpen(true)}>
            {t('profile.menu.sign_out')}
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
      <SignOutDialog open={!!open} onOpenChange={setOpen} />
    </>
  )
}
