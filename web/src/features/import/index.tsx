//
// Copyright (c) 2025-2026 rustmailer.com (https://rustmailer.com)
//
// This file is part of the Bichon Email Archiving Project

import { useState, useRef, useCallback, useEffect } from 'react';
import { useTranslation } from 'react-i18next';
import { useMutation, useQuery } from '@tanstack/react-query';
import {
  Upload, FileText, X, CheckCircle2, AlertTriangle,
  Sparkles, PenLine, ListTree, ChevronsUpDown, Check,
  Clock, ChevronRight,
} from 'lucide-react';

import { Button } from '@/components/ui/button';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { Progress } from '@/components/ui/progress';
import { ScrollArea } from '@/components/ui/scroll-area';
import { RadioGroup, RadioGroupItem } from '@/components/ui/radio-group';
import { cn } from '@/lib/utils';
import { Main } from '@/components/layout/main';
import { FixedHeader } from '@/components/layout/fixed-header';
import { useToast } from '@/hooks/use-toast';
import { Badge } from '@/components/ui/badge';
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover';
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from '@/components/ui/command';

import {
  upload_import,
  get_import_progress,
  get_import_accounts,
  list_import_history,
  type ImportProgress,
  type ImportHistory,
} from '@/api/import/api';
import { get_system_configurations } from '@/api/system/api';
import { list_mailboxes } from '@/api/mailbox/api';
import { extractFolderHint, type FolderHint } from './folder-hint';

const MAX_EML = 100 * 1024 * 1024;   // 100 MB (hardcoded)
const DEFAULT_MAX_MBOX = 1024 * 1024 * 1024; // 1 GB (fallback; actual limit from server settings)
const DEFAULT_MAX_PST = 2048 * 1024 * 1024; // 2 GB (fallback; actual limit from server settings)

// MIME types that are clearly NOT email files — reject these upfront.
const BLOCKED_MIME_PREFIXES = [
  'video/', 'audio/', 'image/', 'font/',
  'application/zip', 'application/gzip', 'application/x-tar',
  'application/x-7z', 'application/x-rar',
  'application/vnd.', 'application/pdf',
  'application/x-msdownload', 'application/x-executable',
];

function isValidFileType(file: File, ext: string): boolean {
  // Check MIME type: reject known binary types
  const mime = file.type.toLowerCase();
  if (mime) {
    for (const prefix of BLOCKED_MIME_PREFIXES) {
      if (mime.startsWith(prefix)) return false;
    }
  }
  // Check extension
  return ext === 'eml' || ext === 'mbox' || ext === 'pst';
}

type FolderMode = '' | 'header' | 'existing' | 'custom';

interface QueuedFile {
  file: File;
  sizeOk: boolean;
  typeOk: boolean;
}

