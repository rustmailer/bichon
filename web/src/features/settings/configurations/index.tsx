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


import * as React from "react"
import { Card, CardContent, CardHeader, CardTitle, CardDescription } from "@/components/ui/card"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { ScrollArea } from "@/components/ui/scroll-area"
import { Skeleton } from "@/components/ui/skeleton"
import { Switch } from "@/components/ui/switch"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import {
  ShieldCheck,
  Server,
  Database,
  Activity,
  Zap,
  SlidersHorizontal,
  Lock
} from "lucide-react"
import {
  get_runtime_settings,
  get_system_configurations,
  reset_runtime_settings,
  update_runtime_settings,
  type RuntimeSettings,
  type RuntimeSettingsUpdate,
} from "@/api/system/api"
import { useQuery, useQueryClient } from "@tanstack/react-query"
import { useTranslation } from "react-i18next"
import { useToast } from "@/hooks/use-toast"

// const formatMB = (bytes?: number) => {
//   if (!bytes) return "—"
//   return `${(bytes / 1024 / 1024).toFixed(0)} MB`
// }

function BooleanBadge({ value }: { value: boolean }) {
  const { t } = useTranslation()
  return value ? (
    <Badge variant="secondary">{t("systemConfig.status.enabled")}</Badge>
  ) : (
    <Badge variant="outline" className="opacity-70">{t("systemConfig.status.disabled")}</Badge>
  )
}

function SettingRow({
  label,
  value,
  description,
}: {
  label: string
  value: React.ReactNode
  description?: string
}) {
  return (
    <div className="py-2.5 border-b border-border/40 last:border-0">
      <div className="grid grid-cols-[1fr_auto] items-center gap-4">
        <div className="text-xs font-medium text-muted-foreground font-mono">{label}</div>
        <div className="text-sm text-right font-medium">{value}</div>
      </div>
      {description && (
        <div className="mt-1 text-[11px] text-muted-foreground">{description}</div>
      )}
    </div>
  )
}

function SettingsCard({
  icon: Icon,
  title,
  description,
  children,
}: {
  icon: React.ElementType
  title: string
  description?: string
  children: React.ReactNode
}) {
  return (
    <Card className="h-full">
      <CardHeader className="flex flex-row items-center gap-3 py-4 px-5 border-b bg-muted/20">
        <div className="p-1.5 rounded-md bg-primary/10">
          <Icon className="h-3.5 w-3.5 text-primary" />
        </div>
        <div className="space-y-0.5">
          <CardTitle className="text-sm font-semibold">{title}</CardTitle>
          {description && (
            <CardDescription className="text-[11px]">{description}</CardDescription>
          )}
        </div>
      </CardHeader>
      <CardContent className="px-5 py-1">{children}</CardContent>
    </Card>
  )
}

function PageSkeleton() {
  return (
    <div className="grid grid-cols-1 md:grid-cols-2 gap-5">
      {Array.from({ length: 6 }).map((_, i) => (
        <Card key={i}>
          <CardHeader className="py-4 px-5 border-b">
            <Skeleton className="h-4 w-40" />
            <Skeleton className="h-3 w-56" />
          </CardHeader>
          <CardContent className="px-5 py-3 space-y-3">
            <Skeleton className="h-3 w-full" />
            <Skeleton className="h-3 w-5/6" />
            <Skeleton className="h-3 w-4/6" />
          </CardContent>
        </Card>
      ))}
    </div>
  )
}

function SettingField({
  label,
  locked = false,
  children,
}: {
  label: string
  locked?: boolean
  children: React.ReactNode
}) {
  const { t } = useTranslation()
  return (
    <div className="flex items-center justify-between gap-4 py-2.5 border-b border-border/40 last:border-0">
      <span className="flex items-center gap-1.5 text-xs font-medium text-muted-foreground font-mono">
        {label}
        {locked && (
          <span
            className="inline-flex shrink-0"
            title={t("systemConfig.runtime.lockedByStartup")}
            aria-label={t("systemConfig.runtime.lockedByStartup")}
            role="img"
          >
            <Lock className="h-3 w-3" />
          </span>
        )}
      </span>
      <div className={locked ? "shrink-0 opacity-60" : "shrink-0"}>
        {children}
      </div>
    </div>
  )
}

