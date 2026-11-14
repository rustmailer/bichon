import { useQueryClient } from '@tanstack/react-query'
import { Loader2, Trash2, Upload } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import Logo from '@/assets/logo.svg'
import {
  delete_branding_logo,
  get_branding,
  resolveApiUrl,
  update_branding,
  type BrandingInfo,
  type BrandingUpdate,
} from '@/api/branding/api'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { useEdition } from '@/hooks/use-edition'
import { useCurrentUser } from '@/hooks/use-current-user'
import { useToast } from '@/hooks/use-toast'

const ACCEPTED_TYPES = [
  'image/svg+xml',
  'image/png',
  'image/jpeg',
  'image/webp',
]
const MAX_LOGO_BYTES = 1024 * 1024

function fileToBase64(
  file: File,
): Promise<{ mime: string; base64: string }> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader()
    reader.onload = () => {
      const result = String(reader.result ?? '')
      const comma = result.indexOf(',')
      resolve({
        mime: file.type || 'image/svg+xml',
        base64: comma >= 0 ? result.slice(comma + 1) : result,
      })
    }
    reader.onerror = reject
    reader.readAsDataURL(file)
  })
}

export function BrandingSettings() {
  const { t } = useTranslation()
  const { toast } = useToast()
  const queryClient = useQueryClient()
  const { isPro } = useEdition()
  const { canGlobal } = useCurrentUser()

  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)
  const [current, setCurrent] = useState<BrandingInfo | null>(null)
  const [companyName, setCompanyName] = useState('')
  const [tagline, setTagline] = useState('')
  const [logoFile, setLogoFile] = useState<File | null>(null)
  const [logoPreview, setLogoPreview] = useState<string | null>(null)
  const fileInputRef = useRef<HTMLInputElement>(null)

  const canManage =
    isPro && (canGlobal('system:root') || canGlobal('user:manage'))

  useEffect(() => {
    let mounted = true
    get_branding()
      .then((info) => {
        if (!mounted) return
        setCurrent(info)
        setCompanyName(info.company_name ?? '')
        setTagline(info.tagline ?? '')
      })
      .catch(() => {
        if (mounted) {
          toast({
            variant: 'destructive',
            title: t('settings.branding.loadFailed', 'Failed to load branding'),
          })
        }
      })
      .finally(() => {
        if (mounted) setLoading(false)
      })
    return () => {
      mounted = false
    }
  }, [t, toast])

  const handleFileSelected = (file: File | undefined) => {
    if (!file) return
    if (!ACCEPTED_TYPES.includes(file.type)) {
      toast({
        variant: 'destructive',
        title: t('settings.branding.logoTypeError', 'Unsupported logo type'),
        description: t(
          'settings.branding.logoTypeErrorDesc',
          'Use an SVG, PNG, JPEG or WebP image.',
        ),
      })
      return
    }
    if (file.size > MAX_LOGO_BYTES) {
      toast({
        variant: 'destructive',
        title: t('settings.branding.logoSizeError', 'Logo too large'),
        description: t(
          'settings.branding.logoSizeErrorDesc',
          'Maximum size is 1 MiB.',
        ),
      })
      return
    }
    setLogoFile(file)
    setLogoPreview(URL.createObjectURL(file))
  }

  const handleSave = async () => {
    setSaving(true)
    try {
      const payload: BrandingUpdate = {
        company_name: companyName.trim() || null,
        tagline: tagline.trim() || null,
      }
      if (logoFile) {
        const { mime, base64 } = await fileToBase64(logoFile)
        payload.logo_mime = mime
        payload.logo_base64 = base64
      }
      const saved = await update_branding(payload)
      setCurrent(saved)
      setLogoFile(null)
      setLogoPreview(null)
      queryClient.invalidateQueries({ queryKey: ['branding'] })
      toast({ title: t('settings.branding.saved', 'Branding saved') })
      // Refresh the page so the new branding (title, favicon, sidebar logo)
      // is applied everywhere without a manual reload.
      window.setTimeout(() => window.location.reload(), 800)
    } catch (err: any) {
      toast({
        variant: 'destructive',
        title: t('settings.branding.saveFailed', 'Failed to save branding'),
        description: err?.response?.data?.message || err?.message,
      })
    } finally {
      setSaving(false)
    }
  }

  const handleResetLogo = async () => {
    setSaving(true)
    try {
      const saved = await delete_branding_logo()
      setCurrent(saved)
      setLogoFile(null)
      setLogoPreview(null)
      queryClient.invalidateQueries({ queryKey: ['branding'] })
      toast({
        title: t('settings.branding.logoReset', 'Logo reset to default'),
      })
      window.setTimeout(() => window.location.reload(), 800)
    } catch (err: any) {
      toast({
        variant: 'destructive',
        title: t('settings.branding.resetFailed', 'Failed to reset logo'),
        description: err?.response?.data?.message || err?.message,
      })
    } finally {
      setSaving(false)
    }
  }

  if (!canManage) {
    return (
      <div className="w-full p-6 text-muted-foreground">
        {t(
          'settings.branding.forbidden',
          'Branding requires system:root or user:manage permission.',
        )}
      </div>
    )
  }

  if (loading) {
    return (
      <div className="flex h-64 items-center justify-center">
        <Loader2 className="h-6 w-6 animate-spin" />
      </div>
    )
  }

  const previewSrc = logoPreview
    ? logoPreview
    : current?.logo_url
      ? resolveApiUrl(current.logo_url)
      : Logo

  return (
    <div className="w-full max-w-7xl space-y-6 px-4">
      <div className="space-y-2">
        <h2 className="text-xl font-bold">
          {t('settings.branding.title', 'Branding')}
        </h2>
        <p className="text-sm text-muted-foreground">
          {t(
            'settings.branding.description',
            'Customize the logo, company name and tagline shown on the login page, sidebar and browser tab.',
          )}
        </p>
      </div>

      <div className="space-y-6 rounded-lg border p-6">
        <div className="space-y-2">
          <Label>{t('settings.branding.logo', 'Logo')}</Label>
          <div className="flex flex-wrap items-center gap-6">
            <div className="flex h-28 w-28 items-center justify-center rounded-lg border bg-muted">
              <img
                src={previewSrc}
                alt={t('settings.branding.logoPreviewAlt', 'Logo preview')}
                className="max-h-24 max-w-24 object-contain"
              />
            </div>
            <div className="space-y-2">
              <div className="flex gap-2">
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  onClick={() => fileInputRef.current?.click()}
                >
                  <Upload className="mr-2 h-4 w-4" />
                  {t('settings.branding.uploadLogo', 'Upload logo')}
                </Button>
                {(current?.logo_url || logoFile) && (
                  <Button
                    type="button"
                    variant="ghost"
                    size="sm"
                    onClick={handleResetLogo}
                  >
                    <Trash2 className="mr-2 h-4 w-4" />
                    {t('settings.branding.resetLogo', 'Reset to default')}
                  </Button>
                )}
              </div>
              <p className="text-xs text-muted-foreground">
                {t(
                  'settings.branding.logoHint',
                  'SVG recommended. PNG, JPEG and WebP allowed. Max 1 MiB.',
                )}
              </p>
              <input
                ref={fileInputRef}
                type="file"
                accept=".svg,.png,.jpg,.jpeg,.webp"
                className="hidden"
                onChange={(e) => handleFileSelected(e.target.files?.[0])}
              />
            </div>
          </div>
        </div>

        <div className="space-y-2">
          <Label htmlFor="branding-company">
            {t('settings.branding.companyName', 'Company name')}
          </Label>
          <Input
            id="branding-company"
            value={companyName}
            maxLength={80}
            placeholder="Bichon"
            onChange={(e) => setCompanyName(e.target.value)}
          />
        </div>

        <div className="space-y-2">
          <Label htmlFor="branding-tagline">
            {t('settings.branding.tagline', 'Tagline')}
          </Label>
          <Input
            id="branding-tagline"
            value={tagline}
            maxLength={200}
            placeholder={t(
              'settings.branding.taglinePlaceholder',
              'e.g. Self-hosted email archiving platform',
            )}
            onChange={(e) => setTagline(e.target.value)}
          />
        </div>

        <div className="flex justify-end">
          <Button type="button" onClick={handleSave} disabled={saving}>
            {saving && <Loader2 className="mr-2 h-4 w-4 animate-spin" />}
            {t('settings.branding.save', 'Save branding')}
          </Button>
        </div>
      </div>
    </div>
  )
}