function formatSize(bytes: number) {
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(2)} GB`;
}

function folderHintLabel(hint: FolderHint): string {
  switch (hint.source) {
    case 'gmail-labels': return 'X-Gmail-Labels';
    case 'bichon-metadata': return 'X-Bichon-Metadata';
    case 'filename': return 'filename';
    case 'mbox-filename': return 'mbox filename';
    case 'pst-filename': return 'PST filename';
  }
}

export default function ImportPage() {
  const { t } = useTranslation();
  const { toast } = useToast();

  const [accountId, setAccountId] = useState<string>('');
  const [folderMode, setFolderMode] = useState<FolderMode>('');
  const [folder, setFolder] = useState('INBOX');
  const [files, setFiles] = useState<QueuedFile[]>([]);
  const [dragging, setDragging] = useState(false);
  // const [importId, setImportId] = useState<string | null>(null);
  const [progress, setProgress] = useState<ImportProgress | null>(null);
  const [uploadPct, setUploadPct] = useState(0);
  const [phase, setPhase] = useState<'idle' | 'uploading' | 'processing' | 'done'>('idle');
  const [folderHint, setFolderHint] = useState<FolderHint | null>(null);
  const [headerFolder, setHeaderFolder] = useState('INBOX');
  const [isPstSelected, setIsPstSelected] = useState(false);

  // Combobox state for existing mailbox selection
  const [mailboxOpen, setMailboxOpen] = useState(false);
  // Combobox state for account selection
  const [accountOpen, setAccountOpen] = useState(false);

  const pollRef = useRef<ReturnType<typeof setInterval> | null>(null);

  const { data: accounts = [] } = useQuery({
    queryKey: ['nosync-accounts'],
    queryFn: get_import_accounts,
    staleTime: 30_000,
  });

  const { data: mailboxData } = useQuery({
    queryKey: ['account-mailboxes', accountId],
    queryFn: () => list_mailboxes(Number(accountId), false),
    enabled: !!accountId,
    staleTime: 60_000,
  });
  const mailboxes = mailboxData?.mailboxes ?? [];

  // Fetch system config to get the configured MBOX/PST upload limits.
  // Falls back to defaults for non-root users or on error.
  const { data: sysConfig } = useQuery({
    queryKey: ['system-configurations'],
    queryFn: get_system_configurations,
    staleTime: 300_000,
    retry: false,
  });
  const maxMbox = sysConfig
    ? sysConfig.bichon_web_mbox_upload_limit_mb * 1024 * 1024
    : DEFAULT_MAX_MBOX;
  const maxPst = sysConfig
    ? sysConfig.bichon_web_pst_upload_limit_mb * 1024 * 1024
    : DEFAULT_MAX_PST;

  // Import history
  const { data: history = [], refetch: refetchHistory } = useQuery({
    queryKey: ['import-history'],
    queryFn: list_import_history,
    staleTime: 10_000,
  });

  // Resolve the effective folder based on current mode
  const effectiveFolder = (() => {
    switch (folderMode) {
      case 'header':
        return headerFolder;
      case 'existing':
      case 'custom':
        return folder;
      default:
        return '';
    }
  })();

  const startPolling = useCallback((id: string) => {
    if (pollRef.current) clearInterval(pollRef.current);
    let retries = 0;
    pollRef.current = setInterval(async () => {
      try {
        const p = await get_import_progress(id);
        setProgress(p);
        retries = 0;
        if (p.status === 'Completed' || p.status === 'Failed') {
          if (pollRef.current) clearInterval(pollRef.current);
          setPhase('done');
          refetchHistory();
        }
      } catch {
        retries++;
        if (retries > 5) {
          if (pollRef.current) clearInterval(pollRef.current);
          setPhase('idle');
        }
      }
    }, 1000);
  }, [refetchHistory]);

  useEffect(() => {
    return () => { if (pollRef.current) clearInterval(pollRef.current); };
  }, []);

  const handleFiles = useCallback(async (newFiles: FileList | File[]) => {
    const arr = Array.from(newFiles) as File[];
    const queued: QueuedFile[] = arr.map((f) => {
      const ext = f.name.split('.').pop()?.toLowerCase() || '';
      const isMbox = ext === 'mbox';
      const isPst = ext === 'pst';
      const max = isMbox ? maxMbox : isPst ? maxPst : MAX_EML;
      const typeOk = isValidFileType(f, ext);
      return { file: f, sizeOk: f.size <= max, typeOk };
    });

    setFiles(queued);
    setPhase('idle');
    setProgress(null);
    //setImportId(null);

    // Extract folder hint from the first valid file.
    // PST files are binary (OLE2) — headers can't be extracted in-browser.
    const firstOk = queued.find((q) => q.sizeOk && q.typeOk);
    if (firstOk) {
      const ext = firstOk.file.name.split('.').pop()?.toLowerCase() || '';
      const isPstFile = ext === 'pst';
      setIsPstSelected(isPstFile);
      if (isPstFile) {
        // PST: folder structure is auto-detected, no manual mode needed
        setFolderHint(null);
        setHeaderFolder('INBOX');
        setFolderMode('');
      } else {
        // EML/MBOX: default to header auto-detect if no mode selected yet
        if (!folderMode) {
          setFolderMode('header');
        }
        try {
          const hint = await extractFolderHint(firstOk.file);
          if (hint) {
            setFolderHint(hint);
            setHeaderFolder(hint.name);
          }
        } catch {
          // ignore
        }
      }
    }
  }, []);

  const removeFile = (idx: number) => {
    setFiles((prev) => prev.filter((_, i) => i !== idx));
    if (files.length <= 1) {
      setFolderHint(null);
      setHeaderFolder('INBOX');
      setFolderMode('');
      setIsPstSelected(false);
    }
  };

  const handleAccountChange = (v: string) => {
    setAccountId(v);
    setFiles([]);
    setFolderHint(null);
    setHeaderFolder('INBOX');
  };

  const handleModeChange = (mode: FolderMode) => {
    setFolderMode(mode);
    // When switching to header mode, re-detect from files if available
    if (mode === 'header' && files.length > 0) {
      const firstOk = files.find((q) => q.sizeOk && q.typeOk);
      if (firstOk) {
        extractFolderHint(firstOk.file).then((hint) => {
          if (hint) {
            setFolderHint(hint);
            setHeaderFolder(hint.name);
          }
        });
      }
    }
  };

  const importMutation = useMutation({
    mutationFn: async () => {
      if (!accountId || !files.length) return;
      const file = files[0].file;
      setPhase('uploading');
      setUploadPct(0);
      const result = await upload_import(
        Number(accountId),
        effectiveFolder,
        file.name,
        file,
        (pct) => setUploadPct(pct),
      );
      //setImportId(result.import_id);
      setProgress(result);
      setPhase('processing');
      startPolling(result.import_id);
    },
    onError: (err: any) => {
      setPhase('idle');
      toast({
        title: t('common.failed'),
        description: err?.response?.data?.message || err.message,
        variant: 'destructive',
      });
    },
  });

  const canImport =
    accountId && effectiveFolder.trim() && files.length > 0 && files.every((f) => f.sizeOk && f.typeOk) && phase === 'idle';

  return (
    <>
      <FixedHeader />
      <Main>
        <div className="flex-1 space-y-6 p-6 md:p-8 max-w-3xl mx-auto">
          <div>
            <h1 className="text-xl font-bold tracking-tight">
              {t('import.title', 'Import EML / MBOX / PST')}
            </h1>
            <p className="text-sm text-muted-foreground mt-1">
              {t('import.description', 'Import email files into a NoSync account. For larger files, use the CLI.')}
            </p>
          </div>

          {/* Step 1: Target account */}
          <Card>
            <CardHeader className="pb-3">
              <CardTitle className="text-sm font-medium">
                {t('import.target', '1. Select target account')}
              </CardTitle>
            </CardHeader>
            <CardContent>
              <div className="max-w-sm space-y-1.5">
                <Label className="text-xs">{t('import.account')}</Label>
                <Popover open={accountOpen} onOpenChange={setAccountOpen}>
                  <PopoverTrigger asChild>
                    <Button
                      variant="outline"
                      role="combobox"
                      className="h-9 justify-between text-xs w-full"
                    >
                      <span className={cn('truncate', !accountId && 'text-muted-foreground')}>
                        {accountId
                          ? accounts.find((a) => String(a.id) === accountId)?.account_name
                          || accounts.find((a) => String(a.id) === accountId)?.email
                          || accountId
                          : t('import.selectAccount')}
                      </span>
                      <ChevronsUpDown className="ml-2 h-3.5 w-3.5 shrink-0 opacity-50" />
                    </Button>
                  </PopoverTrigger>
                  <PopoverContent className="w-[280px] p-0" align="start">
                    <Command>
                      <CommandInput
                        placeholder={t('import.searchAccount', 'Search accounts...')}
                        className="h-9 text-xs"
                      />
                      <CommandList>
                        <CommandEmpty>
                          {t('import.noAccountFound', 'No account found.')}
                        </CommandEmpty>
                        <CommandGroup>
                          {accounts.map((a) => (
                            <CommandItem
                              key={a.id}
                              value={a.account_name || a.email || String(a.id)}
                              onSelect={() => {
                                handleAccountChange(String(a.id));
                                setAccountOpen(false);
                              }}
                              className='text-xs'
                            >
                              <Check
                                className={cn(
                                  'h-4 w-4',
                                  accountId === String(a.id) ? 'opacity-100' : 'opacity-0',
                                )}
                              />
                              {a.account_name || a.email}
                            </CommandItem>
                          ))}
                        </CommandGroup>
                      </CommandList>
                    </Command>
                  </PopoverContent>
                </Popover>
              </div>
            </CardContent>
          </Card>

          {/* Step 2: Folder determination mode */}
          <Card>
            <CardHeader className="pb-3">
              <CardTitle className="text-sm font-medium">
                {isPstSelected
                  ? t('import.folderStructure', '2. Folder structure')
                  : t('import.folderMethod', '2. Choose folder method')}
              </CardTitle>
              <CardDescription className="text-xs">
                {isPstSelected
                  ? t('import.pstFolderDesc', 'The PST file contains its own folder structure (e.g. Inbox, Sent Items, etc.). Folders will be automatically created during import.')
                  : files.length === 0
                    ? t('import.selectFileFirst', 'Select a file first to determine available options.')
                    : t('import.folderMethodDesc', 'How should the target mail folder be determined?')}
              </CardDescription>
            </CardHeader>
            <CardContent className="space-y-4">
              {!isPstSelected && (
              <RadioGroup
                value={folderMode}
                onValueChange={(v) => handleModeChange(v as FolderMode)}
                className="gap-3"
              >
                {/* Mode 1: Auto-detect from headers */}
                <label
                  className={cn(
                    'flex items-start gap-3 rounded-lg border p-3 cursor-pointer transition-colors',
                    folderMode === 'header'
                      ? 'border-primary bg-primary/5'
                      : 'border-border hover:bg-muted/50',
                  )}
                >
                  <RadioGroupItem value="header" id="mode-header" className="mt-0.5" />
                  <div className="flex-1 min-w-0">
                    <div className="flex items-center gap-2">
                      <Sparkles className="h-4 w-4 text-primary" />
                      <span className="text-sm font-medium">
                        {t('import.modeHeader', 'Auto-detect from email headers')}
                      </span>
                    </div>
                    <p className="text-xs text-muted-foreground mt-1">
                      {t('import.modeHeaderDesc', 'Read X-Gmail-Labels / X-Bichon-Metadata from the uploaded file. Falls back to filename.')}
                    </p>
                    {folderMode === 'header' && (
                      <div className="mt-2 flex items-center gap-2">
                        <Badge variant="secondary" className="text-xs font-normal">
                          {folderHint
                            ? t('import.detectedFolder', 'Detected') + ': ' + headerFolder
                            : t('import.noFileYet', 'No file selected yet')}
                        </Badge>
                        {folderHint && (
                          <span className="text-[10px] text-muted-foreground">
                            ({t('import.source')}: {folderHintLabel(folderHint)})
                          </span>
                        )}
                      </div>
                    )}
                  </div>
                </label>

                {/* Mode 2: Pick from existing mailboxes */}
                <label
                  className={cn(
                    'flex items-start gap-3 rounded-lg border p-3 cursor-pointer transition-colors',
                    folderMode === 'existing'
                      ? 'border-primary bg-primary/5'
                      : 'border-border hover:bg-muted/50',
                    !accountId && 'opacity-50 pointer-events-none',
                  )}
                >
                  <RadioGroupItem value="existing" id="mode-existing" className="mt-0.5" disabled={!accountId} />
                  <div className="flex-1 min-w-0">
                    <div className="flex items-center gap-2">
                      <ListTree className="h-4 w-4 text-primary" />
                      <span className="text-sm font-medium">
                        {t('import.modeExisting', 'Choose from existing mailboxes')}
                      </span>
                    </div>
                    <p className="text-xs text-muted-foreground mt-1">
                      {t('import.modeExistingDesc', 'Select one of the mailboxes already present in this account.')}
                    </p>
                    {folderMode === 'existing' && (
                      <div className="mt-2">
                        {mailboxes.length === 0 ? (
                          <span className="text-xs text-muted-foreground">
                            {accountId
                              ? t('import.noMailboxes', 'No mailboxes found in this account.')
                              : t('import.selectAccountFirst', 'Select an account first.')}
                          </span>
                        ) : (
                          <Popover open={mailboxOpen} onOpenChange={setMailboxOpen}>
                            <PopoverTrigger asChild>
                              <Button
                                variant="outline"
                                role="combobox"
                                className="h-8 justify-between text-xs max-w-xs w-full"
                              >
                                <span className="truncate">
                                  {folder || t('import.selectMailbox', 'Select a mailbox...')}
                                </span>
                                <ChevronsUpDown className="ml-2 h-3.5 w-3.5 shrink-0 opacity-50" />
                              </Button>
                            </PopoverTrigger>
                            <PopoverContent className="w-[280px] p-0" align="start">
                              <Command>
                                <CommandInput
                                  placeholder={t('import.searchMailbox', 'Search mailboxes...')}
                                  className="h-9 text-xs"
                                />
                                <CommandList>
                                  <CommandEmpty>
                                    {t('import.noMailboxFound', 'No mailbox found.')}
                                  </CommandEmpty>
                                  <CommandGroup>
                                    {mailboxes.map((mb) => (
                                      <CommandItem
                                        key={mb.id}
                                        value={mb.name}
                                        onSelect={(value) => {
                                          setFolder(value);
                                          setMailboxOpen(false);
                                        }}
                                        className='text-xs'
                                      >
                                        <Check
                                          className={cn(
                                            'h-4 w-4',
                                            folder === mb.name ? 'opacity-100' : 'opacity-0',
                                          )}
                                        />
                                        {mb.name}
                                      </CommandItem>
                                    ))}
                                  </CommandGroup>
                                </CommandList>
                              </Command>
                            </PopoverContent>
                          </Popover>
                        )}
                      </div>
                    )}
                  </div>
                </label>

                {/* Mode 3: Manual input */}
                <label
                  className={cn(
                    'flex items-start gap-3 rounded-lg border p-3 cursor-pointer transition-colors',
                    folderMode === 'custom'
                      ? 'border-primary bg-primary/5'
                      : 'border-border hover:bg-muted/50',
                  )}
                >
                  <RadioGroupItem value="custom" id="mode-custom" className="mt-0.5" />
                  <div className="flex-1 min-w-0">
                    <div className="flex items-center gap-2">
                      <PenLine className="h-4 w-4 text-primary" />
                      <span className="text-sm font-medium">
                        {t('import.modeCustom', 'Enter a custom folder name')}
                      </span>
                    </div>
                    <p className="text-xs text-muted-foreground mt-1">
                      {t('import.modeCustomDesc', 'Manually type the target mail folder name.')}
                    </p>
                    {folderMode === 'custom' && (
                      <div className="mt-2">
                        <Input
                          className="h-8 text-xs max-w-xs"
                          value={folder}
                          onChange={(e) => setFolder(e.target.value)}
                          placeholder="INBOX"
                        />
                      </div>
                    )}
                  </div>
                </label>
              </RadioGroup>
              )}
            </CardContent>
          </Card>

          {/* Step 3: File upload */}
          <Card>
            <CardHeader className="pb-3">
              <CardTitle className="text-sm font-medium">
                {t('import.chooseFiles', '3. Choose files')}
              </CardTitle>
              <CardDescription className="text-xs">
                {t('import.limits', {
                  defaultValue: 'Max: EML 100 MB · MBOX {{maxMbox}} MB · PST {{maxPst}} MB. Larger files → CLI.',
                  maxMbox: (maxMbox / (1024 * 1024)).toFixed(0),
                  maxPst: (maxPst / (1024 * 1024)).toFixed(0)
                })}
              </CardDescription>
            </CardHeader>
            <CardContent>
              <div
                className={cn(
                  'border-2 border-dashed rounded-lg p-8 text-center cursor-pointer transition-colors',
                  dragging ? 'border-primary bg-primary/5' : 'border-muted-foreground/25 hover:border-muted-foreground/50',
                  phase !== 'idle' && 'pointer-events-none opacity-50',
                )}
                onDragOver={(e) => { e.preventDefault(); setDragging(true); }}
                onDragLeave={() => setDragging(false)}
                onDrop={(e) => { e.preventDefault(); setDragging(false); handleFiles(e.dataTransfer.files); }}
                onClick={() => {
                  const input = document.createElement('input');
                  input.type = 'file';
                  input.accept = '.eml,.mbox,.pst,message/rfc822,application/mbox,text/plain';
                  input.multiple = true;
                  input.onchange = () => input.files && handleFiles(input.files);
                  input.click();
                }}
              >
                <Upload className="mx-auto h-10 w-10 text-muted-foreground/60 mb-3" />
                <p className="text-sm font-medium">
                  {t('import.dropHere', 'Drop .eml / .mbox / .pst files here')}
                </p>
                <p className="text-xs text-muted-foreground mt-1">
                  {t('import.orClick', 'or click to browse')}
                </p>
              </div>

              {files.length > 0 && (
                <div className="mt-4 space-y-2">
                  {files.map((qf, i) => (
                    <div
                      key={i}
                      className={cn(
                        'flex items-center gap-3 px-3 py-2 rounded-md border text-sm',
                        qf.sizeOk && qf.typeOk
                          ? 'bg-muted/30 border-border'
                          : 'bg-destructive/5 border-destructive/30 text-destructive',
                      )}
                    >
                      <FileText className="h-4 w-4 shrink-0" />
                      <span className="flex-1 truncate">{qf.file.name}</span>
                      <span className={cn('text-xs shrink-0', qf.sizeOk && qf.typeOk ? 'text-muted-foreground' : 'font-medium')}>
                        {formatSize(qf.file.size)}
                      </span>
                      {!qf.typeOk && (
                        <span className="text-xs font-medium text-destructive shrink-0">Invalid type</span>
                      )}
                      {!qf.sizeOk && qf.typeOk && (
                        <span className="text-xs font-medium text-destructive shrink-0">Too large</span>
                      )}
                      {qf.sizeOk && qf.typeOk ? (
                        <CheckCircle2 className="h-4 w-4 text-green-600 shrink-0" />
                      ) : (
                        <AlertTriangle className="h-4 w-4 shrink-0" />
                      )}
                      {phase === 'idle' && (
                        <button
                          type="button"
                          onClick={(e) => { e.stopPropagation(); removeFile(i); }}
                          className="p-0.5 hover:bg-muted rounded"
                        >
                          <X className="h-3.5 w-3.5" />
                        </button>
                      )}
                    </div>
                  ))}
                </div>
              )}
            </CardContent>
          </Card>

          {/* Step 4: Progress & Results */}
          {(phase !== 'idle' || progress) && (
            <Card>
              <CardHeader className="pb-3">
                <CardTitle className="text-sm font-medium">
                  {phase === 'uploading' && t('import.uploading', 'Uploading…')}
                  {phase === 'processing' && t('import.processing', 'Processing…')}
                  {phase === 'done' && (progress?.status === 'Completed' ? t('import.completed', 'Import complete') : t('import.failed', 'Import failed'))}
                </CardTitle>
              </CardHeader>
              <CardContent className="space-y-4">
                {phase === 'uploading' && (
                  <div className="space-y-1.5">
                    <div className="flex justify-between text-xs text-muted-foreground">
                      <span>{t('import.uploadingFile')}</span>
                      <span>{uploadPct}%</span>
                    </div>
                    <Progress value={uploadPct} className="h-2" />
                  </div>
                )}

                {progress && progress.total > 0 && (
                  <div className="space-y-1.5">
                    <div className="flex justify-between text-xs text-muted-foreground">
                      <span>
                        {t('import.processed', { current: progress.success + progress.failed, total: progress.total })}
                      </span>
                      <span>
                        {progress.total > 0
                          ? Math.round(((progress.success + progress.failed) / progress.total) * 100)
                          : 0}%
                      </span>
                    </div>
                    <Progress
                      value={progress.total > 0 ? ((progress.success + progress.failed) / progress.total) * 100 : 0}
                      className="h-2"
                    />
                  </div>
                )}

                {progress && progress.total > 0 && (
                  <div className="flex gap-4 text-xs">
                    <span className="flex items-center gap-1">
                      <CheckCircle2 className="h-3.5 w-3.5 text-green-600" />
                      {t('import.successCount', { count: progress.success })}
                    </span>
                    <span className="flex items-center gap-1">
                      <AlertTriangle className="h-3.5 w-3.5 text-amber-600" />
                      {t('import.failedCount', { count: progress.failed })}
                    </span>
                  </div>
                )}

                {progress && progress.failed_details.length > 0 && (
                  <details className="text-xs">
                    <summary className="cursor-pointer text-muted-foreground hover:text-foreground">
                      {t('import.failedDetails', 'Failed items')} ({progress.failed_details.length})
                    </summary>
                    <ScrollArea className="h-32 mt-2">
                      <div className="space-y-1">
                        {progress.failed_details.map((d, i) => (
                          <div key={i} className="text-muted-foreground font-mono text-[11px]">
                            #{d.index}: {d.error_message}
                          </div>
                        ))}
                      </div>
                    </ScrollArea>
                  </details>
                )}
              </CardContent>
            </Card>
          )}

          {/* Import button */}
          <div className="flex justify-between items-center">
            <div className="text-xs text-muted-foreground">
              {isPstSelected
                ? t('import.pstFolders', 'PST folder structure will be preserved during import')
                : (<>{t('import.willImportTo', 'Will import to')}: <span className="font-medium text-foreground">{effectiveFolder}</span></>)}
            </div>
            <Button
              onClick={() => importMutation.mutate()}
              disabled={!canImport || importMutation.isPending}
              className="gap-2"
            >
              {importMutation.isPending ? (
                <Upload className="h-4 w-4 animate-pulse" />
              ) : (
                <Upload className="h-4 w-4" />
              )}
              {t('import.startImport', 'Import')}
            </Button>
          </div>

          {/* Import history */}
          {history.length > 0 && (
            <CollapsibleHistory
              history={history}
              t={t}
              accountLabel={(id: number) =>
                accounts.find((a) => a.id === id)?.account_name
                || accounts.find((a) => a.id === id)?.email
                || String(id)
              }
            />
          )}
        </div>
      </Main>
    </>
  );
}

// ─── Import history collapsible ──────────────────────────────────────────

function statusColor(status: string) {
  switch (status) {
    case 'completed': return 'text-green-600';
    case 'failed': return 'text-destructive';
    case 'processing': return 'text-amber-600';
    default: return 'text-muted-foreground';
  }
}

function statusLabel(status: string) {
  switch (status) {
    case 'completed': return 'Completed';
    case 'failed': return 'Failed';
    case 'processing': return 'Processing';
    case 'pending': return 'Pending';
    default: return status;
  }
}

function timeAgo(ts: number) {
  const seconds = Math.floor((Date.now() - ts) / 1000);
  if (seconds < 60) return `${seconds}s ago`;
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m ago`;
  if (seconds < 86400) return `${Math.floor(seconds / 3600)}h ago`;
  return new Date(ts).toLocaleDateString();
}

