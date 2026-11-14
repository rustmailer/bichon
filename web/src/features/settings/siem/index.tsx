import { Eye, Loader2, Send, Trash2 } from 'lucide-react'
import { useEffect, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { useTranslation } from 'react-i18next'
import {
  get_siem_config,
  preview_siem_config,
  test_siem_config,
  update_siem_config,
  type SiemConfigView,
  type SiemPreview,
  type SiemUpdate,
} from '@/api/siem/api'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { PasswordInput } from '@/components/password-input'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { Switch } from '@/components/ui/switch'
import { Textarea } from '@/components/ui/textarea'
import { useEdition } from '@/hooks/use-edition'
import { useCurrentUser } from '@/hooks/use-current-user'
import { useToast } from '@/hooks/use-toast'

const CATEGORY_KEYS = ['login', 'delete', 'export', 'admin', 'repair'] as const

const FORMAT_OPTIONS: { value: string; i18nKey: string; fallback: string }[] = [
  {
    value: 'generic',
    i18nKey: 'Generic',
    fallback: 'Generic JSON envelope',
  },
  {
    value: 'splunk-hec',
    i18nKey: 'Splunk',
    fallback: 'Splunk HTTP Event Collector',
  },
  {
    value: 'elasticsearch-bulk',
    i18nKey: 'Es',
    fallback: 'Elasticsearch bulk (NDJSON)',
  },
]

export function SiemSettings() {
  const { t } = useTranslation()
  const { toast } = useToast()
  const { isEnterprise } = useEdition()
  const { canGlobal } = useCurrentUser()
  const queryClient = useQueryClient()

  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)
  const [testing, setTesting] = useState(false)
  const [previewing, setPreviewing] = useState(false)
  const [current, setCurrent] = useState<SiemConfigView | null>(null)

  const [enabled, setEnabled] = useState(false)
  const [url, setUrl] = useState('')
  const [authToken, setAuthToken] = useState('')
  const [hmacSecret, setHmacSecret] = useState('')
  const [clearToken, setClearToken] = useState(false)
  const [clearSecret, setClearSecret] = useState(false)
  const [categories, setCategories] = useState<string[]>([])
  const [format, setFormat] = useState('generic')
  const [mappingScript, setMappingScript] = useState('')
  const [previewResult, setPreviewResult] = useState<SiemPreview | null>(null)
  const [previewError, setPreviewError] = useState('')
  const [batchSize, setBatchSize] = useState('200')
  const [pollInterval, setPollInterval] = useState('2')
  const [retryAttempts, setRetryAttempts] = useState('5')
  const [retryBaseDelay, setRetryBaseDelay] = useState('1')
  const [retryMaxDelay, setRetryMaxDelay] = useState('60')

  const canManage =
    isEnterprise && (canGlobal('system:root') || canGlobal('user:manage'))

  useEffect(() => {
    let mounted = true
    get_siem_config()
      .then((cfg) => {
        if (!mounted) return
        setCurrent(cfg)
        setEnabled(cfg.enabled)
        setUrl(cfg.url ?? '')
        setCategories(cfg.categories)
        setFormat(cfg.format)
        setMappingScript(cfg.mapping_script ?? '')
        setBatchSize(String(cfg.batch_size))
        setPollInterval(String(cfg.poll_interval_secs))
        setRetryAttempts(String(cfg.retry_max_attempts))
        setRetryBaseDelay(String(cfg.retry_base_delay_secs))
        setRetryMaxDelay(String(cfg.retry_max_delay_secs))
      })
      .catch(() => {
        if (mounted) {
          toast({
            variant: 'destructive',
            title: t(
              'settings.siem.loadFailed',
              'Failed to load SIEM configuration',
            ),
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

  const toggleCategory = (cat: string) => {
    setCategories((prev) =>
      prev.includes(cat) ? prev.filter((c) => c !== cat) : [...prev, cat],
    )
  }

  const num = (value: string, fallback: number) => {
    const n = Number(value)
    return Number.isFinite(n) && n > 0 ? n : fallback
  }

  const handleSave = async () => {
    setSaving(true)
    try {
      const payload: SiemUpdate = {
        enabled,
        url: url.trim() || null,
        categories,
        format,
        mapping_script: mappingScript.trim() ? mappingScript : null,
        batch_size: num(batchSize, 200),
        poll_interval_secs: num(pollInterval, 2),
        retry_max_attempts: num(retryAttempts, 5),
        retry_base_delay_secs: num(retryBaseDelay, 1),
        retry_max_delay_secs: num(retryMaxDelay, 60),
      }
      // Secrets: omit (or "********") keeps the stored value, "" clears it.
      if (clearToken) {
        payload.auth_token = ''
      } else if (authToken.trim()) {
        payload.auth_token = authToken
      }
      if (clearSecret) {
        payload.hmac_secret = ''
      } else if (hmacSecret.trim()) {
        payload.hmac_secret = hmacSecret
      }
      const saved = await update_siem_config(payload)
      setCurrent(saved)
      queryClient.invalidateQueries({ queryKey: ['edition'] })
      setAuthToken('')
      setHmacSecret('')
      setClearToken(false)
      setClearSecret(false)
      toast({ title: t('settings.siem.saved', 'SIEM configuration saved') })
    } catch (err: any) {
      toast({
        variant: 'destructive',
        title: t('settings.siem.saveFailed', 'Failed to save SIEM configuration'),
        description: err?.response?.data?.message || err?.message,
      })
    } finally {
      setSaving(false)
    }
  }

  const handleTest = async () => {
    setTesting(true)
    try {
      const result = await test_siem_config()
      if (result.ok) {
        toast({
          title: t(
            'settings.siem.testOk',
            'Test event delivered (HTTP {{status}})',
            { status: result.status },
          ),
        })
      } else {
        toast({
          variant: 'destructive',
          title: t('settings.siem.testFailed', 'Test event failed'),
          description: result.error,
        })
      }
    } catch (err: any) {
      toast({
        variant: 'destructive',
        title: t('settings.siem.testFailed', 'Test event failed'),
        description: err?.response?.data?.message || err?.message,
      })
    } finally {
      setTesting(false)
    }
  }

  const handlePreview = async () => {
    setPreviewing(true)
    setPreviewError('')
    setPreviewResult(null)
    try {
      const result = await preview_siem_config({
        format,
        mapping_script: mappingScript.trim() ? mappingScript : null,
      })
      setPreviewResult(result)
    } catch (err: any) {
      setPreviewError(err?.response?.data?.message || err?.message)
    } finally {
      setPreviewing(false)
    }
  }

  if (!canManage) {
    return (
      <div className='w-full p-6 text-muted-foreground'>
        {t(
          'settings.siem.forbidden',
          'SIEM forwarding requires the Enterprise edition and system:root or user:manage permission.',
        )}
      </div>
    )
  }

  if (loading) {
    return (
      <div className='flex h-64 items-center justify-center'>
        <Loader2 className='h-6 w-6 animate-spin' />
      </div>
    )
  }

  return (
    <div className='w-full max-w-7xl space-y-6 px-4'>
      <div className='space-y-2'>
        <h2 className='text-xl font-bold'>
          {t('settings.siem.title', 'SIEM Webhook')}
        </h2>
        <p className='text-sm text-muted-foreground'>
          {t(
            'settings.siem.description',
            'Forward audit events to an external SIEM (Splunk, Elasticsearch, QRadar…) as JSON signed with an HMAC-SHA256 signature.',
          )}
        </p>
      </div>

      <div className='space-y-6 rounded-lg border p-6'>
        <div className='flex items-center justify-between'>
          <div className='space-y-1'>
            <Label>{t('settings.siem.enabled', 'Forwarding enabled')}</Label>
            <p className='text-xs text-muted-foreground'>
              {t(
                'settings.siem.enabledHint',
                'When enabled, matching audit events are pushed to the webhook URL.',
              )}
            </p>
          </div>
          <Switch checked={enabled} onCheckedChange={setEnabled} />
        </div>

        <div className='space-y-2'>
          <Label htmlFor='siem-url'>
            {t('settings.siem.url', 'Webhook URL')}
          </Label>
          <Input
            id='siem-url'
            value={url}
            placeholder='https://siem.example.com/hooks/bichon'
            onChange={(e) => setUrl(e.target.value)}
          />
        </div>

        <div className='space-y-2'>
          <Label>{t('settings.siem.format', 'Payload format')}</Label>
          <Select value={format} onValueChange={setFormat}>
            <SelectTrigger className='w-full sm:w-72'>
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {FORMAT_OPTIONS.map((opt) => (
                <SelectItem key={opt.value} value={opt.value}>
                  {t(`settings.siem.format${opt.i18nKey}`, opt.fallback)}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          <p className='text-xs text-muted-foreground'>
            {t(
              'settings.siem.formatHint',
              'Wire format of the forwarded payload. Generic is a plain JSON envelope; Splunk HEC expects an HTTP Event Collector endpoint; Elasticsearch bulk emits NDJSON for the _bulk API.',
            )}
          </p>
        </div>

        <div className='space-y-2'>
          <Label htmlFor='siem-mapping'>
            {t('settings.siem.mapping', 'Field mapping')}
          </Label>
          <Textarea
            id='siem-mapping'
            value={mappingScript}
            placeholder={t(
              'settings.siem.mappingPlaceholder',
              'set payload.host = "bichon"\nrename user = actor\ncopy seq = event_id\ndrop prev_hash',
            )}
            rows={6}
            onChange={(e) => setMappingScript(e.target.value)}
            className='font-mono text-xs'
          />
          <p className='text-xs text-muted-foreground'>
            {t(
              'settings.siem.mappingHint',
              'One rule per line: set <path> = "literal" · set <path> = {{field}} · copy <from> = <to> · rename <from> = <to> · drop <path> … Paths are dotted; {{field}} interpolates the string form, a bare {{field}} copies the value as-is. Rules run top to bottom on each event.',
            )}
          </p>
          <div className='flex flex-wrap items-center gap-2'>
            <Button
              type='button'
              variant='outline'
              size='sm'
              onClick={handlePreview}
              disabled={previewing}
            >
              {previewing ? (
                <Loader2 className='mr-2 h-4 w-4 animate-spin' />
              ) : (
                <Eye className='mr-2 h-4 w-4' />
              )}
              {t('settings.siem.testMapping', 'Preview payload')}
            </Button>
          </div>
          {previewError && (
            <p className='text-xs font-medium text-destructive'>
              {previewError}
            </p>
          )}
          {previewResult && (
            <pre className='max-h-72 overflow-auto whitespace-pre-wrap rounded-md bg-muted p-3 font-mono text-xs'>
              <span className='font-sans text-muted-foreground'>
                {previewResult.content_type} · {previewResult.events} events
              </span>
              {'\n'}
              {typeof previewResult.body === 'string'
                ? previewResult.body
                : JSON.stringify(previewResult.body, null, 2)}
            </pre>
          )}
        </div>

        <div className='grid gap-4 sm:grid-cols-2'>
          <div className='space-y-2'>
            <Label htmlFor='siem-token'>
              {t('settings.siem.authToken', 'Bearer token (optional)')}
            </Label>
            <PasswordInput
              id='siem-token'
              value={clearToken ? '' : authToken}
              placeholder={
                current?.auth_token_set
                  ? t(
                      'settings.siem.secretSetPlaceholder',
                      '******** (unchanged)',
                    )
                  : undefined
              }
              onChange={(e) => setAuthToken(e.target.value)}
            />
            {current?.auth_token_set && (
              <Button
                type='button'
                variant='ghost'
                size='sm'
                className='h-6 px-2 text-xs'
                onClick={() => setClearToken((v) => !v)}
              >
                <Trash2 className='mr-1 h-3 w-3' />
                {t('settings.siem.clearSecret', 'Clear')}
              </Button>
            )}
          </div>

          <div className='space-y-2'>
            <Label htmlFor='siem-secret'>
              {t('settings.siem.hmacSecret', 'HMAC-SHA256 signing secret')}
            </Label>
            <PasswordInput
              id='siem-secret'
              value={clearSecret ? '' : hmacSecret}
              placeholder={
                current?.hmac_secret_set
                  ? t(
                      'settings.siem.secretSetPlaceholder',
                      '******** (unchanged)',
                    )
                  : undefined
              }
              onChange={(e) => setHmacSecret(e.target.value)}
            />
            {current?.hmac_secret_set && (
              <Button
                type='button'
                variant='ghost'
                size='sm'
                className='h-6 px-2 text-xs'
                onClick={() => setClearSecret((v) => !v)}
              >
                <Trash2 className='mr-1 h-3 w-3' />
                {t('settings.siem.clearSecret', 'Clear')}
              </Button>
            )}
          </div>
        </div>
        <p className='text-xs text-muted-foreground'>
          {t(
            'settings.siem.hmacSecretHint',
            'The signature is sent in the X-Siem-Signature header as sha256=<hex> of the raw request body.',
          )}
        </p>

        <div className='space-y-2'>
          <Label>{t('settings.siem.categories', 'Event categories')}</Label>
          <div className='grid gap-3 sm:grid-cols-2'>
            {CATEGORY_KEYS.map((cat) => (
              <label
                key={cat}
                className='flex items-start gap-2 rounded-md border p-3'
              >
                <Checkbox
                  checked={categories.includes(cat)}
                  onCheckedChange={() => toggleCategory(cat)}
                />
                <div className='space-y-0.5'>
                  <p className='text-sm font-medium'>
                    {t(`settings.siem.cat${cat[0].toUpperCase()}${cat.slice(1)}`)}
                  </p>
                  <p className='text-xs text-muted-foreground'>
                    {t(`settings.siem.cat${cat[0].toUpperCase()}${cat.slice(1)}Hint`)}
                  </p>
                </div>
              </label>
            ))}
          </div>
        </div>

        <div className='grid gap-4 sm:grid-cols-2 lg:grid-cols-5'>
          <div className='space-y-2'>
            <Label htmlFor='siem-batch'>
              {t('settings.siem.batchSize', 'Batch size')}
            </Label>
            <Input
              id='siem-batch'
              type='number'
              min={1}
              value={batchSize}
              onChange={(e) => setBatchSize(e.target.value)}
            />
          </div>
          <div className='space-y-2'>
            <Label htmlFor='siem-poll'>
              {t('settings.siem.pollInterval', 'Poll interval (s)')}
            </Label>
            <Input
              id='siem-poll'
              type='number'
              min={1}
              value={pollInterval}
              onChange={(e) => setPollInterval(e.target.value)}
            />
          </div>
          <div className='space-y-2'>
            <Label htmlFor='siem-attempts'>
              {t('settings.siem.retryMaxAttempts', 'Max attempts')}
            </Label>
            <Input
              id='siem-attempts'
              type='number'
              min={1}
              value={retryAttempts}
              onChange={(e) => setRetryAttempts(e.target.value)}
            />
          </div>
          <div className='space-y-2'>
            <Label htmlFor='siem-basedelay'>
              {t('settings.siem.retryBaseDelay', 'Base delay (s)')}
            </Label>
            <Input
              id='siem-basedelay'
              type='number'
              min={1}
              value={retryBaseDelay}
              onChange={(e) => setRetryBaseDelay(e.target.value)}
            />
          </div>
          <div className='space-y-2'>
            <Label htmlFor='siem-maxdelay'>
              {t('settings.siem.retryMaxDelay', 'Max delay (s)')}
            </Label>
            <Input
              id='siem-maxdelay'
              type='number'
              min={1}
              value={retryMaxDelay}
              onChange={(e) => setRetryMaxDelay(e.target.value)}
            />
          </div>
        </div>
        <p className='text-xs text-muted-foreground'>
          {t(
            'settings.siem.retryHint',
            'Failed batches are retried with exponential backoff and are never dropped — the SIEM should deduplicate by the event seq.',
          )}
        </p>

        <div className='flex flex-wrap items-center justify-end gap-2'>
          <Button
            type='button'
            variant='outline'
            onClick={handleTest}
            disabled={testing || saving}
          >
            {testing ? (
              <Loader2 className='mr-2 h-4 w-4 animate-spin' />
            ) : (
              <Send className='mr-2 h-4 w-4' />
            )}
            {t('settings.siem.test', 'Send test event')}
          </Button>
          <Button type='button' onClick={handleSave} disabled={saving || testing}>
            {saving && <Loader2 className='mr-2 h-4 w-4 animate-spin' />}
            {t('settings.siem.save', 'Save configuration')}
          </Button>
        </div>
      </div>
    </div>
  )
}
