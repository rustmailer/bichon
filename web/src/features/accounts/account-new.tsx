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

import { useCallback, useState } from "react";
import { useForm, FormProvider } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { useTranslation } from "react-i18next";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useNavigate, Link } from "@tanstack/react-router";
import { ArrowLeft, Loader2 } from "lucide-react";
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
import { create_account, autoconfig } from "@/api/account/api";
import { getAccountSchema, type AccountFormValues } from "./components/schema";
import type { AxiosError } from "axios";

const defaultValues: AccountFormValues = {
  login_name: undefined,
  account_name: undefined,
  email: '',
  imap: {
    host: "",
    port: 993,
    encryption: 'Ssl',
    auth: { auth_type: 'Password', password: undefined },
    use_proxy: undefined,
  },
  enabled: true,
  use_dangerous: false,
  uidonly_enabled: false,
  date_since: undefined,
  date_before: undefined,
  download_interval_min: 60,
  download_batch_size: 30,
  max_email_size_bytes: 100 * 1024 * 1024,
  auto_download_new_mailboxes: true,
  download_schedule: undefined,
  archive_rules: undefined,
};

function SectionHeader({ title, description }: { title: string; description?: string }) {
  return (
    <div className="pb-3">
      <h3 className="text-base font-semibold">{title}</h3>
      {description && <p className="text-sm text-muted-foreground mt-0.5">{description}</p>}
    </div>
  );
}

export function AccountNewPage() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const { toast } = useToast();
  const queryClient = useQueryClient();
  const [autoConfigLoading, setAutoConfigLoading] = useState(false);

  const accountSchema = getAccountSchema(false, t);
  const form = useForm<AccountFormValues>({
    mode: "onChange",
    defaultValues,
    resolver: zodResolver(accountSchema),
  });

  const createMutation = useMutation({
    mutationFn: create_account,
    onSuccess: () => {
      toast({
        title: t('accounts.accountCreated'),
        description: t('accounts.accountCreatedDesc'),
        action: <ToastAction altText={t('common.close')}>{t('common.close')}</ToastAction>,
      });
      queryClient.invalidateQueries({ queryKey: ['account-list'] });
      navigate({ to: '/accounts' });
    },
    onError: (error: AxiosError) => {
      const errorMessage =
        (error.response?.data as { message?: string })?.message ||
        error.message ||
        t('accounts.creationFailed');
      toast({
        variant: "destructive",
        title: t('accounts.accountCreationFailed'),
        description: errorMessage as string,
        action: <ToastAction altText={t('common.tryAgain')}>{t('common.tryAgain')}</ToastAction>,
      });
    },
  });

  const onSubmit = useCallback(
    (data: AccountFormValues) => {
      const { use_proxy, ...imapRest } = data.imap;
      createMutation.mutate({
        email: data.email,
        account_name: data.account_name,
        login_name: data.login_name,
        imap: {
          ...imapRest,
          use_proxy,
          auth: {
            ...data.imap.auth,
            password: data.imap.auth.auth_type === 'OAuth2' ? undefined : data.imap.auth.password,
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
        account_type: "IMAP",
        archive_rules: data.archive_rules || null,
      });
    },
    [createMutation]
  );

  const handleAutoConfig = async () => {
    const email = form.getValues('email');
    if (!email) return;
    const imap = form.getValues('imap');
    if (imap.host.trim() !== "" && imap.port > 0) return;

    setAutoConfigLoading(true);
    try {
      const result = await autoconfig(email);
      if (result) {
        form.setValue('imap.host', result.imap.host);
        form.setValue('imap.port', result.imap.port);
        form.setValue('imap.encryption', result.imap.encryption);
        if (result.oauth2) form.setValue('imap.auth.auth_type', 'OAuth2');
      }
    } catch (error) {
      console.error('Auto-configuration failed:', error);
    }
    setAutoConfigLoading(false);
  };

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
              { label: t('accounts.settings.newAccount') },
            ]} />
          </div>

          <div className="rounded-lg border shadow-sm bg-card p-6 md:p-8">
            <div className="mb-6">
              <h2 className="text-xl font-bold">{t('accounts.addAccount')}</h2>
              <p className="text-sm text-muted-foreground mt-1">{t('accounts.addNewEmailAccountHere')}</p>
            </div>

            <FormProvider {...form}>
              <Form {...form}>
                <form onSubmit={form.handleSubmit(onSubmit)} className="space-y-10">
                  <section>
                    <SectionHeader
                      title={t('accounts.settings.general')}
                      description={t('accounts.settings.generalDesc')}
                    />
                    <TabGeneral />
                  </section>

                  <hr />

                  <section>
                    <SectionHeader
                      title={t('accounts.settings.server')}
                      description={t('accounts.settings.serverDesc')}
                    />
                    <div className="flex items-center gap-2 mb-4">
                      <Button
                        type="button"
                        variant="outline"
                        size="sm"
                        disabled={autoConfigLoading}
                        onClick={handleAutoConfig}
                      >
                        {autoConfigLoading && <Loader2 className="mr-1.5 h-3.5 w-3.5 animate-spin" />}
                        {autoConfigLoading ? t('accounts.autoConfiguring') : t('accounts.autoDiscover')}
                      </Button>
                    </div>
                    <TabServer />
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
                      onClick={() => navigate({ to: '/accounts' })}
                    >
                      {t('common.cancel')}
                    </Button>
                    <Button type="submit" size="lg" disabled={createMutation.isPending}>
                      {createMutation.isPending ? t('accounts.creating') : t('accounts.submit')}
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
