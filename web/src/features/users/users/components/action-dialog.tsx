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

import { useState, useMemo } from 'react'
import { useFieldArray, useForm } from 'react-hook-form'
import { zodResolver } from '@hookform/resolvers/zod'
import { useMutation, useQueryClient } from '@tanstack/react-query'
import { Loader2, Shield, Settings2, UserIcon, Plus, Trash2, Mail, Check, ChevronsUpDown } from 'lucide-react'
import { AxiosError } from 'axios'

import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import {
  Form,
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
} from '@/components/ui/form'
import { Input } from '@/components/ui/input'
import { Textarea } from '@/components/ui/textarea'
import { ScrollArea } from '@/components/ui/scroll-area'
import { cn } from '@/lib/utils'
import { Checkbox } from '@/components/ui/checkbox'
import { Switch } from '@/components/ui/switch'
import { Separator } from '@/components/ui/separator'
import { toast } from '@/hooks/use-toast'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select"
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover"
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from "@/components/ui/command"

import { create_user, update_user, User } from '@/api/users/api'
import useMinimalAccountList from '@/hooks/use-minimal-account-list'
import { useRoles } from '@/hooks/use-roles'
import { PasswordInput } from '@/components/password-input'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { useTranslation } from 'react-i18next'
import { getCreateUserSchema, getUpdateUserSchema, type UserFormValues } from './schema'

export type UserForm = UserFormValues

interface Props {
  currentRow?: User
  open: boolean
  onOpenChange: (open: boolean) => void
}

function AccountSearchSelect({
  accounts,
  value,
  onChange,
  placeholder,
}: {
  accounts: { id: number; email: string; name?: string }[]
  value: number
  onChange: (v: number) => void
  placeholder: string
}) {
  const [open, setOpen] = useState(false)
  const [search, setSearch] = useState('')

  const selectedAccount = accounts.find(a => a.id === value)

  const filtered = useMemo(() => {
    const q = search.toLowerCase()
    return accounts
      .filter(a =>
        !q || a.email.toLowerCase().includes(q) || a.name?.toLowerCase().includes(q) || String(a.id).includes(q)
      )
      .sort((a, b) => a.email.toLowerCase().localeCompare(b.email.toLowerCase()))
  }, [accounts, search])

  return (
    <Popover open={open} onOpenChange={(o) => { setOpen(o); if (!o) setSearch('') }}>
      <PopoverTrigger asChild>
        <FormControl>
          <Button variant="outline" role="combobox" className="bg-muted/10 justify-between w-full font-normal">
            {selectedAccount ? selectedAccount.email : placeholder}
            <ChevronsUpDown className="ml-2 h-4 w-4 shrink-0 opacity-50" />
          </Button>
        </FormControl>
      </PopoverTrigger>
      <PopoverContent className="p-0 w-[--radix-popover-trigger-width]" align="start">
        <Command shouldFilter={false}>
          <CommandInput value={search} onValueChange={setSearch} placeholder={placeholder} />
          <CommandList>
            <CommandEmpty>No account found</CommandEmpty>
            <CommandGroup>
              {filtered.map(acc => (
                <CommandItem
                  key={acc.id}
                  value={String(acc.id)}
                  onSelect={() => { onChange(acc.id); setOpen(false) }}
                >
                  <Check className={cn("mr-2 h-4 w-4", acc.id === value ? "opacity-100" : "opacity-0")} />
                  <span className="truncate">{acc.email}</span>
                  {acc.name && <span className="ml-2 text-xs text-muted-foreground">{acc.name}</span>}
                </CommandItem>
              ))}
            </CommandGroup>
          </CommandList>
        </Command>
      </PopoverContent>
    </Popover>
  )
}

