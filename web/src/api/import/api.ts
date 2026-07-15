//
// Copyright (c) 2025-2026 rustmailer.com (https://rustmailer.com)
//
// This file is part of the Bichon Email Archiving Project

import axiosInstance from '@/api/axiosInstance';
import { list_accounts } from '@/api/account/api';
import type { AccountModel } from '@/api/account/api';

export interface ImportProgress {
  import_id: string;
  status: 'Pending' | 'Processing' | 'Completed' | 'Failed';
  format: string;
  total: number;
  success: number;
  duplicates: number;
  failed: number;
  failed_details: { index: number; error_message: string }[];
}

export const upload_import = async (
  accountId: number,
  mailFolder: string,
  fileName: string,
  file: File,
  onProgress?: (pct: number) => void
): Promise<ImportProgress> => {
  const response = await axiosInstance.post<ImportProgress>(
    `api/v1/upload-import`,
    file,
    {
      params: { account_id: accountId, mail_folder: mailFolder, file_name: fileName },
      headers: { 'Content-Type': 'application/octet-stream' },
      onUploadProgress: (e) => {
        if (e.total && onProgress) onProgress(Math.round((e.loaded / e.total) * 100));
      },
    }
  );
  return response.data;
};

export const get_import_progress = async (importId: string): Promise<ImportProgress> => {
  const response = await axiosInstance.get<ImportProgress>(
    `api/v1/import-progress/${importId}`
  );
  return response.data;
};

export const check_disk_space = async (): Promise<number> => {
  const response = await axiosInstance.get<number>('api/v1/check-disk-space');
  return response.data;
};

export const get_import_accounts = async (): Promise<AccountModel[]> => {
  const data = await list_accounts();
  return (data.items || []).filter(
    (a) => a.enabled
  );
};

// ── Import history ────────────────────────────────────────────────

export interface ImportHistory {
  id: string;
  user_id: number;
  import_id: string;
  account_id: number;
  folder: string;
  format: string;
  status: 'pending' | 'processing' | 'completed' | 'failed';
  total: number;
  success: number;
  duplicates: number;
  failed: number;
  failed_details: { index: number; error_message: string }[];
  created_at: number;
}

export const list_import_history = async (): Promise<ImportHistory[]> => {
  const response = await axiosInstance.get<ImportHistory[]>('api/v1/import-history');
  return response.data;
};
