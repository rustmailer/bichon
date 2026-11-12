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

/**
 * Outcome of a repair attempt. Mirrors the Rust `BlobRepairReport::status`.
 *
 * Note that `failed` arrives with **HTTP 200** — it is a verdict on the record,
 * not a transport error. Only a rejected request reaches the axios error path.
 */
export type BlobRepairStatus = 'complete' | 'repaired' | 'partial' | 'failed'

/**
 * Mirrors `bichon_core::envelope::extractor::BlobRepairReport`.
 *
 * The report is self-describing: it carries the record's identity so a caller
 * that holds no envelope — the integrity failures table has only a failure row —
 * can still label the outcome without a second request.
 */
export interface BlobRepairReport {
    envelope_id: string
    account_id: number
    mailbox_id: number
    /** Subject of the repaired record, for display. */
    subject: string
    /** RFC `Message-ID` of the repaired record, for display. */
    message_id: string
    status: BlobRepairStatus
    /** Whether the email blob was missing before the repair. */
    email_missing_before: boolean
    /** Whether the email blob is still missing after the repair. */
    email_missing_after: boolean
    /** Legacy mirror of `email_missing_before`; prefer the before/after pair. */
    email_blob_missing: boolean
    /**
     * `true` when the fetched content validated but the write was skipped
     * because a backup window was open. Reported as `partial` even though the
     * source data was recovered — retry once the backup finishes.
     */
    skipped_due_to_backup: boolean
    /**
     * Attachment hashes to *suspect* before the repair. Best-effort naming
     * only: `status` is the verdict, so this can be empty on a broken record.
     */
    missing_before: string[]
    /** Attachment hashes to suspect after the repair. Same caveat as `missing_before`. */
    missing_after: string[]
    error: string | null
}

/** One persisted repair attempt. Mirrors `bichon_core::envelope::repair::BlobRepairRecord`. */
export interface BlobRepairRecord {
    id: string
    /** UTC millis when the repair finished. */
    created_at: number
    operator_id: number
    operator: string
    account_id: number
    mailbox_id: number
    subject: string
    message_id: string
    report: BlobRepairReport
}

/**
 * Re-fetches the whole message from the IMAP server and re-attaches any missing
 * content — the body and every attachment, not just one of them.
 *
 * Can take a while: the server fetches from IMAP and may wait up to 30s on a
 * blob drain. The axios timeout is 60s, which covers it.
 *
 * Repairs are non-destructive — content is hash-verified against the archived
 * `content_hash` before being written, and only the record's own blob keys are
 * replaced — so this deliberately does not go through the approval flow that
 * deletion does.
 */
export const repairMessage = async (accountId: number, envelopeId: string) => {
    const response = await axiosInstance.post<BlobRepairReport>(
        `api/v1/repair-message/${accountId}/${envelopeId}`
    );
    return response.data;
};

/**
 * The most recent repair reports, newest first. Bounded server-side to the
 * ring's cap regardless of `limit`.
 *
 * Pass `accountId` to narrow to one account; the server additionally drops any
 * record the caller may not repair, so the result can be shorter than `limit`.
 */
export const listRepairReports = async (limit = 200, accountId?: number) => {
    const response = await axiosInstance.get<BlobRepairRecord[]>(
        'api/v1/repair-reports',
        {
            params: {
                limit,
                ...(accountId != null ? { account_id: accountId } : {}),
            },
        }
    );
    return response.data;
};