export function UserActionDialog({ currentRow, open, onOpenChange }: Props) {
  const { t } = useTranslation()
  const isEdit = !!currentRow
  const [showAdvanced, setShowAdvanced] = useState(isEdit && !!currentRow?.acl)
  const queryClient = useQueryClient()

  const { global, account } = useRoles()
  const { minimalList: allAccounts } = useMinimalAccountList()

  const form = useForm<UserForm>({
    resolver: zodResolver(isEdit ? getUpdateUserSchema(t) : getCreateUserSchema(t)),
    defaultValues: useMemo(() => {
      if (isEdit && currentRow) {
        const accessEntries = currentRow.account_access_map
          ? Object.entries(currentRow.account_access_map).map(([accId, roleId]) => ({
            accountId: Number(accId),
            roleId: Number(roleId)
          }))
          : [];

        return {
          username: currentRow.username,
          email: currentRow.email,
          password: '',
          global_roles: currentRow.global_roles ?? [],
          account_access_entries: accessEntries,
          description: currentRow.description ?? '',
          acl: currentRow.acl ? {
            ip_whitelist: currentRow.acl.ip_whitelist?.join('\n'),
            rate_limit: currentRow.acl.rate_limit,
          } : undefined,
        }
      }
      return {
        username: '', email: '', password: '', global_roles: [], account_access_entries: [], description: '', acl: undefined
      }
    }, [isEdit, currentRow])
  })

  const { fields, append, remove } = useFieldArray({
    control: form.control,
    name: "account_access_entries"
  });

  const selectedRoleIds = form.watch("global_roles") || [];

  const isSystemAdmin = useMemo(() => {
    if (!selectedRoleIds.length) return false
    return global.isAdmin(selectedRoleIds)
  }, [global, selectedRoleIds])

  const createMutation = useMutation({ mutationFn: create_user, onSuccess: handleSuccess, onError: handleError })
  const updateMutation = useMutation({ mutationFn: (data: any) => update_user(currentRow!.id, data), onSuccess: handleSuccess, onError: handleError })

  function handleSuccess() {
    toast({ title: isEdit ? t('users.actions.toast.updated') : t('users.actions.toast.created') })
    queryClient.invalidateQueries({ queryKey: ['user-list'] })
    onOpenChange(false)
    form.reset()
  }

  function handleError(err: AxiosError) {
    toast({ variant: 'destructive', title: t('common.error'), description: (err.response?.data as any)?.message || t('common.op_failed') })
  }

  const onSubmit = (values: UserForm) => {
    const validEntries = values.account_access_entries.filter((e: any) => e.accountId > 0 && e.roleId > 0);
    const account_access_map = Object.fromEntries(
      validEntries.map((e: any) => [e.accountId, e.roleId])
    );
    const { account_access_entries, ...rest } = values;
    const payload = {
      ...rest,
      account_access_map,
      acl: values.acl ? {
        ...values.acl,
        ip_whitelist: values.acl.ip_whitelist?.split('\n').map((v: string) => v.trim()).filter(Boolean)
      } : undefined
    }

    isEdit ? updateMutation.mutate(payload) : createMutation.mutate(payload)
  }

  const isSaving = createMutation.isPending || updateMutation.isPending;

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent aria-describedby={undefined} className="max-w-5xl h-[80vh] flex flex-col p-0 overflow-hidden">
        <DialogHeader className="p-6 pb-0 shrink-0">
          <div className="flex items-center gap-4 mb-4">
            {isEdit && currentRow?.avatar ? (
              <img src={`data:image/png;base64,${currentRow.avatar}`} className="h-12 w-12 rounded-full border shadow-sm object-cover" alt="" />
            ) : (
              <div className="flex h-12 w-12 items-center justify-center rounded-full bg-primary/10 text-primary">
                <UserIcon className="h-6 w-6" />
              </div>
            )}
            <div>
              <DialogTitle className="text-xl">
                {isEdit ? t('users.actions.dialog.edit_title', { name: currentRow?.username }) : t('users.actions.dialog.create_title')}
              </DialogTitle>
              <p className="text-xs text-muted-foreground mt-0.5">{t('users.actions.dialog.subtitle')}</p>
            </div>
          </div>
        </DialogHeader>
        <Form {...form}>
          <form id="user-form" onSubmit={form.handleSubmit(onSubmit)} className="flex-1 flex flex-col overflow-hidden">
            <Tabs defaultValue="general" className="flex-1 flex flex-col overflow-hidden">
              <div className="px-6 border-b bg-muted/20">
                <TabsList className="h-12 bg-transparent gap-6 p-0">
                  <TabsTrigger value="general" className="h-12 rounded-none border-b-2 border-transparent data-[state=active]:border-primary data-[state=active]:bg-transparent shadow-none">
                    {t('users.actions.tabs.general')}
                  </TabsTrigger>
                  <TabsTrigger value="permissions" className="h-12 rounded-none border-b-2 border-transparent data-[state=active]:border-primary data-[state=active]:bg-transparent shadow-none">
                    {t('users.actions.tabs.permissions')}
                  </TabsTrigger>
                  <TabsTrigger value="security" className="h-12 rounded-none border-b-2 border-transparent data-[state=active]:border-primary data-[state=active]:bg-transparent shadow-none">
                    {t('users.actions.tabs.security')}
                  </TabsTrigger>
                </TabsList>
              </div>
              <TabsContent value="general" className="flex-1 overflow-hidden m-0 p-6 focus-visible:ring-0">
                <ScrollArea className="h-full pr-4">
                  <div className="max-w-2xl space-y-8 mb-5">
                    <section className="space-y-4">
                      <h4 className="text-sm font-medium flex items-center gap-2"><UserIcon className="h-4 w-4" /> {t('users.actions.sections.basic')}</h4>
                      <div className="grid grid-cols-2 gap-4">
                        <FormField control={form.control} name="username" render={({ field }) => (
                          <FormItem>
                            <FormLabel>{t('users.actions.fields.username')}</FormLabel>
                            <FormControl>
                              <Input {...field} /></FormControl>
                            <FormMessage />
                            <FormDescription> {t('users.actions.fields.username_hint')} </FormDescription>
                          </FormItem>
                        )} />
                        <FormField control={form.control} name="email" render={({ field }) => (
                          <FormItem><FormLabel>{t('users.actions.fields.email')}</FormLabel><FormControl><Input {...field} /></FormControl>
                            <FormMessage />
                            <FormDescription> {t('users.actions.fields.email_hint')} </FormDescription>
                          </FormItem>
                        )} />
                      </div>
                      <FormField control={form.control} name="password" render={({ field }) => (
                        <FormItem>
                          <FormLabel>{t('users.actions.fields.password')}</FormLabel>
                          <FormControl><PasswordInput {...field} placeholder={isEdit ? t('users.actions.fields.password_placeholder_edit') : t('users.actions.fields.password_placeholder_create')} /></FormControl>
                          <FormMessage />
                          <FormDescription>
                            {isEdit
                              ? t('users.actions.fields.password_hint_edit')
                              : t('users.actions.fields.password_hint_create')}
                          </FormDescription>
                        </FormItem>
                      )} />
                    </section>

                    <Separator />

                    <section className="space-y-4">
                      <h4 className="text-sm font-medium flex items-center gap-2"><Shield className="h-4 w-4" /> {t('users.actions.sections.global')}</h4>
                      <FormField control={form.control} name="global_roles" render={({ field }) => (
                        <FormItem>
                          <div className="grid grid-cols-2 gap-3 border rounded-lg p-4 bg-muted/5">
                            {global.roles?.map(role => (
                              <div key={role.id} className="flex items-center gap-2">
                                <Checkbox
                                  id={`role-${role.id}`}
                                  checked={field.value.includes(role.id)}
                                  onCheckedChange={(c) => field.onChange(c ? [...field.value, role.id] : field.value.filter(id => id !== role.id))}
                                />
                                <label htmlFor={`role-${role.id}`} className="text-sm font-normal cursor-pointer">{role.name}</label>
                              </div>
                            ))}
                          </div>
                          <FormMessage />
                          <FormDescription>
                            {t('users.actions.fields.global_roles_hint')}
                          </FormDescription>
                        </FormItem>
                      )} />
                    </section>
                    <Separator />
                    <FormField
                      control={form.control}
                      name="description"
                      render={({ field }) => (
                        <FormItem>
                          <FormLabel>{t('users.actions.fields.description')}</FormLabel>
                          <FormControl>
                            <Textarea
                              {...field}
                              placeholder={t('users.actions.fields.description_placeholder')}
                              className="min-h-[100px] resize-none"
                            />
                          </FormControl>
                          <FormDescription>
                            {t('users.actions.fields.description_hint')}
                          </FormDescription>
                          <FormMessage />
                        </FormItem>
                      )}
                    />
                  </div>
                </ScrollArea>
              </TabsContent>

              <TabsContent value="permissions" className="flex-1 overflow-hidden m-0 p-6 focus-visible:ring-0">
                <div className="flex flex-col h-full">
                  <div className="flex items-center justify-between mb-4 shrink-0">
                    <div>
                      <h4 className="text-sm font-medium">{t('users.actions.sections.mail_access')}</h4>
                      <p className="text-xs text-muted-foreground">{t('users.actions.sections.mail_access_subtitle')}</p>
                    </div>
                    <Button type="button" variant="outline" size="sm" disabled={isSystemAdmin} onClick={() => append({ accountId: 0, roleId: 0 })}>
                      <Plus className="h-3 w-3 mr-1" /> {t('users.actions.buttons.add_account')}
                    </Button>
                  </div>

                  {isSystemAdmin ? (
                    <div className="flex-1 border-2 border-dashed rounded-xl flex flex-col items-center justify-center bg-primary/[0.02] p-6">
                      <Shield className="h-12 w-12 text-primary/40 mb-3" />
                      <p className="font-semibold text-primary/80">{t('users.actions.states.full_access')}</p>
                      <p className="text-sm text-muted-foreground text-center max-w-sm mt-1">
                        {t('users.actions.states.full_access_desc')}
                      </p>
                    </div>
                  ) : (
                    <ScrollArea className="flex-1 -mx-2 px-2">
                      <div className="space-y-3 pb-4">
                        {fields.map((item, index) => (
                          <div key={item.id} className="flex items-start gap-3 p-3 border rounded-xl bg-card shadow-sm hover:border-primary/30 transition-colors">
                            <FormField control={form.control} name={`account_access_entries.${index}.accountId`} render={({ field }) => (
                              <FormItem className="flex-1">
                                <AccountSearchSelect
                                  accounts={allAccounts ?? []}
                                  value={field.value}
                                  onChange={(v) => field.onChange(v)}
                                  placeholder={t('users.actions.fields.select_account')}
                                />
                                <FormMessage />
                              </FormItem>
                            )} />
                            <FormField control={form.control} name={`account_access_entries.${index}.roleId`} render={({ field }) => (
                              <FormItem className="w-48">
                                <Select onValueChange={(v) => field.onChange(Number(v))} value={field.value ? String(field.value) : undefined}>
                                  <FormControl><SelectTrigger className="bg-muted/10"><SelectValue placeholder={t('users.actions.fields.assign_role')} /></SelectTrigger></FormControl>
                                  <SelectContent>
                                    {account.roles?.map(r => <SelectItem key={r.id} value={String(r.id)}>{r.name}</SelectItem>)}
                                  </SelectContent>
                                </Select>
                                <FormMessage />
                              </FormItem>
                            )} />
                            <Button variant="ghost" size="icon" onClick={() => remove(index)} className="text-destructive shrink-0 hover:bg-destructive/5"><Trash2 className="h-4 w-4" /></Button>
                          </div>
                        ))}
                        {fields.length === 0 && (
                          <div className="h-40 border-2 border-dashed rounded-xl flex flex-col items-center justify-center text-muted-foreground">
                            <Mail className="h-8 w-8 mb-2 opacity-20" />
                            <p className="text-xs">{t('users.actions.states.no_accounts')}</p>
                          </div>
                        )}
                      </div>
                    </ScrollArea>
                  )}
                  {form.formState.errors.account_access_entries?.message && (
                    <p className="text-xs font-medium text-destructive pt-2">{form.formState.errors.account_access_entries.message as string}</p>
                  )}
                </div>
              </TabsContent>

              <TabsContent value="security" className="flex-1 overflow-hidden m-0 p-6 focus-visible:ring-0">
                <ScrollArea className="h-full pr-4">
                  <div className="max-w-2xl space-y-8">
                    <section className="space-y-4">
                      <div className="flex items-center justify-between">
                        <div className="flex items-center gap-2 font-medium text-sm"><Settings2 className="h-4 w-4" /> {t('users.actions.sections.policy')}</div>
                        <Switch checked={showAdvanced} onCheckedChange={setShowAdvanced} />
                      </div>

                      {!showAdvanced ? (
                        <div className="p-6 border rounded-lg bg-muted/20 text-center">
                          <p className="text-sm text-muted-foreground">{t('users.actions.states.security_disabled')}</p>
                        </div>
                      ) : (
                        <div className="space-y-6 animate-in fade-in slide-in-from-top-1">
                          <FormField control={form.control} name="acl.ip_whitelist" render={({ field }) => (
                            <FormItem>
                              <FormLabel>{t('users.actions.fields.ip_whitelist')}</FormLabel>
                              <FormControl>
                                <Textarea {...field} className="font-mono text-xs min-h-[140px] resize-none" placeholder="192.168.1.0/24" />
                              </FormControl>
                              <FormDescription>{t('users.actions.fields.ip_whitelist_hint')}</FormDescription>
                              <FormMessage />
                            </FormItem>
                          )} />

                          <div className="grid grid-cols-2 gap-4">
                            <FormField control={form.control} name="acl.rate_limit.quota" render={({ field }) => (
                              <FormItem><FormLabel>{t('users.actions.fields.rate_quota')}</FormLabel><FormControl><Input type="number" {...field} onChange={e => field.onChange(Number(e.target.value))} /></FormControl></FormItem>
                            )} />
                            <FormField control={form.control} name="acl.rate_limit.interval" render={({ field }) => (
                              <FormItem><FormLabel>{t('users.actions.fields.burst_interval')}</FormLabel><FormControl><Input type="number" {...field} onChange={e => field.onChange(Number(e.target.value))} /></FormControl></FormItem>
                            )} />
                          </div>
                        </div>
                      )}
                    </section>
                  </div>
                </ScrollArea>
              </TabsContent>
            </Tabs>
          </form>
        </Form>
        <DialogFooter className="p-4 px-6 border-t shrink-0 bg-muted/5">
          <Button variant="ghost" onClick={() => onOpenChange(false)}>{t('common.cancel')}</Button>
          <Button type="submit" form="user-form" disabled={isSaving} className="min-w-[120px]">
            {isSaving ? <Loader2 className="mr-2 h-4 w-4 animate-spin" /> : null}
            {isEdit ? t('users.actions.buttons.update') : t('users.actions.buttons.create')}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}