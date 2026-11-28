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


import React from 'react'
import { cn } from '@/lib/utils'
import { useIsTopNav } from '@/context/nav-layout-context'

interface MainProps extends React.HTMLAttributes<HTMLElement> {
  fixed?: boolean;
  higher?: boolean;
  ref?: React.Ref<HTMLElement>;
}

export const Main = ({ fixed, higher, ...props }: MainProps) => {
  // The top bar replaces the fixed page header, so its 16 (or 12) unit
  // clearance above the content is no longer needed.
  const topNav = useIsTopNav();
  return (
    <main
      className={cn(
        'peer-[.header-fixed]/header',
        topNav ? 'mt-0' : higher ? 'mt-12' : 'mt-16', // Conditional class for 'higher'
        'px-4 py-6',
        fixed && 'fixed-main flex flex-col flex-grow overflow-hidden'
      )}
      {...props}
    />
  );
};

Main.displayName = 'Main';