function RuntimeSettingsCard() {
  const { t } = useTranslation()
  const { toast } = useToast()
  const queryClient = useQueryClient()
  const { data } = useQuery({
    queryKey: ["runtime-settings"],
    queryFn: get_runtime_settings,
  })
  const [form, setForm] = React.useState<RuntimeSettings | null>(null)
  const [proxyText, setProxyText] = React.useState("")
  const [saving, setSaving] = React.useState(false)

  React.useEffect(() => {
    if (data) {
      setForm(data)
      setProxyText(data.trusted_proxies.join(", "))
    }
  }, [data])

  if (!form) {
    return (
      <Card>
        <CardHeader className="flex flex-row items-center gap-3 py-4 px-5 border-b bg-muted/20">
          <Skeleton className="h-6 w-6 rounded-md" />
          <Skeleton className="h-4 w-48" />
        </CardHeader>
        <CardContent className="px-5 py-4 space-y-3">
          <Skeleton className="h-3 w-full" />
          <Skeleton className="h-3 w-5/6" />
          <Skeleton className="h-3 w-4/6" />
        </CardContent>
      </Card>
    )
  }

  const patch = (next: Partial<RuntimeSettings>) =>
    setForm((current) => (current ? { ...current, ...next } : current))

  const num = (raw: string, fallback: number) => {
    const n = Number(raw)
    return Number.isFinite(n) ? n : fallback
  }

  const locked = new Set(form.locked_by_startup)
  const splitProxies = (raw: string) =>
    raw
      .split(",")
      .map((entry) => entry.trim())
      .filter(Boolean)
  // A light pre-check only; the server validates the full IP/CIDR grammar.
  const proxyEntryLooksValid = (entry: string) =>
    entry === "*" || /^[0-9a-fA-F:.]+(\/\d{1,3})?$/.test(entry)

  // Mirror the server-side bounds so a bad value is caught before the request.
  const valid = {
    proxies: splitProxies(proxyText).every(proxyEntryLooksValid),
    smtp_port: form.smtp_port >= 1 && form.smtp_port <= 65535,
    token:
      form.webui_token_expiration_hours >= 1 &&
      form.webui_token_expiration_hours <= 8760,
    sync:
      form.sync_concurrency == null ||
      (form.sync_concurrency >= 1 && form.sync_concurrency <= 1024),
    imap:
      form.imap_timeout_seconds >= 0 && form.imap_timeout_seconds <= 86400,
    mbox:
      form.web_mbox_upload_limit_mb >= 1 &&
      form.web_mbox_upload_limit_mb <= 102400,
    pst:
      form.web_pst_upload_limit_mb >= 1 &&
      form.web_pst_upload_limit_mb <= 102400,
  }
  const allValid = Object.values(valid).every(Boolean)
  const fieldClass = (ok: boolean, extra = "") =>
    `h-8 w-28 text-right ${ok ? "" : "border-destructive focus-visible:ring-destructive"} ${extra}`.trim()

  const applyResult = async (
    result: Awaited<ReturnType<typeof update_runtime_settings>>,
    savedKey: string
  ) => {
    setForm(result.settings)
    setProxyText(result.settings.trusted_proxies.join(", "))
    // The response already carries the effective values; refresh the cache in
    // the background instead of making Save wait for a second round trip.
    void queryClient.invalidateQueries({ queryKey: ["runtime-settings"] })
    toast({
      title: t(savedKey),
      description: result.restart_required
        ? t("systemConfig.runtime.restartRequired")
        : undefined,
    })
  }

  const save = async () => {
    if (!allValid) {
      toast({
        variant: "destructive",
        title: t("common.op_failed"),
        description: t("systemConfig.runtime.invalid"),
      })
      return
    }
    setSaving(true)
    const payload: RuntimeSettingsUpdate = {
      smtp_enabled: form.smtp_enabled,
      smtp_port: form.smtp_port,
      smtp_encryption: form.smtp_encryption,
      smtp_auth_required: form.smtp_auth_required,
      webui_token_expiration_hours: form.webui_token_expiration_hours,
      sync_concurrency: form.sync_concurrency,
      imap_timeout_seconds: form.imap_timeout_seconds,
      web_mbox_upload_limit_mb: form.web_mbox_upload_limit_mb,
      web_pst_upload_limit_mb: form.web_pst_upload_limit_mb,
      sanitize_email_html: form.sanitize_email_html,
      trusted_proxies: splitProxies(proxyText),
    }
    try {
      await applyResult(
        await update_runtime_settings(payload),
        "systemConfig.runtime.saved"
      )
    } catch (err) {
      toast({
        variant: "destructive",
        title: t("common.op_failed"),
        description: err instanceof Error ? err.message : String(err),
      })
    } finally {
      setSaving(false)
    }
  }

  const reset = async () => {
    setSaving(true)
    try {
      await applyResult(await reset_runtime_settings(), "systemConfig.runtime.resetDone")
    } catch (err) {
      toast({
        variant: "destructive",
        title: t("common.op_failed"),
        description: err instanceof Error ? err.message : String(err),
      })
    } finally {
      setSaving(false)
    }
  }

  return (
    <SettingsCard
      icon={SlidersHorizontal}
      title={t("systemConfig.runtime.title")}
      description={t("systemConfig.runtime.desc")}
    >
      <div className="grid grid-cols-1 lg:grid-cols-2 gap-x-8">
        <SettingField
          label="BICHON_ENABLE_SMTP"
          locked={locked.has("smtp_enabled")}
        >
          <Switch
            checked={form.smtp_enabled}
            disabled={locked.has("smtp_enabled")}
            onCheckedChange={(checked) => patch({ smtp_enabled: checked })}
          />
        </SettingField>
        <SettingField
          label="BICHON_SMTP_PORT"
          locked={locked.has("smtp_port")}
        >
          <Input
            type="number"
            min={1}
            max={65535}
            disabled={locked.has("smtp_port")}
            aria-invalid={!valid.smtp_port}
            className={fieldClass(valid.smtp_port)}
            value={form.smtp_port}
            onChange={(e) => patch({ smtp_port: num(e.target.value, form.smtp_port) })}
          />
        </SettingField>
        <SettingField
          label="BICHON_SMTP_ENCRYPTION"
          locked={locked.has("smtp_encryption")}
        >
          <Select
            value={form.smtp_encryption}
            disabled={locked.has("smtp_encryption")}
            onValueChange={(value) =>
              patch({ smtp_encryption: value as RuntimeSettings["smtp_encryption"] })
            }
          >
            <SelectTrigger className="h-8 w-32">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="none">none</SelectItem>
              <SelectItem value="starttls">starttls</SelectItem>
              <SelectItem value="tls">tls</SelectItem>
            </SelectContent>
          </Select>
        </SettingField>
        <SettingField
          label="BICHON_SMTP_AUTH_REQUIRED"
          locked={locked.has("smtp_auth_required")}
        >
          <Switch
            checked={form.smtp_auth_required}
            disabled={locked.has("smtp_auth_required")}
            onCheckedChange={(checked) => patch({ smtp_auth_required: checked })}
          />
        </SettingField>
        <SettingField
          label="BICHON_WEBUI_TOKEN_EXPIRATION_HOURS"
          locked={locked.has("webui_token_expiration_hours")}
        >
          <Input
            type="number"
            min={1}
            max={8760}
            disabled={locked.has("webui_token_expiration_hours")}
            aria-invalid={!valid.token}
            className={fieldClass(valid.token)}
            value={form.webui_token_expiration_hours}
            onChange={(e) =>
              patch({
                webui_token_expiration_hours: num(
                  e.target.value,
                  form.webui_token_expiration_hours
                ),
              })
            }
          />
        </SettingField>
        <SettingField label="BICHON_SYNC_CONCURRENCY">
          <Input
            type="number"
            min={0}
            max={1024}
            aria-invalid={!valid.sync}
            className={fieldClass(valid.sync)}
            placeholder={t("systemConfig.status.auto")}
            value={form.sync_concurrency ?? ""}
            onChange={(e) =>
              patch({
                sync_concurrency:
                  e.target.value === "" ? null : num(e.target.value, 0),
              })
            }
          />
        </SettingField>
        <SettingField label="BICHON_IMAP_TIMEOUT_SECONDS">
          <Input
            type="number"
            min={0}
            max={86400}
            aria-invalid={!valid.imap}
            className={fieldClass(valid.imap)}
            value={form.imap_timeout_seconds}
            onChange={(e) =>
              patch({
                imap_timeout_seconds: num(
                  e.target.value,
                  form.imap_timeout_seconds
                ),
              })
            }
          />
        </SettingField>
        <SettingField label="BICHON_WEB_MBOX_UPLOAD_LIMIT_MB">
          <Input
            type="number"
            min={1}
            max={102400}
            aria-invalid={!valid.mbox}
            className={fieldClass(valid.mbox)}
            value={form.web_mbox_upload_limit_mb}
            onChange={(e) =>
              patch({
                web_mbox_upload_limit_mb: num(
                  e.target.value,
                  form.web_mbox_upload_limit_mb
                ),
              })
            }
          />
        </SettingField>
        <SettingField label="BICHON_WEB_PST_UPLOAD_LIMIT_MB">
          <Input
            type="number"
            min={1}
            max={102400}
            aria-invalid={!valid.pst}
            className={fieldClass(valid.pst)}
            value={form.web_pst_upload_limit_mb}
            onChange={(e) =>
              patch({
                web_pst_upload_limit_mb: num(
                  e.target.value,
                  form.web_pst_upload_limit_mb
                ),
              })
            }
          />
        </SettingField>
      </div>
      <div className="py-2.5 border-b border-border/40">
        <div className="flex items-center justify-between gap-4">
          <div className="min-w-0">
            <span className="text-xs font-medium text-muted-foreground font-mono">
              SANITIZE_EMAIL_HTML
            </span>
            <p className="mt-1 text-[11px] text-muted-foreground">
              {t("systemConfig.runtime.sanitizeEmailHtmlHelp")}
            </p>
          </div>
          <Switch
            checked={form.sanitize_email_html}
            onCheckedChange={(checked) =>
              patch({ sanitize_email_html: checked })
            }
          />
        </div>
      </div>
      <div className="py-2.5 border-b border-border/40">
        <div className="flex items-center gap-1.5">
          <span className="text-xs font-medium text-muted-foreground font-mono">
            BICHON_TRUSTED_PROXIES
          </span>
          {locked.has("trusted_proxies") && (
            <span
              className="inline-flex shrink-0"
              title={t("systemConfig.runtime.lockedByStartup")}
              aria-label={t("systemConfig.runtime.lockedByStartup")}
              role="img"
            >
              <Lock className="h-3 w-3" />
            </span>
          )}
        </div>
        <Input
          disabled={locked.has("trusted_proxies")}
          aria-invalid={!valid.proxies}
          className={`mt-1.5 h-8 ${
            valid.proxies ? "" : "border-destructive focus-visible:ring-destructive"
          }`.trim()}
          placeholder="127.0.0.1, 10.0.0.0/8"
          value={proxyText}
          onChange={(e) => setProxyText(e.target.value)}
        />
        <p className="mt-1 text-[11px] text-muted-foreground">
          {t("systemConfig.runtime.trustedProxiesHelp")}
        </p>
      </div>
      <div className="flex items-center justify-end gap-2 border-t py-3">
        <Button variant="outline" size="sm" onClick={reset} disabled={saving}>
          {t("common.reset")}
        </Button>
        <Button size="sm" onClick={save} disabled={saving}>
          {t("common.save")}
        </Button>
      </div>
    </SettingsCard>
  )
}

