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

import { useFormContext, useWatch } from "react-hook-form";
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
import { Button } from "@/components/ui/button";
import { ChevronDown, HelpCircle, Plus } from "lucide-react";
import { useState } from "react";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import { useEdition } from "@/hooks/use-edition";
import { RuleGroup } from "./rule-group";
import { FilterRuleEditor } from "./filter-rule-editor";
import { ExtensionPicker } from "./extension-picker";
import { AccountFormValues } from "./schema";

const SUGGESTED_OVERRIDE_HEADERS = [
  'X-AnonAddy-Original-Sender',
  'X-SimpleLogin-Original-From',
];

const SUGGESTED_SPAM_HEADERS = [
  'X-Spam-Flag',
  'X-Spam',
  'X-Spam-Status',
  'X-Barracuda-Spam-Status',
  'X-Barracuda-Spam-Flag',
  'X-MS-Exchange-Organization-SCL',
];

const EXTRACTION_EXTENSIONS = ['pdf', 'docx'] as const;

interface TabFiltersProps {
  /** Collapse the whole section behind an "Advanced settings" accordion (used on the create page). */
  collapsedByDefault?: boolean;
}

export function TabFilters({ collapsedByDefault = false }: TabFiltersProps = {}) {
  const { t } = useTranslation();
  const { control, setValue } = useFormContext<AccountFormValues>();
  const { isPro } = useEdition();

  const archiveRules = useWatch({ control, name: 'archive_rules' });
  const extractionRules = useWatch({ control, name: 'extraction_rules' });

  const archiveEnabled = archiveRules?.enabled ?? false;
  const extractionEnabled = extractionRules?.enabled ?? false;

  const toggleArchive = (checked: boolean) => {
    if (checked) {
      setValue('archive_rules', {
        enabled: true,
        senders: { include: [], exclude: [] },
        subjects: { include: [], exclude: [] },
        skip_larger_than: undefined,
        spam_headers: [],
      });
    } else {
      setValue('archive_rules', undefined);
    }
  };

  const toggleExtraction = (checked: boolean) => {
    if (checked) {
      setValue('extraction_rules', {
        enabled: true,
        extensions: { include: [...EXTRACTION_EXTENSIONS], exclude: [] },
        folders: { include: [], exclude: [] },
        attachment_names: { include: [], exclude: [] },
        senders: { include: [], exclude: [] },
      });
    } else {
      setValue('extraction_rules', undefined);
    }
  };

  const spamHeaders = archiveRules?.spam_headers ?? [];

  const addSpamHeader = (header: string) => {
    if (!spamHeaders.includes(header)) {
      setValue('archive_rules.spam_headers', [...spamHeaders, header]);
    }
  };

  const removeSpamHeader = (header: string) => {
    setValue('archive_rules.spam_headers', spamHeaders.filter((h) => h !== header));
  };

  const [newSpamHeader, setNewSpamHeader] = useState('');

  const handleAddCustomSpamHeader = () => {
    const trimmed = newSpamHeader.trim();
    if (trimmed && !spamHeaders.includes(trimmed)) {
      setValue('archive_rules.spam_headers', [...spamHeaders, trimmed]);
      setNewSpamHeader('');
    }
  };

  // Sender header override: hidden under its own collapsed section, because
  // only alias/forwarding users need it. The backend silently falls back to
  // the real From whenever a configured header is absent or invalid.
  const overrideHeaders = useWatch({ control, name: 'override_from_headers' }) ?? [];
  const [newOverrideHeader, setNewOverrideHeader] = useState('');

  const addOverrideHeader = (header: string) => {
    const trimmed = header.trim();
    if (
      trimmed &&
      !overrideHeaders.some((h) => h.toLowerCase() === trimmed.toLowerCase())
    ) {
      setValue('override_from_headers', [...overrideHeaders, trimmed], {
        shouldDirty: true,
      });
    }
  };

  const removeOverrideHeader = (header: string) => {
    setValue(
      'override_from_headers',
      overrideHeaders.filter((h) => h !== header),
      { shouldDirty: true }
    );
  };

  const handleAddOverrideHeader = () => {
    addOverrideHeader(newOverrideHeader);
    setNewOverrideHeader('');
  };

  const BYTES_PER_MB = 1024 * 1024;

  const rulesContent = (
    <div className="space-y-8">
      {/* Archive filtering */}
      <RuleGroup
        title={t('accounts.rules.archiveTitle', 'Archive Filtering')}
        description={t(
          'accounts.rules.archiveDesc',
          'When enabled, only emails matching the rules below are archived. When disabled, all emails are archived.'
        )}
        enabled={archiveEnabled}
        onToggle={toggleArchive}
      >
        <FilterRuleEditor
          path="archive_rules.senders"
          title={t('accounts.filters.senderFilter')}
          help={t('accounts.filters.senderFilterHelp')}
        />
        <FilterRuleEditor
          path="archive_rules.subjects"
          title={t('accounts.filters.subjectFilter')}
          help={t('accounts.filters.subjectFilterHelp')}
        />

        {/* Size Limit */}
        <div className="space-y-4 rounded-md border p-5">
          <div className="flex items-center gap-2">
            <h4 className="text-sm font-semibold">{t('accounts.filters.sizeLimit')}</h4>
          </div>
          <FormField
            control={control}
            name="archive_rules.skip_larger_than"
            render={({ field }) => (
              <FormItem>
                <FormLabel>{t('accounts.filters.skipLargerThan')}</FormLabel>
                <FormControl>
                  <div className="flex items-center gap-2">
                    <Input
                      type="number"
                      placeholder={t('accounts.filters.noLimit')}
                      className="max-w-[160px]"
                      value={field.value ? field.value / BYTES_PER_MB : ''}
                      onChange={(e) => {
                        const parsed = parseInt(e.target.value, 10);
                        field.onChange(isNaN(parsed) ? undefined : parsed * BYTES_PER_MB);
                      }}
                    />
                    <span className="text-sm text-muted-foreground">MB</span>
                  </div>
                </FormControl>
                <FormDescription>{t('accounts.filters.sizeLimitDesc')}</FormDescription>
                <FormMessage />
              </FormItem>
            )}
          />
        </div>

        {/* Spam Headers */}
        <div className="space-y-4 rounded-md border p-5">
          <div className="flex items-center gap-2">
            <h4 className="text-sm font-semibold">{t('accounts.filters.spamHeaders')}</h4>
            <Tooltip>
              <TooltipTrigger asChild>
                <HelpCircle className="h-3.5 w-3.5 text-muted-foreground" />
              </TooltipTrigger>
              <TooltipContent>
                {t('accounts.filters.spamHeadersHelp')}
              </TooltipContent>
            </Tooltip>
          </div>
          <div className="space-y-2">
            {spamHeaders.length > 0 ? (
              spamHeaders.map((header) => (
                <div key={header} className="flex items-center gap-2">
                  <div className="flex-1 rounded-md border bg-muted/50 px-3 py-1.5 text-sm font-mono">
                    {header}
                  </div>
                  <Button
                    variant="ghost"
                    size="icon"
                    className="h-7 w-7 shrink-0"
                    onClick={() => removeSpamHeader(header)}
                  >
                    <span className="text-muted-foreground">&#x2715;</span>
                  </Button>
                </div>
              ))
            ) : (
              <p className="text-xs text-muted-foreground italic">
                {t('accounts.filters.noSpamHeaders')}
              </p>
            )}
          </div>

          <div className="flex items-center gap-2">
            <Input
              className="h-8 text-sm max-w-[220px]"
              placeholder="X-Spam-Flag"
              value={newSpamHeader}
              onChange={(e) => setNewSpamHeader(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') {
                  e.preventDefault();
                  handleAddCustomSpamHeader();
                }
              }}
            />
            <Button
              variant="outline"
              type="button"
              size="sm"
              className="h-8 text-xs"
              onClick={handleAddCustomSpamHeader}
            >
              <Plus className="h-3 w-3 mr-1" />
              {t('accounts.filters.addHeader')}
            </Button>
          </div>

          <div>
            <p className="text-xs text-muted-foreground mb-2">{t('accounts.filters.suggestions')}</p>
            <div className="flex flex-wrap gap-1.5">
              {SUGGESTED_SPAM_HEADERS.filter((h) => !spamHeaders.includes(h)).map((header) => (
                <button
                  key={header}
                  type="button"
                  className="inline-flex items-center rounded-full border bg-background px-2.5 py-0.5 text-xs font-mono text-muted-foreground hover:text-foreground hover:border-primary/50 transition-colors"
                  onClick={() => addSpamHeader(header)}
                >
                  + {header}
                </button>
              ))}
            </div>
          </div>
        </div>
      </RuleGroup>

      {/* Attachment extraction (Pro) */}
      {isPro && (
        <RuleGroup
          title={t('accounts.rules.extractionTitle', 'Attachment Extraction')}
          description={t(
            'accounts.rules.extractionDesc',
            'When enabled, only attachments matching the rules below are extracted for full-text search. When disabled, attachment text is extracted for all attachments (default).'
          )}
          enabled={extractionEnabled}
          onToggle={toggleExtraction}
        >
          <ExtensionPicker
            path="extraction_rules.extensions.include"
            title={t('accounts.filters.extraction.extensions', 'File Extensions')}
            help={t(
              'accounts.filters.extraction.extensionsHelp',
              'Only attachments in the selected formats have their text extracted for full-text search.'
            )}
            options={EXTRACTION_EXTENSIONS}
          />
          <FilterRuleEditor
            path="extraction_rules.folders"
            title={t('accounts.filters.extraction.folders', 'Folders')}
            help={t(
              'accounts.filters.extraction.foldersHelp',
              'Only extract attachments from emails in folders matching these patterns.'
            )}
          />
          <FilterRuleEditor
            path="extraction_rules.attachment_names"
            title={t('accounts.filters.extraction.attachmentNames', 'Attachment Names')}
            help={t(
              'accounts.filters.extraction.attachmentNamesHelp',
              'Only extract attachments whose filename matches these patterns.'
            )}
          />
          <FilterRuleEditor
            path="extraction_rules.senders"
            title={t('accounts.filters.extraction.senders', 'Senders')}
            help={t(
              'accounts.filters.extraction.sendersHelp',
              'Only extract attachments from senders matching these patterns.'
            )}
          />
        </RuleGroup>
      )}

      {/* Sender header override — defaults to collapsed. */}
      <Collapsible defaultOpen={false}>
        <CollapsibleTrigger asChild>
          <button
            type="button"
            className="flex w-full items-center justify-between gap-3 rounded-md border bg-muted/30 px-4 py-3 text-left text-sm font-medium transition-colors hover:bg-muted/50"
          >
            <span>
              {t('accounts.filters.overrideTitle', 'Sender header override')}
            </span>
            <ChevronDown className="h-4 w-4 shrink-0 text-muted-foreground transition-transform data-[state=open]:rotate-180" />
          </button>
        </CollapsibleTrigger>
        <CollapsibleContent className="pt-4">
          <div className="space-y-4 rounded-md border p-5">
            <p className="text-xs text-muted-foreground">
              {t(
                'accounts.filters.overrideDesc',
                'When one of these headers contains a valid email address, it replaces the stored sender address (for example the original sender behind an alias). Missing or invalid values fall back to the real From header.'
              )}
            </p>
            <p className="text-xs text-muted-foreground">
              {t(
                'accounts.filters.overrideUntrusted',
                'These headers are set by the sender and are not verified. Changes apply to messages archived from now on; to apply them to messages that are already archived, rebuild the index.'
              )}
            </p>

            <div className="space-y-2">
              {overrideHeaders.length > 0 ? (
                overrideHeaders.map((header) => (
                  <div key={header} className="flex items-center gap-2">
                    <div className="flex-1 rounded-md border bg-muted/50 px-3 py-1.5 text-sm font-mono">
                      {header}
                    </div>
                    <Button
                      variant="ghost"
                      size="icon"
                      className="h-7 w-7 shrink-0"
                      onClick={() => removeOverrideHeader(header)}
                    >
                      <span className="text-muted-foreground">&#x2715;</span>
                    </Button>
                  </div>
                ))
              ) : (
                <p className="text-xs text-muted-foreground italic">
                  {t('accounts.filters.noOverrideHeaders', 'No headers configured.')}
                </p>
              )}
            </div>

            <div className="flex items-center gap-2">
              <Input
                className="h-8 text-sm max-w-[260px]"
                placeholder="X-AnonAddy-Original-Sender"
                value={newOverrideHeader}
                onChange={(e) => setNewOverrideHeader(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === 'Enter') {
                    e.preventDefault();
                    handleAddOverrideHeader();
                  }
                }}
              />
              <Button
                variant="outline"
                type="button"
                size="sm"
                className="h-8 text-xs"
                onClick={handleAddOverrideHeader}
              >
                <Plus className="h-3 w-3 mr-1" />
                {t('accounts.filters.addHeader')}
              </Button>
            </div>

            <div className="flex flex-wrap gap-1.5">
              {SUGGESTED_OVERRIDE_HEADERS.filter(
                (h) => !overrideHeaders.some((v) => v.toLowerCase() === h.toLowerCase())
              ).map((header) => (
                <button
                  key={header}
                  type="button"
                  className="inline-flex items-center rounded-full border bg-background px-2.5 py-0.5 text-xs font-mono text-muted-foreground hover:text-foreground hover:border-primary/50 transition-colors"
                  onClick={() => addOverrideHeader(header)}
                >
                  + {header}
                </button>
              ))}
            </div>
          </div>
        </CollapsibleContent>
      </Collapsible>
    </div>
  );

  if (collapsedByDefault) {
    return (
      <Collapsible defaultOpen={false}>
        <CollapsibleTrigger asChild>
          <button
            type="button"
            className="flex w-full items-center justify-between gap-3 rounded-md border bg-muted/30 px-4 py-3 text-left text-sm font-medium transition-colors hover:bg-muted/50"
          >
            <span>{t('accounts.rules.advancedSettings', 'Advanced Settings')}</span>
            {/* <span className="truncate text-xs font-normal text-muted-foreground">{summary}</span> */}
            <ChevronDown className="h-4 w-4 shrink-0 text-muted-foreground transition-transform data-[state=open]:rotate-180" />
          </button>
        </CollapsibleTrigger>
        <CollapsibleContent className="pt-4">
          {rulesContent}
        </CollapsibleContent>
      </Collapsible>
    );
  }

  return rulesContent;
}
