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


import { ThemeSwitch } from "../theme-switch";
import { ProfileDropdown } from "../profile-dropdown";
import { Header } from "./header";
import { NotificationPopover } from "./notification";
import { GithubLinkButton } from "./github";
import { DocsLinkButton } from "./docs";
import { LanguageSwitch } from "../language-switch";

export const FixedHeader = () => {
    return (
        <Header fixed>
            <div className='ml-auto flex items-center space-x-4'>
                <NotificationPopover />
                <GithubLinkButton />
                <DocsLinkButton />
                <LanguageSwitch />
                <ThemeSwitch />
                <ProfileDropdown />
            </div>
        </Header>
    );
};