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
import { useMemo } from 'react'
import { useQuery } from '@tanstack/react-query'
import { BellIcon, Loader2, ExternalLinkIcon } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import ReactMarkdown from 'react-markdown'
import remarkGfm from 'remark-gfm'
import { get_notifications, SystemAlert } from '@/api/system/api'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from '@/components/ui/popover'
import { ScrollArea } from '@/components/ui/scroll-area'

interface Release {
  tag_name: string
  published_at: string
  body: string
  html_url: string
}

interface BaseNotification {
  type: string
}

interface ReleaseNotification extends BaseNotification {
  type: 'new-release'
  data: Release
}

interface SystemAlertNotification extends BaseNotification {
  type: 'system-alert'
  data: SystemAlert
}

type ActiveNotification = ReleaseNotification | SystemAlertNotification

export function NotificationPopover() {
  const { data, isLoading } = useQuery({
    queryKey: ['system-notifications'],
    queryFn: get_notifications,
    staleTime: 1000 * 60 * 30, // 30 minutes
    // Alerts can appear/disappear between sessions (e.g. a backup warning
    // was cleared elsewhere); always refetch on mount.
    refetchOnMount: 'always',
  })

  const { t } = useTranslation()

  const activeNotifications = useMemo((): ActiveNotification[] => {
    if (!data) return []

    const notifications: ActiveNotification[] = []

    if (data.release.is_newer && data.release.latest) {
      notifications.push({
        type: 'new-release',
        data: {
          tag_name: data.release.latest.tag_name,
          published_at: data.release.latest.published_at,
          body: data.release.latest.body,
          html_url: data.release.latest.html_url,
        },
      })
    }
    if (data.alerts && data.alerts.length > 0) {
      data.alerts.forEach((alert) => {
        notifications.push({ type: 'system-alert', data: alert })
      })
    }
    return notifications
  }, [data])

  const showNotificationBadge = activeNotifications.length > 0

  return (
    <Popover>
      <PopoverTrigger asChild>
        <Button
          variant='ghost'
          size='icon'
          className='relative'
          disabled={isLoading}
        >
          {isLoading ? (
            <Loader2 className='h-5 w-5 animate-spin' />
          ) : (
            <>
              <BellIcon className='h-5 w-5' />
              {showNotificationBadge && (
                <Badge
                  variant='default'
                  className='absolute -right-1 -top-1 h-5 w-5 rounded-full p-0 flex items-center justify-center'
                >
                  {activeNotifications.length}
                </Badge>
              )}
            </>
          )}
        </Button>
      </PopoverTrigger>
      <PopoverContent className='w-[32rem] p-0' align='end'>
        <div className='p-4 border-b'>
          <h4 className='font-medium text-sm'>
            {t('system.notifications')}
            {showNotificationBadge && ` (${activeNotifications.length})`}
          </h4>
        </div>
        <ScrollArea className='h-72'>
          {isLoading ? (
            <div className='flex items-center justify-center p-8'>
              <Loader2 className='h-6 w-6 animate-spin text-muted-foreground' />
            </div>
          ) : activeNotifications.length === 0 ? (
            <div className='p-8 text-center space-y-2'>
              <BellIcon className='mx-auto h-6 w-6 text-muted-foreground' />
              <p className='text-sm text-muted-foreground'>
                No new notifications
              </p>
            </div>
          ) : (
            <div className='divide-y'>
              {activeNotifications.map((notification, index) => (
                <div key={index} className='p-4'>
                  {notification.type === 'system-alert' ? (
                    <SystemAlertView data={notification.data} />
                  ) : (
                    <ReleaseNotificationView data={notification.data} />
                  )}
                </div>
              ))}
            </div>
          )}
        </ScrollArea>
      </PopoverContent>
    </Popover>
  )
}

function ReleaseNotificationView({ data }: { data: Release }) {
  return (
    <div className='space-y-4'>
      <div className='space-y-2'>
        <div className='flex items-center justify-between'>
          <h3 className='text-sm font-semibold'>{data.tag_name}</h3>
          <span className='text-xs bg-green-100 text-green-800 px-2 py-1 rounded-full'>
            New Release
          </span>
        </div>
        <p className='text-xs text-muted-foreground'>
          Released {data.published_at}
        </p>
      </div>

      <div className='prose prose-xs dark:prose-invert max-w-none text-xs'>
        <ReactMarkdown remarkPlugins={[remarkGfm]}>{data.body}</ReactMarkdown>
      </div>

      {data.html_url && (
        <div className='pt-2'>
          <a
            href={data.html_url}
            target='_blank'
            rel='noopener noreferrer'
            className='text-xs text-primary hover:underline inline-flex items-center'
          >
            View full release notes{' '}
            <ExternalLinkIcon className='ml-1 h-3 w-3' />
          </a>
        </div>
      )}
    </div>
  )
}

function SystemAlertView({ data }: { data: SystemAlert }) {
  const { t } = useTranslation()
  const isError = data.severity === 'error'
  return (
    <div className='space-y-2'>
      <div className='flex items-center justify-between gap-2'>
        <h3 className='text-sm font-semibold'>{data.title}</h3>
        <span
          className={`shrink-0 text-xs px-2 py-1 rounded-full ${
            isError
              ? 'bg-red-100 text-red-800 dark:bg-red-900/50 dark:text-red-200'
              : 'bg-amber-100 text-amber-800 dark:bg-amber-900/50 dark:text-amber-200'
          }`}
        >
          {isError
            ? t('system.alertError', 'Error')
            : t('system.alertWarning', 'Warning')}
        </span>
      </div>
      <p className='text-xs text-muted-foreground whitespace-pre-wrap break-words'>
        {data.message}
      </p>
    </div>
  )
}
