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
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <http://www.gnu.org/licenses/>.


import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs';
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table';
import { Skeleton } from '@/components/ui/skeleton';
import { XAxis, YAxis, CartesianGrid, Tooltip, ResponsiveContainer, PieChart, Pie, Cell, BarChart, Bar } from 'recharts';
import { Mail, HardDrive, Database, Users, Inbox, Info } from 'lucide-react';
import { formatBytes, formatNumber } from '@/lib/utils';
import { useQuery } from '@tanstack/react-query';
import { get_dashboard_stats, TimeBucket } from '@/api/system/api';
import { Main } from '@/components/layout/main';
import { FixedHeader } from '@/components/layout/fixed-header';
import { useTranslation } from 'react-i18next';
import { getToken } from '@/stores/authStore';

interface DailyActivity {
  date: string;
  count: number;
}

function convertRecentActivity(timeBuckets: TimeBucket[], locale: string): DailyActivity[] {
  const dateFormatter = new Intl.DateTimeFormat(locale, {
    month: 'short',
    day: 'numeric',
  });

  return timeBuckets.map(bucket => {
    const date = new Date(bucket.timestamp_ms);
    return {
      date: dateFormatter.format(date),
      count: bucket.count,
      timestamp_ms: bucket.timestamp_ms,
    };
  });
}

const formatTooltipDate = (timestamp_ms: number, locale: string): string => {
  const date = new Date(timestamp_ms);
  return new Intl.DateTimeFormat(locale, {
    year: 'numeric',
    month: 'long',
    day: 'numeric',
  }).format(date);
};

const COLORS = ['#3b82f6', '#10b981', '#f59e0b', '#ef4444'];

// Skeleton Components
const MetricCardSkeleton = () => (
  <Card>
    <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
      <Skeleton className="h-4 w-32" />
      <Skeleton className="h-4 w-4" />
    </CardHeader>
    <CardContent>
      <Skeleton className="h-8 w-24 mb-1" />
      <Skeleton className="h-3 w-20" />
    </CardContent>
  </Card>
);

const ChartSkeleton = () => (
  <div className="h-80 w-full flex items-center justify-center">
    <div className="space-y-2 w-full px-4">
      {[...Array(6)].map((_, i) => (
        <Skeleton key={i} className="h-12 w-full" />
      ))}
    </div>
  </div>
);

// Empty State Components
const EmptyChart = ({ title }: { title: string }) => (
  <div className="h-80 flex flex-col items-center justify-center text-muted-foreground">
    <Inbox className="h-12 w-12 mb-3 opacity-40" />
    <p className="text-sm font-medium">{title}</p>
  </div>
);

const EmptyTable = ({ title }: { title: string }) => (
  <div className="py-10 text-center text-muted-foreground">
    <Inbox className="h-10 w-10 mx-auto mb-3 opacity-40" />
    <p className="text-sm font-medium">{title}</p>
  </div>
);

