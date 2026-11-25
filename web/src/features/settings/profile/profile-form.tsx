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

import { useForm } from 'react-hook-form'
import { zodResolver } from '@hookform/resolvers/zod'
import { useMutation, useQueryClient } from '@tanstack/react-query'
import { Loader2 } from 'lucide-react'
import { AxiosError } from 'axios'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'

import {
  Form,
  FormControl,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
} from '@/components/ui/form'
import { Input } from '@/components/ui/input'
import { Button } from '@/components/ui/button'
import { toast } from '@/hooks/use-toast'
import { PasswordInput } from '@/components/password-input'
import { update_user, User } from '@/api/users/api'
import { Badge } from '@/components/ui/badge'
import { FileWithPreview } from '@/hooks/use-file-upload'
import AvatarUpload from './avatar-upload'
import { PermissionsDialog } from '../access/permissions-dialog'
import { profileSchema, type ProfileFormValues } from './schema'

function fileToBase64(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader()
    reader.onload = () => {
      const result = reader.result as string
      resolve(result.split(',')[1])
    }
    reader.onerror = reject
    reader.readAsDataURL(file)
  })
}

interface UserProfileFormProps {
  user: User
}

export function UserProfileForm({ user }: UserProfileFormProps) {
  const { t } = useTranslation()
  const queryClient = useQueryClient()
  const [avatarFile, setAvatarFile] = useState<FileWithPreview | null>(null)

  const [permissionsOpen, setPermissionsOpen] = useState(false)
  const [permissionsAccountId, setPermissionsAccountId] = useState<number | undefined>(undefined)

  const form = useForm<ProfileFormValues>({
    resolver: zodResolver(profileSchema(t)),
    mode: 'onChange',
    defaultValues: {
      username: user.username,
      email: user.email,
      password: '',
    },
  })

  const avatarSrc = user.avatar
    ? `data:image/png;base64,${user.avatar}`
    : undefined

  const mutation = useMutation({
    mutationFn: async (values: ProfileFormValues) => {
      let avatar_base64: string | undefined

      if (avatarFile?.file instanceof File) {
        avatar_base64 = await fileToBase64(avatarFile.file)
      }

      return update_user(user.id, {
        ...values,
        avatar_base64,
      })
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['current-user'] })
      toast({ title: t('settings.profile.toast.updated') })
    },
    onError: (err: AxiosError) => {
      toast({
        variant: 'destructive',
        title: t('settings.profile.toast.update_failed'),
        description: (err.response?.data as any)?.message || err.message,
      })
    },
  })

  const roleNames = user.global_roles_names

  return (
    <>
      <Form {...form}>
        <form
          onSubmit={form.handleSubmit((values) => mutation.mutate(values))}
          className="space-y-6 w-full max-w-xl mx-auto px-4 md:px-6"
        >
          <div className="space-y-6">
            <div className="space-y-6">
              <div className="flex justify-center">
                <AvatarUpload
                  onFileChange={setAvatarFile}
                  defaultAvatar={avatarSrc}
                  disabled={mutation.isPending}
                />
              </div>

              {roleNames && roleNames.length > 0 && (
                <div className="space-y-3">
                  <div className="flex items-center justify-between">
                    <div className="text-sm font-medium text-muted-foreground">
                      {t('settings.profile.section.roles')}
                    </div>
                    <Button
                      type="button"
                      variant="ghost"
                      size="sm"
                      className="text-xs"
                      onClick={() => {
                        setPermissionsAccountId(undefined)
                        setPermissionsOpen(true)
                      }}
                    >
                      {t('settings.profile.button.view_global_permissions')}
                    </Button>
                  </div>

                  <div className="flex flex-wrap gap-2">
                    {roleNames.map((role, index) => (
                      <Badge key={index} variant="secondary">
                        {role}
                      </Badge>
                    ))}
                  </div>
                </div>
              )}

              <FormField
                control={form.control}
                name="username"
                render={({ field }) => (
                  <FormItem>
                    <FormLabel>
                      {t('settings.profile.field.username')}
                    </FormLabel>
                    <FormControl>
                      <Input {...field} />
                    </FormControl>
                    <FormMessage />
                  </FormItem>
                )}
              />

              <FormField
                control={form.control}
                name="email"
                render={({ field }) => (
                  <FormItem>
                    <FormLabel>
                      {t('settings.profile.field.email')}
                    </FormLabel>
                    <FormControl>
                      <Input {...field} />
                    </FormControl>
                    <FormMessage />
                  </FormItem>
                )}
              />

              <FormField
                control={form.control}
                name="password"
                render={({ field }) => (
                  <FormItem>
                    <FormLabel>
                      {t('settings.profile.field.password')}
                    </FormLabel>
                    <FormControl>
                      <PasswordInput
                        placeholder={t(
                          'settings.profile.placeholder.password_keep',
                        )}
                        {...field}
                      />
                    </FormControl>
                    <FormMessage />
                  </FormItem>
                )}
              />
            </div>
          </div>

          <div className="flex justify-start pt-4">
            <Button
              type="submit"
              disabled={form.formState.isSubmitting || mutation.isPending}
            >
              {(form.formState.isSubmitting || mutation.isPending) && (
                <Loader2 className="mr-2 h-4 w-4 animate-spin" />
              )}
              {t('settings.profile.button.update_profile')}
            </Button>
          </div>
        </form>
      </Form>
      <PermissionsDialog
        currentRow={user}
        open={permissionsOpen}
        onOpenChange={setPermissionsOpen}
        mode="global"
        accountId={permissionsAccountId}
      />
    </>
  )
}
