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


import { Card } from "@/components/ui/card";
import Logo from '@/assets/logo.svg'
import { useTranslation } from 'react-i18next'

export default function APIDocs() {
  const { t } = useTranslation()

  const rawBase: string = (window as any).__BICHON_BASE__ || '';
  const normalized = rawBase.replace(/\/+$/, '');
  const base_url = normalized.startsWith('http')
    ? normalized
    : `${window.location.origin}${normalized}`;

  const docsOptions = [
    { name: t('apiDocs.swaggerUI'), path: `${base_url}/api-docs/swagger` },
    { name: t('apiDocs.reDoc'), path: `${base_url}/api-docs/redoc` },
    { name: t('apiDocs.openAPIExplorer'), path: `${base_url}/api-docs/explorer` },
    { name: t('apiDocs.scalar'), path: `${base_url}/api-docs/scalar` },
    { name: t('apiDocs.downloadSpecYAML'), path: `${base_url}/api-docs/spec.yaml` }
  ];
  const handleCardClick = (path: string) => {
    // Open in new tab
    window.open(path, '_blank', 'noopener,noreferrer');
  };

  return (
    <>
      <div className='w-full'>
        <div className='-mx-4 flex-1 overflow-auto px-4 py-1 flex-row lg:space-x-12 space-y-0'>
          <div className='m-auto flex h-full w-full flex-col items-center justify-center gap-6 p-4'>
            <div className="grid w-full gap-4 sm:grid-cols-1 md:grid-cols-2 xl:max-w-4xl">
              {docsOptions.map((option) => (
                <Card
                  key={option.name}
                  className="cursor-pointer p-6 transition-all hover:bg-accent hover:text-accent-foreground hover:shadow-md"
                  onClick={() => handleCardClick(option.path)}
                >
                  <div className="flex items-center gap-4">
                    <img
                      src={Logo}
                      className="max-h-[66px] w-auto opacity-20 saturate-0 transition-all duration-300 hover:opacity-100 hover:saturate-100 object-contain"
                      alt="Bichon Logo"
                    />
                    <h3 className="text-sm font-medium">{option.name}</h3>
                  </div>
                </Card>
              ))}
            </div>
          </div>
        </div>
      </div>
    </>
  );
}