export default function MailArchiveDashboard() {
  const token = getToken();
  const { data: stats, isLoading } = useQuery({
    queryKey: ['dashboard-stats'],
    enabled: !!token,
    queryFn: get_dashboard_stats,
  });

  const { t, i18n } = useTranslation();

  const currentLocale = i18n.resolvedLanguage || i18n.language || navigator.language;

  const totalAttachments = (stats?.with_attachment_count ?? 0) + (stats?.without_attachment_count ?? 0);
  const attachmentRatio = totalAttachments > 0 ? (stats?.with_attachment_count ?? 0) / totalAttachments : 0;

  const hasRecentActivity = stats?.recent_activity && stats.recent_activity.length > 0;
  const hasTopSenders = stats?.top_senders && stats.top_senders.length > 0;
  const hasTopEmails = stats?.top_largest_emails && stats.top_largest_emails.length > 0;
  const hasTopAccounts = stats?.top_accounts && stats.top_accounts.length > 0;

  const attachmentData = totalAttachments > 0
    ? [
      { name: 'With Attachments', value: attachmentRatio, fill: COLORS[1] },
      { name: 'No Attachments', value: 1 - attachmentRatio, fill: '#e5e7eb' },
    ]
    : [
      { name: 'No Data', value: 1, fill: '#e5e7eb' },
    ];

  if (isLoading) {
    return (
      <div className="flex-1 space-y-6 p-6 md:p-8">
        <div className="flex items-center justify-between">
          <div>
            <Skeleton className="h-9 w-48 mb-2" />
            <Skeleton className="h-4 w-64" />
          </div>
          <Skeleton className="h-7 w-28 rounded-full" />
        </div>

        <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-6">
          {[...Array(6)].map((_, i) => (
            <MetricCardSkeleton key={i} />
          ))}
        </div>

        <div className="space-y-4">
          <Skeleton className="h-10 w-full max-w-md" />
          <Card>
            <CardHeader>
              <Skeleton className="h-6 w-48 mb-2" />
              <Skeleton className="h-4 w-64" />
            </CardHeader>
            <CardContent>
              <ChartSkeleton />
            </CardContent>
          </Card>
        </div>
      </div>
    );
  }

  return (
    <>
      <FixedHeader />
      <Main higher>
        <div className="flex-1 space-y-6 p-6 md:p-8">
          <div className="flex items-center justify-between">
            <div>
              <h2 className="text-3xl font-bold tracking-tight">{t('dashboard.title')}</h2>
            </div>
          </div>


          <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-6">
            <Card>
              <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
                <CardTitle className="text-sm font-medium">{t('dashboard.mailAccounts')}</CardTitle>
                <Users className="h-4 w-4 text-muted-foreground" />
              </CardHeader>
              <CardContent>
                <div className="text-2xl font-bold">{formatNumber(stats!.account_count)}</div>
                <p className="text-xs text-muted-foreground">{t('dashboard.connected')}</p>
              </CardContent>
            </Card>

            <Card>
              <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
                <CardTitle className="text-sm font-medium">{t('dashboard.totalEmails')}</CardTitle>
                <Mail className="h-4 w-4 text-muted-foreground" />
              </CardHeader>
              <CardContent>
                <div className="text-2xl font-bold">{formatNumber(stats!.email_count)}</div>
                <p className="text-xs text-muted-foreground">{t('dashboard.syncedLocally')}</p>
              </CardContent>
            </Card>

            <Card>
              <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
                <CardTitle className="text-sm font-medium">{t('dashboard.totalEmailSize')}</CardTitle>
                <HardDrive className="h-4 w-4 text-muted-foreground" />
              </CardHeader>
              <CardContent>
                <div className="text-2xl font-bold">{formatBytes(stats!.total_size_bytes)}</div>
                <p className="text-xs text-muted-foreground">{t('dashboard.logicalVolume')}</p>
              </CardContent>
            </Card>

            <Card>
              <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
                <CardTitle className="text-sm font-medium">{t('dashboard.localDataFiles')}</CardTitle>
                <Database className="h-4 w-4 text-muted-foreground" />
              </CardHeader>
              <CardContent>
                <div className="text-2xl font-bold">{formatBytes(stats!.storage_usage_bytes)}</div>
                <p className="text-xs text-muted-foreground">{t('dashboard.actualDiskUsage')}</p>
              </CardContent>
            </Card>

            <Card>
              <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
                <CardTitle className="text-sm font-medium">{t('dashboard.indexSize')}</CardTitle>
                <Database className="h-4 w-4 text-muted-foreground" />
              </CardHeader>
              <CardContent>
                <div className="text-2xl font-bold">{formatBytes(stats!.index_usage_bytes)}</div>
                <p className="text-xs text-muted-foreground">{t('dashboard.tantivyIndex')}</p>
              </CardContent>
            </Card>
            <Card>
              <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
                <CardTitle className="text-sm font-medium">{t('dashboard.systemVersion')}</CardTitle>
                <Info className="h-4 w-4 text-muted-foreground" />
              </CardHeader>
              <CardContent>
                <div className='text-2xl font-bold'>
                  {stats!.system_version ? (
                    <a
                      href={`https://github.com/rustmailer/bichon/releases/tag/${stats!.system_version}`}
                      target="_blank"
                      rel="noopener noreferrer"
                      className="hover:underline"
                    >
                      {stats!.system_version}
                    </a>
                  ) : 'N/A'}
                </div>
                <p className="text-xs text-muted-foreground">
                  {stats!.commit_hash ?? 'N/A'}
                </p>
              </CardContent>
            </Card>
          </div>

          <Tabs defaultValue="trend" className="space-y-4">
            <TabsList className="grid w-full grid-cols-3 lg:w-auto">
              <TabsTrigger value="trend">{t('dashboard.dayTrend')}</TabsTrigger>
              <TabsTrigger value="attachment">{t('dashboard.attachments')}</TabsTrigger>
              <TabsTrigger value="top">{t('dashboard.topLists')}</TabsTrigger>
            </TabsList>

            <TabsContent value="trend" className="space-y-4">
              <Card>
                <CardHeader>
                  <CardTitle>{t('dashboard.newEmails')}</CardTitle>
                  <CardDescription>{t('dashboard.messageDistribution')}</CardDescription>
                </CardHeader>
                <CardContent className="h-80">
                  {hasRecentActivity ? (
                    <ResponsiveContainer width="100%" height="100%">
                      <BarChart data={convertRecentActivity(stats!.recent_activity, currentLocale)} margin={{ top: 20, right: 30, left: 20, bottom: 5 }}>
                        <CartesianGrid strokeDasharray="3 3" />
                        <XAxis dataKey="date" tick={{ fontSize: 12 }} interval="preserveStart" tickCount={10} />
                        <YAxis tick={{ fontSize: 12 }} />
                        <Tooltip
                          formatter={(v) => formatNumber(v as number)}
                          content={({ payload }) => {
                            if (payload && payload.length) {
                              const dataPoint = payload[0].payload;
                              const fullDate = formatTooltipDate(dataPoint.timestamp_ms, currentLocale);
                              return (
                                <div className="p-2 border rounded-lg shadow-md bg-white dark:bg-gray-800">
                                  <p className="font-semibold text-sm mb-1">{fullDate}</p>
                                  <p className="text-xs">{t('dashboard.emails')}: {formatNumber(dataPoint.count)}</p>
                                </div>
                              );
                            }
                            return null;
                          }}
                          contentStyle={{
                            backgroundColor: 'hsl(var(--background))',
                            border: '1px solid hsl(var(--border))',
                            borderRadius: '8px'
                          }}
                        />
                        <Bar dataKey="count" fill="#3b82f6" radius={[8, 8, 0, 0]} barSize={36} />
                      </BarChart>
                    </ResponsiveContainer>
                  ) : (
                    <EmptyChart title={t('dashboard.noRecentActivity')} />
                  )}
                </CardContent>
              </Card>
            </TabsContent>

            <TabsContent value="attachment" className="space-y-4">
              <Card>
                <CardHeader>
                  <CardTitle>{t('dashboard.attachmentRatio')}</CardTitle>
                  <CardDescription>
                    {totalAttachments > 0
                      ? t('dashboard.attachmentRatioDesc', { percent: (attachmentRatio * 100).toFixed(1) })
                      : t('dashboard.noEmailsSynced')}
                  </CardDescription>
                </CardHeader>
                <CardContent className="flex items-center justify-center h-80">
                  <ResponsiveContainer width={300} height={300}>
                    <PieChart>
                      <Pie
                        data={attachmentData}
                        cx="50%"
                        cy="50%"
                        innerRadius={60}
                        outerRadius={100}
                        paddingAngle={5}
                        dataKey="value"
                      >
                        {attachmentData.map((entry, i) => (
                          <Cell key={`cell-${i}`} fill={entry.fill} />
                        ))}
                      </Pie>
                      <Tooltip formatter={(v) => totalAttachments > 0 ? `${((v as number) * 100).toFixed(1)}%` : '0%'} />
                    </PieChart>
                  </ResponsiveContainer>
                  <div className="ml-8 space-y-2">
                    {totalAttachments > 0 ? (
                      <>
                        <div className="flex items-center gap-2">
                          <div className="h-3 w-3 rounded-full bg-green-500"></div>
                          <span className="text-sm">
                            {t('dashboard.withAttachments')} ({(attachmentRatio * 100).toFixed(1)}%)
                          </span>
                        </div>
                        <div className="flex items-center gap-2">
                          <div className="h-3 w-3 rounded-full bg-gray-200"></div>
                          <span className="text-sm">{t('dashboard.noAttachments')}</span>
                        </div>
                      </>
                    ) : (
                      <div className="flex items-center gap-2">
                        <div className="h-3 w-3 rounded-full bg-gray-200"></div>
                        <span className="text-sm">{t('common.noData')}</span>
                      </div>
                    )}
                  </div>
                </CardContent>
              </Card>
            </TabsContent>

            <TabsContent value="top" className="space-y-6">
              <div className="grid gap-6 md:grid-cols-2 lg:grid-cols-3">
                <Card className="md:col-span-1">
                  <CardHeader>
                    <CardTitle>{t('dashboard.top10Senders')}</CardTitle>
                    <CardDescription>{t('dashboard.byMessageCount')}</CardDescription>
                  </CardHeader>
                  <CardContent>
                    {hasTopSenders ? (
                      <Table>
                        <TableHeader>
                          <TableRow>
                            <TableHead>{t('dashboard.sender')}</TableHead>
                            <TableHead className="text-right">{t('dashboard.count')}</TableHead>
                          </TableRow>
                        </TableHeader>
                        <TableBody>
                          {stats!.top_senders.map((s) => (
                            <TableRow key={s.key}>
                              <TableCell className="font-medium max-w-[180px] truncate" title={s.key}>
                                {s.key}
                              </TableCell>
                              <TableCell className="text-right">{formatNumber(s.count)}</TableCell>
                            </TableRow>
                          ))}
                        </TableBody>
                      </Table>
                    ) : (
                      <EmptyTable title={t('dashboard.noSendersData')} />
                    )}
                  </CardContent>
                </Card>
                <Card className="md:col-span-1">
                  <CardHeader>
                    <CardTitle>{t('dashboard.top10LargestEmails')}</CardTitle>
                    <CardDescription>{t('dashboard.byMessageSize')}</CardDescription>
                  </CardHeader>
                  <CardContent>
                    {hasTopEmails ? (
                      <Table>
                        <TableHeader>
                          <TableRow>
                            <TableHead>{t('dashboard.subject')}</TableHead>
                            <TableHead className="text-right">{t('dashboard.size')}</TableHead>
                          </TableRow>
                        </TableHeader>
                        <TableBody>
                          {stats!.top_largest_emails.map((m, index) => (
                            <TableRow key={index}>
                              <TableCell
                                className="max-w-[180px] truncate font-medium"
                                title={m.subject}
                              >
                                {m.subject || t('dashboard.noSubject')}
                              </TableCell>
                              <TableCell className="text-right">{formatBytes(m.size_bytes)}</TableCell>
                            </TableRow>
                          ))}
                        </TableBody>
                      </Table>
                    ) : (
                      <EmptyTable title={t('dashboard.noLargeEmails')} />
                    )}
                  </CardContent>
                </Card>
                <Card className="md:col-span-1">
                  <CardHeader>
                    <CardTitle>{t('dashboard.top10Accounts')}</CardTitle>
                    <CardDescription>{t('dashboard.byMessageCount')}</CardDescription>
                  </CardHeader>
                  <CardContent>
                    {hasTopAccounts ? (
                      <Table>
                        <TableHeader>
                          <TableRow>
                            <TableHead>{t('dashboard.account')}</TableHead>
                            <TableHead className="text-right">{t('dashboard.emails')}</TableHead>
                          </TableRow>
                        </TableHeader>
                        <TableBody>
                          {stats!.top_accounts.map((acc) => (
                            <TableRow key={acc.key}>
                              <TableCell className="font-medium max-w-[160px] truncate" title={acc.key}>
                                {acc.key}
                              </TableCell>
                              <TableCell className="text-right">{formatNumber(acc.count)}</TableCell>
                            </TableRow>
                          ))}
                        </TableBody>
                      </Table>
                    ) : (
                      <EmptyTable title={t('dashboard.noAccountData')} />
                    )}
                  </CardContent>
                </Card>
              </div>
            </TabsContent>
          </Tabs>
        </div>

        <div className="p-6 md:p-8 pt-0 text-center text-xs text-muted-foreground">
          © 2025 <a href="https://rustmailer.com" target="_blank" rel="noopener noreferrer" className="hover:underline">rustmailer.com</a> - Bichon Email Archiving Project
        </div>
      </Main>
    </>
  );
}