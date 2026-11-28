//
// Copyright (c) 2025-2026 rustmailer.com (https://rustmailer.com)
//
// This file is part of the Bichon Email Archiving Project
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as
// published by the Free Software Foundation, either version 3 of the
// License, or (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <http://www.gnu.org/licenses/>.
//
// Create / rotate credentials / delete for a provider connection. The secret
// is write-only: it is sent once and never read back, so the rotation dialog
// always starts empty.
import { useState } from 'react'
import { useMutation, useQueryClient } from '@tanstack/react-query'
import { useTranslation } from 'react-i18next'
import { errorMessage } from '@/lib/error'
import { toast } from '@/hooks/use-toast'
import {
  create_connection,
  delete_connection,
  update_connection,
  type ProviderConnectionView,
  type ProviderKind,
  type ProviderPreset,
} from '@/api/providers/api'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { Textarea } from '@/components/ui/textarea'

export function CreateConnectionDialog({
  open,
  onOpenChange,
  presets,
  onCreated,
}: {
  open: boolean
  onOpenChange: (open: boolean) => void
  presets: ProviderPreset[]
  onCreated: (connection: ProviderConnectionView) => void
}) {
  const { t } = useTranslation()
  const queryClient = useQueryClient()
  const [provider, setProvider] = useState<ProviderKind>('Graph')
  const [name, setName] = useState('')
  const [cloud, setCloud] = useState('global')
  const [tenantId, setTenantId] = useState('')
  const [adminEmail, setAdminEmail] = useState('')
  const [clientId, setClientId] = useState('')
  const [clientSecret, setClientSecret] = useState('')
  const [serviceAccountJson, setServiceAccountJson] = useState('')

  const reset = () => {
    setProvider('Graph')
    setName('')
    setCloud('global')
    setTenantId('')
    setAdminEmail('')
    setClientId('')
    setClientSecret('')
    setServiceAccountJson('')
  }

  const create = useMutation({
    mutationFn: () =>
      create_connection({
        provider,
        // Google has no sovereign-cloud variants: discovery and Gmail are
        // fixed endpoints, and the API accepts only the global preset.
        cloud: provider === 'Gmail' ? 'global' : cloud,
        name: name.trim(),
        credentials:
          provider === 'Gmail'
            ? { service_account_json: serviceAccountJson.trim() }
            : {
                client_id: clientId.trim(),
                client_secret: clientSecret,
              },
        tenant_id: provider === 'Graph' ? tenantId.trim() : null,
        admin_email: provider === 'Gmail' ? adminEmail.trim() : null,
      }),
    onSuccess: (created) => {
      toast({ title: t('providers.create.created', 'Connection created') })
      queryClient.invalidateQueries({ queryKey: ['provider-connections'] })
      onCreated(created)
      reset()
      onOpenChange(false)
    },
    onError: (error) => {
      toast({
        title: t('providers.create.failed', 'Could not create the connection'),
        description: errorMessage(error),
        variant: 'destructive',
      })
    },
  })

  const complete =
    name.trim() &&
    (provider === 'Graph'
      ? tenantId.trim() && clientId.trim() && clientSecret
      : adminEmail.trim() && serviceAccountJson.trim())

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className='sm:max-w-lg'>
        <DialogHeader>
          <DialogTitle>
            {provider === 'Gmail'
              ? t(
                  'providers.create.gmailTitle',
                  'Add a Google Workspace connection'
                )
              : t('providers.create.title', 'Add a Microsoft 365 connection')}
          </DialogTitle>
          <DialogDescription>
            {provider === 'Gmail'
              ? t(
                  'providers.create.gmailDesc',
                  'The connection holds a service account with domain-wide delegation. Discovery uses the Admin SDK; each mailbox is synced by impersonating its owner.'
                )
              : t(
                  'providers.create.desc',
                  'The connection holds the app registration one or more mailboxes authenticate with. Create the app in Entra ID first and grant it Mail.Read plus directory read.'
                )}
          </DialogDescription>
        </DialogHeader>
        <div className='grid gap-4'>
          <div className='grid gap-2'>
            <Label>{t('providers.create.provider', 'Provider')}</Label>
            <Select
              value={provider}
              onValueChange={(value) => setProvider(value as ProviderKind)}
            >
              <SelectTrigger>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value='Graph'>
                  {t('providers.create.microsoft', 'Microsoft 365')}
                </SelectItem>
                <SelectItem value='Gmail'>
                  {t('providers.create.google', 'Google Workspace')}
                </SelectItem>
              </SelectContent>
            </Select>
          </div>
          <div className='grid gap-2'>
            <Label htmlFor='provider-name'>
              {t('providers.create.name', 'Connection name')}
            </Label>
            <Input
              id='provider-name'
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder={t('providers.create.namePlaceholder', 'Contoso M365')}
            />
          </div>
          {provider === 'Graph' ? (
            <>
              <div className='grid gap-2'>
                <Label>{t('providers.create.cloud', 'Cloud partition')}</Label>
                <Select value={cloud} onValueChange={setCloud}>
                  <SelectTrigger>
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {presets.map((preset) => (
                      <SelectItem key={preset.id} value={preset.id}>
                        {preset.display_name}
                        {!preset.verified
                          ? ` · ${t('providers.unverified', 'not verified')}`
                          : ''}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>
              <div className='grid gap-2'>
                <Label htmlFor='provider-tenant'>
                  {t('providers.create.tenant', 'Tenant ID')}
                </Label>
                <Input
                  id='provider-tenant'
                  value={tenantId}
                  onChange={(e) => setTenantId(e.target.value)}
                  placeholder='contoso.onmicrosoft.com'
                />
              </div>
              <div className='grid gap-2'>
                <Label htmlFor='provider-client-id'>
                  {t('providers.create.clientId', 'Application (client) ID')}
                </Label>
                <Input
                  id='provider-client-id'
                  value={clientId}
                  onChange={(e) => setClientId(e.target.value)}
                />
              </div>
              <div className='grid gap-2'>
                <Label htmlFor='provider-client-secret'>
                  {t('providers.create.clientSecret', 'Client secret')}
                </Label>
                <Input
                  id='provider-client-secret'
                  type='password'
                  autoComplete='new-password'
                  value={clientSecret}
                  onChange={(e) => setClientSecret(e.target.value)}
                />
              </div>
            </>
          ) : (
            <>
              <div className='grid gap-2'>
                <Label htmlFor='provider-admin-email'>
                  {t(
                    'providers.create.adminEmail',
                    'Administrator email'
                  )}
                </Label>
                <Input
                  id='provider-admin-email'
                  type='email'
                  value={adminEmail}
                  onChange={(e) => setAdminEmail(e.target.value)}
                  placeholder='admin@example.com'
                />
              </div>
              <div className='grid gap-2'>
                <Label htmlFor='provider-service-account'>
                  {t(
                    'providers.create.serviceAccount',
                    'Service account key (JSON)'
                  )}
                </Label>
                <Textarea
                  id='provider-service-account'
                  rows={6}
                  className='font-mono text-xs'
                  value={serviceAccountJson}
                  onChange={(e) => setServiceAccountJson(e.target.value)}
                  placeholder='{ "type": "service_account", ... }'
                />
                <p className='text-xs text-muted-foreground'>
                  {t(
                    'providers.create.serviceAccountHint',
                    'Domain-wide delegation must be authorized for gmail.readonly and the directory scope. The key is stored encrypted and never shown again.'
                  )}
                </p>
              </div>
            </>
          )}
        </div>
        <DialogFooter>
          <Button variant='outline' onClick={() => onOpenChange(false)}>
            {t('common.cancel', 'Cancel')}
          </Button>
          <Button
            disabled={!complete || create.isPending}
            onClick={() => create.mutate()}
          >
            {t('common.create', 'Create')}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

export function CredentialsDialog({
  connection,
  onOpenChange,
}: {
  connection: ProviderConnectionView | null
  onOpenChange: (open: boolean) => void
}) {
  const { t } = useTranslation()
  const queryClient = useQueryClient()
  const [clientId, setClientId] = useState('')
  const [clientSecret, setClientSecret] = useState('')
  const [serviceAccountJson, setServiceAccountJson] = useState('')
  const isGmail = connection?.provider === 'Gmail'

  const rotate = useMutation({
    mutationFn: () =>
      update_connection(connection!.id, {
        credentials: isGmail
          ? { service_account_json: serviceAccountJson.trim() }
          : {
              client_id: clientId.trim(),
              client_secret: clientSecret,
            },
      }),
    onSuccess: () => {
      toast({
        title: t(
          'providers.credentials.saved',
          'Credentials updated; test the connection again'
        ),
      })
      queryClient.invalidateQueries({ queryKey: ['provider-connections'] })
      setClientId('')
      setClientSecret('')
      setServiceAccountJson('')
      onOpenChange(false)
    },
    onError: (error) => {
      toast({
        title: t(
          'providers.credentials.failed',
          'Could not update the credentials'
        ),
        description: errorMessage(error),
        variant: 'destructive',
      })
    },
  })

  const complete = isGmail
    ? serviceAccountJson.trim()
    : clientId.trim() && clientSecret

  return (
    <Dialog open={connection !== null} onOpenChange={onOpenChange}>
      <DialogContent className='sm:max-w-lg'>
        <DialogHeader>
          <DialogTitle>
            {t('providers.credentials.title', 'Update credentials')}
          </DialogTitle>
          <DialogDescription>
            {isGmail
              ? t(
                  'providers.credentials.gmailDesc',
                  'Paste the new service account JSON key. Accounts and their checkpoints are kept; the connection returns to "untested" until the next test.'
                )
              : t(
                  'providers.credentials.desc',
                  'Enter the new client ID and secret. Accounts and their cursors are kept; the connection returns to "untested" until the next test.'
                )}
          </DialogDescription>
        </DialogHeader>
        <div className='grid gap-4'>
          {isGmail ? (
            <div className='grid gap-2'>
              <Label htmlFor='rotate-service-account'>
                {t(
                  'providers.create.serviceAccount',
                  'Service account key (JSON)'
                )}
              </Label>
              <Textarea
                id='rotate-service-account'
                rows={6}
                className='font-mono text-xs'
                value={serviceAccountJson}
                onChange={(e) => setServiceAccountJson(e.target.value)}
              />
            </div>
          ) : (
            <>
              <div className='grid gap-2'>
                <Label htmlFor='rotate-client-id'>
                  {t('providers.create.clientId', 'Application (client) ID')}
                </Label>
                <Input
                  id='rotate-client-id'
                  value={clientId}
                  onChange={(e) => setClientId(e.target.value)}
                />
              </div>
              <div className='grid gap-2'>
                <Label htmlFor='rotate-client-secret'>
                  {t('providers.create.clientSecret', 'Client secret')}
                </Label>
                <Input
                  id='rotate-client-secret'
                  type='password'
                  autoComplete='new-password'
                  value={clientSecret}
                  onChange={(e) => setClientSecret(e.target.value)}
                />
              </div>
            </>
          )}
        </div>
        <DialogFooter>
          <Button variant='outline' onClick={() => onOpenChange(false)}>
            {t('common.cancel', 'Cancel')}
          </Button>
          <Button
            disabled={!complete || rotate.isPending}
            onClick={() => rotate.mutate()}
          >
            {t('common.save', 'Save')}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

export function DeleteConnectionDialog({
  connection,
  onOpenChange,
  onDeleted,
}: {
  connection: ProviderConnectionView | null
  onOpenChange: (open: boolean) => void
  onDeleted: (id: number) => void
}) {
  const { t } = useTranslation()
  const queryClient = useQueryClient()

  const remove = useMutation({
    mutationFn: () => delete_connection(connection!.id),
    onSuccess: () => {
      toast({ title: t('providers.deleted', 'Connection deleted') })
      queryClient.invalidateQueries({ queryKey: ['provider-connections'] })
      onDeleted(connection!.id)
      onOpenChange(false)
    },
    onError: (error) => {
      toast({
        title: t('providers.delete.failed', 'Could not delete the connection'),
        description: errorMessage(error),
        variant: 'destructive',
      })
    },
  })

  return (
    <Dialog open={connection !== null} onOpenChange={onOpenChange}>
      <DialogContent className='sm:max-w-md'>
        <DialogHeader>
          <DialogTitle>
            {t('providers.delete.title', 'Delete this connection?')}
          </DialogTitle>
          <DialogDescription>
            {t(
              'providers.delete.desc',
              'Only possible when no account still uses it. Delete or re-point its accounts first.'
            )}
          </DialogDescription>
        </DialogHeader>
        <DialogFooter>
          <Button variant='outline' onClick={() => onOpenChange(false)}>
            {t('common.cancel', 'Cancel')}
          </Button>
          <Button
            variant='destructive'
            disabled={remove.isPending}
            onClick={() => remove.mutate()}
          >
            {t('common.delete', 'Delete')}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
