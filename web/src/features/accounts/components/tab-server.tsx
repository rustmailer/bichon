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

import { useFormContext } from "react-hook-form";
import { useTranslation } from "react-i18next";
import {
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
  FormControl,
  FormDescription,
} from "@/components/ui/form";
import { Input } from "@/components/ui/input";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { PasswordInput } from "@/components/password-input";
import { AccountFormValues } from "./schema";
import useProxyList from "@/hooks/use-proxy";

interface TabServerProps {
  isEdit?: boolean;
}

export function TabServer({ isEdit }: TabServerProps) {
  const { t } = useTranslation();
  const { control, watch } = useFormContext<AccountFormValues>();
  const authType = watch('imap.auth.auth_type');
  const { proxyOptions } = useProxyList();

  return (
    <div className="space-y-6">
      <FormField
        control={control}
        name="imap.host"
        render={({ field }) => (
          <FormItem>
            <FormLabel>{t('accounts.imapHost')}<span className="text-red-500 align-super text-xs">*</span></FormLabel>
            <FormControl>
              <Input {...field} placeholder={t('accounts.imapHostPlaceholder')} />
            </FormControl>
            <FormMessage />
          </FormItem>
        )}
      />

      <div className="grid grid-cols-2 gap-4">
        <FormField
          control={control}
          name="imap.port"
          render={({ field }) => (
            <FormItem>
              <FormLabel>{t('accounts.imapPort')}<span className="text-red-500 align-super text-xs">*</span></FormLabel>
              <FormControl>
                <Input
                  type="number"
                  {...field}
                  onChange={(e) => field.onChange(parseInt(e.target.value, 10))}
                  placeholder={t('accounts.imapPortPlaceholder')}
                />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />

        <FormField
          control={control}
          name="imap.encryption"
          render={({ field }) => (
            <FormItem>
              <FormLabel>{t('accounts.imapEncryption')}<span className="text-red-500 align-super text-xs">*</span></FormLabel>
              <Select onValueChange={field.onChange} value={field.value}>
                <FormControl>
                  <SelectTrigger>
                    <SelectValue />
                  </SelectTrigger>
                </FormControl>
                <SelectContent>
                  <SelectItem value="Ssl">SSL/TLS</SelectItem>
                  <SelectItem value="StartTls">StartTLS</SelectItem>
                  <SelectItem value="None">{t('accounts.none')}</SelectItem>
                </SelectContent>
              </Select>
              <FormMessage />
            </FormItem>
          )}
        />
      </div>

      <FormField
        control={control}
        name="login_name"
        render={({ field }) => (
          <FormItem>
            <FormLabel>{t('accounts.login_name')}</FormLabel>
            <FormControl>
              <Input {...field} value={field.value ?? ''} placeholder={t('accounts.namePlaceholder')} disabled={isEdit} />
            </FormControl>
            <FormDescription>{t('accounts.nameDescription')}</FormDescription>
            <FormMessage />
          </FormItem>
        )}
      />

      <FormField
        control={control}
        name="imap.auth.auth_type"
        render={({ field }) => (
          <FormItem>
            <FormLabel>{t('accounts.imapAuthMethod')}<span className="text-red-500 align-super text-xs">*</span></FormLabel>
            <Select onValueChange={field.onChange} value={field.value}>
              <FormControl>
                <SelectTrigger>
                  <SelectValue />
                </SelectTrigger>
              </FormControl>
              <SelectContent>
                <SelectItem value="Password">{t('accounts.authPassword')}</SelectItem>
                <SelectItem value="OAuth2">OAuth2</SelectItem>
              </SelectContent>
            </Select>
            <FormMessage />
          </FormItem>
        )}
      />

      {authType === 'Password' && (
        <FormField
          control={control}
          name="imap.auth.password"
          render={({ field }) => (
            <FormItem>
              <FormLabel>{t('accounts.imapPassword')}{!isEdit && <span className="text-red-500 align-super text-xs">*</span>}</FormLabel>
              <FormControl>
                <PasswordInput placeholder={isEdit ? t('accounts.leaveEmptyToKeepPassword') : t('accounts.enterPassword')} {...field} />
              </FormControl>
              {isEdit && <FormDescription>{t('accounts.leaveEmptyToKeepPassword')}</FormDescription>}
              <FormMessage />
            </FormItem>
          )}
        />
      )}

      <FormField
        control={control}
        name="imap.use_proxy"
        render={({ field }) => (
          <FormItem>
            <FormLabel>{t('accounts.useProxyOptional')}</FormLabel>
            <Select
              onValueChange={(v) => field.onChange(v === 'none' ? undefined : Number(v))}
              defaultValue={field.value?.toString()}
            >
              <FormControl>
                <SelectTrigger>
                  <SelectValue placeholder={t('accounts.selectProxy')}/>
                </SelectTrigger>
              </FormControl>
              <SelectContent>
                <SelectItem key="none" value="none">{t('accounts.useNoProxy')}</SelectItem>
                {proxyOptions.map((opt) => (
                  <SelectItem key={opt.value} value={opt.value}>
                    <span className="max-w-[280px] truncate block" title={opt.label}>
                      {opt.label}
                    </span>
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <FormDescription>{t('accounts.imapProxy')}</FormDescription>
            <FormMessage />
          </FormItem>
        )}
      />

      <FormField
        control={control}
        name="use_dangerous"
        render={({ field }) => (
          <FormItem className="flex flex-row items-start space-x-3 space-y-0 rounded-md border p-4">
            <FormControl>
              <Checkbox checked={field.value} onCheckedChange={field.onChange} />
            </FormControl>
            <div className="space-y-1 leading-none">
              <FormLabel>{t('accounts.useDangerous')}</FormLabel>
            </div>
          </FormItem>
        )}
      />

      <FormField
        control={control}
        name="uidonly_enabled"
        render={({ field }) => (
          <FormItem className="flex flex-row items-start space-x-3 space-y-0 rounded-md border p-4">
            <FormControl>
              <Checkbox checked={field.value} onCheckedChange={field.onChange} />
            </FormControl>
            <div className="space-y-1 leading-none">
              <FormLabel>{t('accounts.uidonlyEnabled')}</FormLabel>
              <FormDescription>{t('accounts.uidonlyEnabledDescription')}</FormDescription>
            </div>
          </FormItem>
        )}
      />
    </div>
  );
}
