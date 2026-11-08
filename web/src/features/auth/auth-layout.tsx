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

import Logo from '@/assets/logo.svg'
import { resolveApiUrl } from '@/api/branding/api'
import { useBranding } from '@/hooks/use-branding'
import { useEdition } from '@/hooks/use-edition'
import { useTranslation } from 'react-i18next'
import { AuthBackground } from './auth-background'
import { BichonMascot } from './bichon-mascot'

type AuthLayoutProps = {
  children: React.ReactNode
}

/// Copyright holder and official site shown in the login footer. Always the
/// product vendor, never the customer's branded company name.
const COPYRIGHT_HOLDER = 'bichon.rustmailer.com'
const HOMEPAGE = 'https://bichon.rustmailer.com'
const COPYRIGHT_START_YEAR = 2025

export function AuthLayout({ children }: AuthLayoutProps) {
  const { companyName, tagline, logoUrl } = useBranding()
  const { isPro } = useEdition()
  const { t } = useTranslation()
  const logo = logoUrl ? resolveApiUrl(logoUrl) : Logo
  const displayName = companyName || (isPro ? 'Bichon Pro' : 'Bichon')
  const currentYear = new Date().getFullYear()
  const copyrightYears =
    currentYear > COPYRIGHT_START_YEAR
      ? `${COPYRIGHT_START_YEAR}-${currentYear}`
      : `${COPYRIGHT_START_YEAR}`
  const description =
    tagline ||
    (isPro
      ? t('common.project_description_pro')
      : t('common.project_description'))

  return (
    <div className='grid min-h-svh lg:grid-cols-2'>
      {/* Form panel */}
      <div className='relative flex flex-col items-center justify-center gap-8 px-6 py-10'>
        <div className='flex flex-col items-center gap-2 lg:hidden'>
          <div className='flex items-center gap-2'>
            <img
              width={36}
              height={36}
              src={logo}
              alt={displayName}
              className='h-9 w-9 object-contain'
            />
            <span className='text-xl font-semibold tracking-tight'>
              {displayName}
            </span>
          </div>
          <p className='text-sm text-muted-foreground'>{description}</p>
        </div>
        <div className='w-full max-w-[400px]'>{children}</div>
      </div>

      {/* Branding panel (desktop only) */}
      <div className='relative hidden flex-col items-center justify-center gap-12 overflow-hidden bg-muted/60 p-10 lg:flex'>
        <div className='pointer-events-none absolute inset-0'>
          <div
            className='absolute inset-0'
            style={{
              background:
                'radial-gradient(60% 50% at 15% 10%, hsl(var(--primary) / 0.14), transparent 60%), radial-gradient(50% 40% at 90% 90%, hsl(var(--primary) / 0.08), transparent 60%)',
            }}
          />
        </div>
        <AuthBackground />

        <div className='relative flex flex-col items-center gap-5 text-center'>
          {/* White-label installs keep their own logo; the default branding
              gets the interactive mascot instead of the static mark. */}
          {logoUrl ? (
            <img
              src={logo}
              alt={displayName}
              className='max-h-40 max-w-40 object-contain'
            />
          ) : (
            <BichonMascot className='h-48 w-48' />
          )}
          <span className='text-3xl font-semibold tracking-tight'>
            {displayName}
          </span>
          <p className='max-w-md text-balance text-lg leading-relaxed text-muted-foreground'>
            {description}
          </p>
        </div>

        <p className='absolute bottom-10 text-sm text-muted-foreground'>
          © {copyrightYears}{' '}
          <a
            href={HOMEPAGE}
            target='_blank'
            rel='noreferrer'
            className='underline-offset-4 transition-colors hover:text-foreground hover:underline'
          >
            {COPYRIGHT_HOLDER}
          </a>
        </p>
      </div>
    </div>
  )
}
