import { z } from 'zod'

const encryptionSchema = z.union([
  z.literal('Ssl'),
  z.literal('StartTls'),
  z.literal('None'),
])

const authTypeSchema = z.union([
  z.literal('Password'),
  z.literal('OAuth2'),
])

export const getAuthConfigSchema = (isEdit: boolean, t: (key: string) => string) =>
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

export const getImapConfigSchema = (isEdit: boolean, t: (key: string) => string) =>
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
      fixed: z
        .string({ message: t('accounts.selectDate') })
        .min(1, { message: t('accounts.selectDate') })
        .optional(),
      relative: relativeDateSchema(t).optional(),
    })
    .optional()

const filterRuleSchema = z.object({
  include: z.array(z.string()),
  exclude: z.array(z.string()),
})

const archiveRulesSchema = z.object({
  enabled: z.boolean(),
  senders: filterRuleSchema,
  subjects: filterRuleSchema,
  skip_larger_than: z.number().int().positive().optional(),
  spam_headers: z.array(z.string()),
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
    uidonly_enabled: z.boolean(),
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
        invalid_type_error: t(
          'validation.singleRequestBatchSizeMustBeNumber'
        ),
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
      .max(100 * 1024 * 1024, { message: t('validation.maxEmailSizeTooLarge') }),
    auto_download_new_mailboxes: z.boolean(),
    download_schedule: z
      .string()
      .optional()
      .refine(
        (val) => {
          if (!val || val.trim() === '') return true;
          const fields = val.trim().split(/\s+/);
          if (fields.length < 6) return false;
          return true;
        },
        { message: t('validation.invalidCronExpression') }
      ),
    archive_rules: archiveRulesSchema.optional(),
  })

export type AccountFormValues = z.infer<
  ReturnType<typeof getAccountSchema>
>
