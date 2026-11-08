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

import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from '@/components/ui/card'
import { UserAuthForm } from './user-auth-form'
import { useTranslation } from 'react-i18next'
import { AuthLayout } from './auth-layout'

export default function SignIn() {
  const { t } = useTranslation()

  return (
    <AuthLayout>
      <Card className='overflow-hidden rounded-2xl border-border/60 shadow-xl shadow-black/5 dark:shadow-black/20'>
        <CardHeader className='space-y-2 p-8 pb-6'>
          <CardTitle className='text-2xl tracking-tight'>
            {t('auth.signInTitle', 'Sign in to your account')}
          </CardTitle>
          <CardDescription className='leading-relaxed'>
            {t('auth.signInDescription', 'Enter your credentials to continue.')}
          </CardDescription>
        </CardHeader>
        <CardContent className='p-8 pt-0'>
          <UserAuthForm />
        </CardContent>
      </Card>
    </AuthLayout>
  )
}
