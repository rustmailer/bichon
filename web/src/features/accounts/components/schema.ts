import { z } from 'zod'
import { isValidRegex } from '@/lib/pattern-utils'

const encryptionSchema = z.union([
  z.literal('Ssl'),
  z.literal('StartTls'),
  z.literal('None'),
])

const authTypeSchema = z.union([z.literal('Password'), z.literal('OAuth2')])

export const getAuthConfigSchema = (
  isEdit: boolean,
  t: (key: string) => string
) =>
  z
    .object({
      auth_type: authTypeSchema,
      password: z.string().optional(),
    })
    .refine(
      (data) => {
        if (data.auth_type === 'Password' && !isEdit) {
          return !!data.password?.trim()
        }
        return true
      },
      {
        message: t('validation.passwordRequired'),
        path: ['password'],
      }
    )

export const getImapConfigSchema = (
  isEdit: boolean,
  t: (key: string) => string
) =>
  z.object({
    host: z
      .string({ required_error: t('validation.imapHostRequired') })
      .min(1, { message: t('validation.imapHostCannotBeEmpty') }),
    port: z
      .number()
      .int()
      .min(0, { message: t('validation.imapPortMustBePositive') })
      .max(65535, { message: t('validation.imapPortMustBeLessThan65536') }),
    encryption: encryptionSchema,
    auth: getAuthConfigSchema(isEdit, t),
    use_proxy: z.number().optional(),
  })

const relativeDateSchema = (t: (key: string) => string) =>
  z.object({
    unit: z.enum(['Days', 'Months', 'Years'], {
      message: t('accounts.selectUnit'),
    }),
    value: z
      .number({ message: t('accounts.enterValue') })
      .int()
      .min(1, t('accounts.mustBeAtLeast1')),
  })

const dateSelectionSchema = (t: (key: string) => string) =>
  z
    .object({
      // nullish: the API returns the unused half as null
      fixed: z
        .string({ message: t('accounts.selectDate') })
        .min(1, { message: t('accounts.selectDate') })
        .nullish(),
      relative: relativeDateSchema(t).nullish(),
    })
    .optional()

const nonEmptyRuleItem = (t: (key: string) => string) =>
  z.string().refine((value) => value.trim() !== '', {
    message: t('validation.required'),
  })

const filterRuleSchema = (t: (key: string) => string) =>
  z.object({
    include: z.array(nonEmptyRuleItem(t)),
    exclude: z.array(nonEmptyRuleItem(t)),
  })

const regexFilterRuleSchema = (t: (key: string) => string) =>
  z.object({
    include: z.array(
      nonEmptyRuleItem(t).refine(isValidRegex, {
        message: t('validation.invalidRegex'),
      })
    ),
    exclude: z.array(
      nonEmptyRuleItem(t).refine(isValidRegex, {
        message: t('validation.invalidRegex'),
      })
    ),
  })

const archiveRulesSchema = (t: (key: string) => string) =>
  z.object({
    enabled: z.boolean(),
    senders: filterRuleSchema(t),
    subjects: filterRuleSchema(t),
    skip_larger_than: z.number().int().positive().optional(),
    spam_headers: z.array(nonEmptyRuleItem(t)),
  })

const extractionRulesSchema = (t: (key: string) => string) =>
  z
    .object({
      enabled: z.boolean(),
      // Exact match, stored verbatim - no regex validation (mirrors backend).
      extensions: filterRuleSchema(t),
      // Regex patterns, validated like the backend's ExtractionRules::validate().
      folders: regexFilterRuleSchema(t),
      attachment_names: regexFilterRuleSchema(t),
      senders: regexFilterRuleSchema(t),
    })
    .refine((rules) => !rules.enabled || rules.extensions.include.length > 0, {
      message: t('validation.requireExtractionExtension'),
      path: ['extensions'],
    })

export const getAccountSchema = (isEdit: boolean, t: (key: string) => string) =>
  z.object({
    account_name: z.string().optional(),
    login_name: z.string().optional(),
    email: z
      .string({ required_error: t('validation.emailRequired') })
      .email({ message: t('validation.invalidEmail') }),
    imap: getImapConfigSchema(isEdit, t),
    enabled: z.boolean(),
    use_dangerous: z.boolean(),
    date_since: dateSelectionSchema(t).optional(),
    date_before: relativeDateSchema(t).optional(),
    download_interval_min: z
      .number({
        invalid_type_error: t('validation.incrementalSyncMustBeNumber'),
      })
      .int()
      .min(1, {
        message: t('validation.incrementalSyncMustBeAtLeast1'),
      }),
    download_batch_size: z
      .number({
        invalid_type_error: t('validation.singleRequestBatchSizeMustBeNumber'),
      })
      .int()
      .min(10, {
        message: t('validation.singleRequestBatchSizeTooSmall'),
      })
      .max(200, {
        message: t('validation.singleRequestBatchSizeTooLarge'),
      }),
    max_email_size_bytes: z
      .number({
        invalid_type_error: t('validation.maxEmailSizeMustBeNumber'),
      })
      .int()
      .min(1 * 1024 * 1024, { message: t('validation.maxEmailSizeTooSmall') })
      .max(100 * 1024 * 1024, {
        message: t('validation.maxEmailSizeTooLarge'),
      }),
    auto_download_new_mailboxes: z.boolean(),
    download_schedule: z
      .string()
      .optional()
      .refine(
        (val) => {
          if (!val || val.trim() === '') return true
          const fields = val.trim().split(/\s+/)
          if (fields.length < 6) return false
          return true
        },
        { message: t('validation.invalidCronExpression') }
      ),
    archive_rules: archiveRulesSchema(t).optional(),
    extraction_rules: extractionRulesSchema(t).optional(),
    retention_days: z
      .number({
        invalid_type_error: t('validation.retentionDaysMustBeNumber'),
      })
      .int()
      .min(0, { message: t('validation.retentionDaysMustBeNonNegative') })
      .max(3650, { message: t('validation.retentionDaysTooLarge') })
      .optional(),
  })

export type AccountFormValues = z.infer<ReturnType<typeof getAccountSchema>>
