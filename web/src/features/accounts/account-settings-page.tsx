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

import { useCallback, useEffect } from "react";
import { useForm, FormProvider } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { useTranslation } from "react-i18next";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { ArrowLeft } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Form } from "@/components/ui/form";
import { useToast } from "@/hooks/use-toast";
import { ToastAction } from "@/components/ui/toast";
import { Breadcrumb } from "@/components/ui/breadcrumb";
import { FixedHeader } from "@/components/layout/fixed-header";
import { Main } from "@/components/layout/main";
import { TabGeneral } from "./components/tab-general";
import { TabServer } from "./components/tab-server";
import { TabDownload } from "./components/tab-download";
import { TabFilters } from "./components/tab-filters";
import { update_account, list_accounts, type AccountModel } from "@/api/account/api";
import { getAccountSchema, type AccountFormValues } from "./components/schema";
import type { AxiosError } from "axios";
import { useQuery } from "@tanstack/react-query";

const emptyImap = {
  host: "",
  port: 0,
  encryption: "None" as const,
  auth: { auth_type: "Password" as const, password: undefined },
};

function mapAccountToFormValues(account: AccountModel): AccountFormValues {
  const imap = { ...(account.imap ?? emptyImap), use_proxy: account.imap?.use_proxy ?? undefined };
  imap.auth = { ...imap.auth, password: undefined };

  return {
    account_name: account.account_name ?? undefined,
    login_name: account.login_name ?? undefined,
    email: account.email,
    imap,
    enabled: account.enabled,
    use_dangerous: account.use_dangerous,
    uidonly_enabled: account.uidonly_enabled,
    date_since: account.date_since ?? undefined,
    date_before: account.date_before ?? undefined,
    download_interval_min: account.download_interval_min ?? 60,
    download_batch_size: account.download_batch_size ?? 30,
    max_email_size_bytes: account.max_email_size_bytes ?? 100 * 1024 * 1024,
    auto_download_new_mailboxes: account.auto_download_new_mailboxes ?? true,
    download_schedule: account.download_schedule ?? undefined,
    archive_rules: account.archive_rules ?? undefined,
  };
}

function SectionHeader({ title, description }: { title: string; description?: string }) {
  return (
    <div className="pb-3">
      <h3 className="text-base font-semibold">{title}</h3>
      {description && <p className="text-sm text-muted-foreground mt-0.5">{description}</p>}
    </div>
  );
}

interface AccountSettingsPageProps {
  accountId: number;
}