function CollapsibleHistory({
  history,
  t,
  accountLabel,
}: {
  history: ImportHistory[];
  t: (key: string) => string;
  accountLabel: (id: number) => string;
}) {
  const [open, setOpen] = useState(false);

  return (
    <div className="border rounded-lg">
      <button
        type="button"
        onClick={() => setOpen(!open)}
        className="w-full flex items-center gap-2 px-4 py-3 text-sm hover:bg-muted/50 transition-colors rounded-lg"
      >
        <Clock className="h-4 w-4 text-muted-foreground" />
        <span className="font-medium">
          {t('import.importHistory')}
        </span>
        <span className="text-xs text-muted-foreground">
          ({history.length})
        </span>
        <ChevronRight
          className={cn(
            'h-4 w-4 ml-auto text-muted-foreground transition-transform',
            open && 'rotate-90',
          )}
        />
      </button>
      {open && (
        <div className="border-t">
          <div className="divide-y">
            {history.map((h) => (
              <div key={h.id} className="px-4 py-3 text-xs space-y-1.5">
                <div className="flex items-center justify-between">
                  <div className="flex items-center gap-2">
                    <span className={cn('font-medium', statusColor(h.status))}>
                      {statusLabel(h.status)}
                    </span>
                    <span className="text-muted-foreground">
                      {accountLabel(h.account_id)} / {h.folder}
                    </span>
                  </div>
                  <span className="text-muted-foreground">{timeAgo(h.created_at)}</span>
                </div>
                <div className="flex items-center gap-3 text-muted-foreground">
                  <span>{h.format.toUpperCase()}</span>
                  <span className="text-green-600">{h.success} success</span>
                  {h.duplicates > 0 && <span>{h.duplicates} dup</span>}
                  {h.failed > 0 && <span className="text-destructive">{h.failed} failed</span>}
                  <span>{h.total} total</span>
                </div>
                {h.failed_details.length > 0 && (
                  <details className="text-[11px]">
                    <summary className="cursor-pointer text-muted-foreground hover:text-foreground">
                      {t('import.failedDetails')} ({h.failed_details.length})
                    </summary>
                    <div className="mt-1 space-y-0.5 max-h-24 overflow-y-auto">
                      {h.failed_details.map((d, i) => (
                        <div key={i} className="text-muted-foreground font-mono">
                          #{d.index}: {d.error_message}
                        </div>
                      ))}
                    </div>
                  </details>
                )}
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}