export default function ServerConfigurationsPage() {
  const { t } = useTranslation()
  const { data, isLoading, isError } = useQuery({
    queryKey: ["system-configurations"],
    queryFn: get_system_configurations,
  })

  if (isError || (!isLoading && !data)) {
    return (
      <div className="p-8 text-center text-sm text-destructive">
        {t("systemConfig.fields.loadError")}
      </div>
    )
  }

  return (
    <div className="w-full">
      <ScrollArea className="h-full w-full">
        <div className="max-w-5xl mx-auto px-6 py-8 space-y-6">

          {isLoading ? (
            <PageSkeleton />
          ) : (
            <div className="space-y-6">
              <RuntimeSettingsCard />
              <div className="grid grid-cols-1 lg:grid-cols-2 gap-5">
              <SettingsCard
                icon={Server}
                title={t("systemConfig.sections.network.title")}
                description={t("systemConfig.sections.network.desc")}
              >
                <SettingRow label="BICHON_BIND_IP" value={data!.bichon_bind_ip ?? "0.0.0.0"} />
                <SettingRow label="BICHON_HTTP_PORT" value={data!.bichon_http_port} />
                <SettingRow label="BICHON_BASE_URL" value={data!.bichon_base_url} />
                <SettingRow label="BICHON_PUBLIC_URL" value={data!.bichon_public_url} />
                <SettingRow
                  label="BICHON_ENABLE_REST_HTTPS"
                  value={<BooleanBadge value={data!.bichon_enable_rest_https} />}
                />
              </SettingsCard>

              <SettingsCard
                icon={Zap}
                title={t("systemConfig.sections.performance.title")}
                description={t("systemConfig.sections.performance.desc")}
              >
                <SettingRow
                  label="BICHON_HTTP_COMPRESSION_ENABLED"
                  value={<BooleanBadge value={data!.bichon_http_compression_enabled} />}
                />
              </SettingsCard>

              <SettingsCard
                icon={Database}
                title={t("systemConfig.sections.storage.title")}
                description={t("systemConfig.sections.storage.desc")}
              >
                <SettingRow label="BICHON_ROOT_DIR" value={<span className="font-mono">{data!.bichon_root_dir}</span>} />
                <SettingRow label="BICHON_DATA_DIR" value={data!.bichon_data_dir ? <span className="font-mono">{data!.bichon_data_dir}</span> : "—"} />
                <SettingRow label="BICHON_INDEX_DIR" value={data!.bichon_index_dir ? <span className="font-mono">{data!.bichon_index_dir}</span> : "—"} />
              </SettingsCard>

              <SettingsCard
                icon={ShieldCheck}
                title={t("systemConfig.sections.security.title")}
                description={t("systemConfig.sections.security.desc")}
              >
                <SettingRow
                  label="BICHON_ENCRYPT_PASSWORD_SET"
                  value={
                    data!.bichon_encrypt_password_set ? (
                      <Badge variant="secondary" className="bg-emerald-500/10 text-emerald-600 border-emerald-500/20">
                        {t("systemConfig.status.configured")}
                      </Badge>
                    ) : (
                      <Badge variant="destructive">{t("systemConfig.status.missing")}</Badge>
                    )
                  }
                />
              </SettingsCard>

              <SettingsCard
                icon={Activity}
                title={t("systemConfig.sections.logging.title")}
                description={t("systemConfig.sections.logging.desc")}
              >
                <SettingRow label="BICHON_LOG_LEVEL" value={<Badge variant="outline" className="uppercase">{data!.bichon_log_level}</Badge>} />
                <SettingRow label="BICHON_ANSI_LOGS" value={<BooleanBadge value={data!.bichon_ansi_logs} />} />
                <SettingRow label="BICHON_JSON_LOGS" value={<BooleanBadge value={data!.bichon_json_logs} />} />
                <SettingRow label="BICHON_LOG_TO_FILE" value={<BooleanBadge value={data!.bichon_log_to_file} />} />
                <SettingRow label="BICHON_MAX_SERVER_LOG_FILES" value={data!.bichon_max_server_log_files} />
              </SettingsCard>
              </div>
            </div>
          )}

        </div>
      </ScrollArea>
    </div>
  )
}