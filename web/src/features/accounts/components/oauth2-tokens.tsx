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


import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { AccountModel } from '../data/schema'
import { Button } from '@/components/ui/button'
import { get_oauth2_tokens } from '@/api/oauth2/api'
import { useQuery } from '@tanstack/react-query'
import { Card, CardContent } from '@/components/ui/card'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { TableSkeleton } from '@/components/table-skeleton'
import { useTranslation } from 'react-i18next'
import { FileIcon } from 'lucide-react'
import { format, formatDistanceToNow } from 'date-fns'
import LongText from '@/components/long-text'
import { useCallback } from 'react'
import { IconCopy } from '@tabler/icons-react'
import { toast } from '@/hooks/use-toast'
import { ToastAction } from '@/components/ui/toast'
import { useNavigate } from '@tanstack/react-router'
import { dateFnsLocaleMap } from '@/lib/utils'
import { enUS } from 'date-fns/locale'

interface Props {
  currentRow: AccountModel
  open: boolean
  onOpenChange: (open: boolean) => void
}

export function OAuth2TokensDialog({ currentRow, open, onOpenChange }: Props) {
  const { t, i18n } = useTranslation()
  const locale = dateFnsLocaleMap[i18n.language.toLowerCase()] ?? enUS;
  const navigate = useNavigate()
  const { data: oauth2Tokens, isLoading } = useQuery({
    queryKey: ['oauth2-tokens', currentRow.id],
    queryFn: () => get_oauth2_tokens(currentRow.id),
    enabled: open && !!currentRow.id,
    retry: 0,
    refetchOnWindowFocus: false,
    refetchOnMount: false,
  })


  const onCopy = useCallback(async (access: boolean, token: string) => {
    try {
      await navigator.clipboard.writeText(token);
      if (access) {
        toast({
          title: t('common.ok'),
          description: t('accounts.accessTokenCopiedToClipboard'),
        });
      } else {
        toast({
          title: t('common.ok'),
          description: t('accounts.refreshTokenCopiedToClipboard'),
        });
      }
    } catch (err) {
      toast({
        variant: "destructive",
        title: t('settings.failedToCopyText'),
        description: (err as Error).message,
        action: <ToastAction altText={t('common.tryAgain')}>{t('common.tryAgain')}</ToastAction>,
      });
    }
  }, []);

  return (
    <Dialog
      open={open}
      onOpenChange={(state) => {
        onOpenChange(state)
      }}
    >
      <DialogContent className='sm:max-w-3xl'>
        <DialogHeader className='text-left'>
          <DialogTitle>{t('accounts.oauth2Tokens')}</DialogTitle>
          <DialogDescription>
            {t('accounts.detailsOfTheOAuth2TokensForTheAccount')}
          </DialogDescription>
        </DialogHeader>
        <Card>
          <CardContent>
            {isLoading ? (
              <TableSkeleton columns={2} rows={10} />
            ) : oauth2Tokens ? (
              <Table className='w-full'>
                <TableHeader>
                  <TableRow>
                    <TableHead>{t('accounts.field')}</TableHead>
                    <TableHead>{t('accounts.value')}</TableHead>
                    <TableHead>{t('common.actions')}</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  <TableRow>
                    <TableCell className='max-w-80'>{t('oauth2.id')}</TableCell>
                    <TableCell>
                      <LongText className='max-w-[240px] sm:max-w-[430px]'>{oauth2Tokens.oauth2_id}</LongText>
                    </TableCell>
                  </TableRow>
                  <TableRow>
                    <TableCell className='max-w-80'>{t('accessTokens.token')}</TableCell>
                    <TableCell>
                      <LongText className='max-w-[240px] sm:max-w-[430px]'>{oauth2Tokens.access_token}</LongText>
                    </TableCell>
                    <TableCell>
                      <Button className='text-xs px-1.5 py-0.5' onClick={() => onCopy(true, oauth2Tokens.access_token)}>
                        <IconCopy className="h-5 w-5" aria-hidden="true" />
                      </Button>
                    </TableCell>
                  </TableRow>
                  <TableRow>
                    <TableCell className='max-w-80'>{t('accounts.refreshToken')}</TableCell>
                    <TableCell>
                      <LongText className='max-w-[240px] sm:max-w-[430px]'>{oauth2Tokens.refresh_token}</LongText>
                    </TableCell>
                    <TableCell>
                      <Button className='text-xs px-1.5 py-0.5' onClick={() => onCopy(false, oauth2Tokens.refresh_token)}>
                        <IconCopy className="h-5 w-5" aria-hidden="true" />
                      </Button>
                    </TableCell>
                  </TableRow>
                  <TableRow>
                    <TableCell className='max-w-80'>{t('settings.createdAt')}</TableCell>
                    <TableCell>
                      {format(new Date(oauth2Tokens.created_at), 'yyyy-MM-dd HH:mm:ss')}
                    </TableCell>
                  </TableRow>
                  <TableRow>
                    <TableCell className='max-w-80'>{t('settings.updatedAt')}</TableCell>
                    <TableCell>
                      {formatDistanceToNow(new Date(oauth2Tokens.updated_at), { addSuffix: true, locale })}
                    </TableCell>
                  </TableRow>
                </TableBody>
              </Table>
            ) : (
              <div className="flex h-[250px] mt-4 shrink-0 items-center justify-center rounded-md border border-dashed">
                <div className="mx-auto flex max-w-[420px] flex-col items-center justify-center text-center">
                  <FileIcon className="h-10 w-10 text-muted-foreground" />
                  <h3 className="mt-4 text-lg font-semibold">{t('accounts.noOAuth2Tokens')}</h3>
                  <p className="mb-4 mt-2 text-sm text-muted-foreground">
                    {t('accounts.theAccountHasNotCompletedTheAuthorizationProcess')}
                    <a onClick={() => navigate({ to: '/oauth2' })} className="ml-1 text-blue-500 underline cursor-pointer">{t('accounts.clickHere')}</a>
                    {t('accounts.toAuthorizeTheAccount')}
                  </p>
                </div>
              </div>
            )}
          </CardContent>
        </Card>
        <DialogFooter>
          <DialogClose asChild>
            <Button variant='outline' className="px-2 py-1 text-sm h-auto">{t('common.close')}</Button>
          </DialogClose>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