export function AccountSettingsPage({ accountId }: AccountSettingsPageProps) {
  const { t } = useTranslation();
  //const navigate = useNavigate();
  const { toast } = useToast();
  const queryClient = useQueryClient();

  const { data: accountList } = useQuery({
    queryKey: ['account-list'],
    queryFn: list_accounts,
  });

  const account = accountList?.items?.find((a) => a.id === accountId);

  const accountSchema = getAccountSchema(true, t);
  const form = useForm<AccountFormValues>({
    mode: "onChange",
    defaultValues: account ? mapAccountToFormValues(account) : undefined,
    resolver: zodResolver(accountSchema),
  });

  useEffect(() => {
    if (account) {
      form.reset(mapAccountToFormValues(account));
    }
  }, [account?.id]);

  const updateMutation = useMutation({
    mutationFn: (data: Record<string, any>) => update_account(accountId, data),
    onSuccess: () => {
      toast({
        title: t('accounts.settings.saved'),
        description: t('accounts.settings.savedDesc'),
        action: <ToastAction altText={t('common.close')}>{t('common.close')}</ToastAction>,
      });
      queryClient.invalidateQueries({ queryKey: ['account-list'] });
    },
    onError: (error: AxiosError) => {
      const errorMessage =
        (error.response?.data as { message?: string })?.message ||
        error.message ||
        t('accounts.updateFailed');
      toast({
        variant: "destructive",
        title: t('accounts.accountUpdateFailed'),
        description: errorMessage as string,
        action: <ToastAction altText={t('common.tryAgain')}>{t('common.tryAgain')}</ToastAction>,
      });
    },
  });

  const onSubmit = useCallback(
    (data: AccountFormValues) => {
      const { use_proxy, ...imapRest } = data.imap;
      const payload: Record<string, any> = {
        email: data.email,
        account_name: data.account_name,
        login_name: data.login_name,
        imap: {
          ...imapRest,
          use_proxy,
          auth: {
            ...data.imap.auth,
            password: data.imap.auth.auth_type === 'OAuth2'
              ? undefined
              : (data.imap.auth.password ? data.imap.auth.password : undefined),
          },
        },
        enabled: data.enabled,
        use_dangerous: data.use_dangerous,
        uidonly_enabled: data.uidonly_enabled,
        date_since: data.date_since,
        date_before: data.date_before,
        download_interval_min: data.download_interval_min,
        download_batch_size: data.download_batch_size,
        max_email_size_bytes: data.max_email_size_bytes,
        auto_download_new_mailboxes: data.auto_download_new_mailboxes,
        download_schedule: data.download_schedule || null,
        archive_rules: data.archive_rules || null,
      };

      if (!data.date_since && !data.date_before) {
        payload.clear_date_range = true;
      }
      if (!data.download_schedule && account?.download_schedule) {
        payload.clear_download_schedule = true;
      }

      updateMutation.mutate(payload);
    },
    [updateMutation, account]
  );

  if (!account) {
    return (
      <>
        <FixedHeader />
        <Main>
          <div className="mx-auto w-full max-w-[46rem] px-4 py-12 text-center text-muted-foreground">
            {t('accounts.settings.loading')}
          </div>
        </Main>
      </>
    );
  }

  return (
    <>
      <FixedHeader />
      <Main>
        <div className="mx-auto w-full max-w-[46rem] px-4 py-6">
          <div className="mb-6 space-y-3">
            <Link
              to="/accounts"
              className="inline-flex items-center gap-1.5 text-sm text-muted-foreground hover:text-foreground transition-colors"
            >
              <ArrowLeft className="h-4 w-4" />
              {t('accounts.settings.backToAccounts')}
            </Link>
            <Breadcrumb items={[
              { label: t('accounts.title'), to: '/accounts' },
              { label: account.email },
              { label: t('accounts.settings.settings') },
            ]} />
          </div>

          <div className="rounded-lg border shadow-sm bg-card p-6 md:p-8">
            <div className="mb-6">
              <h2 className="text-xl font-bold">{account.email}</h2>
              <p className="text-sm text-muted-foreground mt-1">{t('accounts.updateTheEmailAccountHere')}</p>
            </div>

            <FormProvider {...form}>
              <Form {...form}>
                <form onSubmit={form.handleSubmit(onSubmit)} className="space-y-10">
                  <section>
                    <SectionHeader
                      title={t('accounts.settings.general')}
                      description={t('accounts.settings.generalDesc')}
                    />
                    <TabGeneral isEdit />
                  </section>

                  <hr />

                  <section>
                    <SectionHeader
                      title={t('accounts.settings.server')}
                      description={t('accounts.settings.serverDesc')}
                    />
                    <TabServer isEdit />
                  </section>

                  <hr />

                  <section>
                    <SectionHeader
                      title={t('accounts.settings.download')}
                      description={t('accounts.settings.downloadDesc')}
                    />
                    <TabDownload />
                  </section>

                  <hr />

                  <section>
                    <SectionHeader
                      title={t('accounts.settings.filters')}
                      description={t('accounts.settings.filtersDesc')}
                    />
                    <TabFilters />
                  </section>

                  <div className="flex items-center justify-between pt-4 border-t">
                    <Button
                      type="button"
                      variant="outline"
                      onClick={() => {
                        if (account) form.reset(mapAccountToFormValues(account));
                      }}
                    >
                      {t('accounts.settings.reset')}
                    </Button>
                    <Button type="submit" size="lg" disabled={updateMutation.isPending}>
                      {updateMutation.isPending ? t('accounts.settings.saving') : t('accounts.saveChanges')}
                    </Button>
                  </div>
                </form>
              </Form>
            </FormProvider>
          </div>
        </div>
      </Main>
    </>
  );
}
