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


import axiosInstance from "@/api/axiosInstance";

export interface ExportAccount {
  id: number;
  email: string;
  name: string | null;
}

export interface ExportPreviewView {
  saved_search_id: string;
  saved_search_name: string;
  format: string;
  accounts: ExportAccount[];
  total_emails: number;
  total_size: number;
}

export interface ExportJobView {
  job_id: string;
  status: string;
  saved_search_id: string;
  saved_search_name: string;
  format: string;
  accounts: ExportAccount[];
  total_emails: number;
  total_size: number;
  processed: number;
  exported: number;
  failed: number;
  error: string | null;
  artifact_name: string | null;
  artifact_size: number;
  artifact_hash: string | null;
  created_at: number;
  finished_at: number | null;
  verify_status: string;
  verify_checked: number;
  verify_matched: number;
  verify_mismatched: number;
  verify_error: string | null;
  verify_started_at: number | null;
  verify_finished_at: number | null;
}

export interface ExportVerifyMismatch {
  envelope_id: string;
  message_id: string | null;
  subject: string | null;
  expected_hash: string | null;
  actual_hash: string | null;
  reason: string;
}

export interface ExportVerifyView {
  job_id: string;
  status: string;
  artifact_name: string | null;
  expected_artifact_hash: string | null;
  actual_artifact_hash: string | null;
  artifact_hash_match: boolean;
  checked: number;
  matched: number;
  mismatched: number;
  verified_at: number;
  mismatches: ExportVerifyMismatch[];
}

export interface ExportVerifyProgressView {
  job_id: string;
  status: string;
  checked: number;
  total: number;
  matched: number;
  mismatched: number;
  error: string | null;
  result: ExportVerifyView | null;
  started_at: number | null;
  finished_at: number | null;
}

export interface DownloadTicketView {
  url: string;
}

/** Wire values of `bichon_core::export::ExportFormat` (variant names, not
 *  the lowercase `as_str` labels used in audit/approval rows). */
export type ExportFormatValue = "Mbox" | "Attachments";

/** An ad-hoc export selection, built from the page's current filter. Which
 *  half is used is decided by `format`; the other must be left unset. */
export interface ExportFilter {
  email?: Record<string, unknown>;
  attachment?: Record<string, unknown>;
}

/** What to export: a saved search or an ad-hoc filter, plus the format.
 *  Exactly one of `saved_search_id` and `filter` is set. */
export interface ExportRequest {
  saved_search_id?: string;
  filter?: ExportFilter;
  /** Display name for an ad-hoc export in the job list / approval card. */
  label?: string;
  format?: ExportFormatValue;
}

export const previewExport = async (request: ExportRequest) => {
  const response = await axiosInstance.post<ExportPreviewView>(
    "api/v1/exports/preview",
    request
  );
  return response.data;
};

/** A queued acknowledgement, returned instead of a job when dual control is on
 *  for `export.batch`. Nothing has been exported — `pending` is the
 *  discriminator, because the ungated path never sets it. */
export interface QueuedExport {
  ok: false;
  pending: true;
  request_id: string;
  expires_at?: number | null;
}

/** What starting an export returns: a job, or the queued acknowledgement. */
export type ExportStartResult = ExportJobView | QueuedExport;

export const is_queued_export = (
  result: ExportStartResult
): result is QueuedExport => (result as QueuedExport).pending === true;

export const createExport = async (
  request: ExportRequest
): Promise<ExportStartResult> => {
  const response = await axiosInstance.post<ExportStartResult>(
    "api/v1/exports",
    request
  );
  return response.data;
};

export const getExportJob = async (jobId: string) => {
  const response = await axiosInstance.get<ExportJobView>(`api/v1/exports/${jobId}`);
  return response.data;
};

export const downloadExport = async (jobId: string) => {
  const response = await axiosInstance.get(`api/v1/exports/${jobId}/download`, {
    responseType: "blob",
  });
  return response.data as Blob;
};

export const listExports = async () => {
  const response = await axiosInstance.get<ExportJobView[]>("api/v1/exports");
  return response.data;
};

export const cancelExport = async (jobId: string) => {
  const response = await axiosInstance.post<ExportJobView>(
    `api/v1/exports/${jobId}/cancel`
  );
  return response.data;
};

export const startExportVerify = async (jobId: string) => {
  const response = await axiosInstance.post<ExportVerifyProgressView>(
    `api/v1/exports/${jobId}/verify`
  );
  return response.data;
};

export const getExportVerifyProgress = async (jobId: string) => {
  const response = await axiosInstance.get<ExportVerifyProgressView>(
    `api/v1/exports/${jobId}/verify`
  );
  return response.data;
};

export const createDownloadTicket = async (jobId: string) => {
  const response = await axiosInstance.post<DownloadTicketView>(
    `api/v1/exports/${jobId}/download-ticket`
  );
  return response.data;
};

export const deleteExport = async (jobId: string) => {
  await axiosInstance.delete(`api/v1/exports/${jobId}`);
};
