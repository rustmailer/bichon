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


import { EmailEnvelope, PaginatedResponse } from "@/api";
import axiosInstance from "@/api/axiosInstance";
import { Group } from "@/api/system/api";
import { saveAs } from 'file-saver';

export const get_thread_messages = async (accountId: number, thread_id: string, page: number, page_size: number) => {
    const params = new URLSearchParams({
        thread_id: String(thread_id),
        page: String(page),
        page_size: String(page_size),
    });

    const response = await axiosInstance.get<PaginatedResponse<EmailEnvelope>>(
        `api/v1/get-thread-messages/${accountId}?${params.toString()}`
    );
    return response.data;
}

export const download_attachment = async (accountId: number, id: string, content_hash: string, fileName: string) => {
    const response = await axiosInstance.get(`api/v1/download-attachment/${accountId}/${id}?content_hash=${content_hash}`, { responseType: 'blob' });
    const blob = new Blob([response.data]);
    saveAs(blob, fileName);
};

/** Fetch raw attachment content for in-browser preview (Content-Disposition: inline). */
export const preview_attachment = async (accountId: number, id: string, content_hash: string) => {
    const response = await axiosInstance.get(
        `api/v1/preview-attachment/${accountId}/${id}`,
        {
            params: { content_hash },
            responseType: 'blob',
        }
    );
    return response.data as Blob;
};

export const download_nested_attachment = async (accountId: number, id: string, content_hash: string, nested_content_hash: string, fileName: string) => {
    const response = await axiosInstance.get(`api/v1/download-nested-attachment/${accountId}/${id}?content_hash=${content_hash}&nested_content_hash=${nested_content_hash}`, { responseType: 'blob' });
    const blob = new Blob([response.data]);
    saveAs(blob, fileName);
};
export interface AttachmentInfo {
    /** MIME content type of the attachment (e.g., `image/png`, `application/pdf`). */
    file_type: string;
    /** Content-ID, used for inline attachments (referenced in HTML by `cid:` URLs). */
    content_id?: string;
    /**
     * Whether the part is rendered inside the body through a `cid:` reference
     * (true) or is a regular, downloadable file (false). A bare
     * `Content-Disposition: inline` does not make a part inline.
     */
    inline: boolean;
    /** Original filename of the attachment, if provided. */
    filename: string;
    /** Size of the attachment in bytes. */
    size: number;
    content_hash: string;
    is_message: boolean
}

export interface MessageContentResponse {
    text?: string;
    html?: string;
    attachments?: AttachmentInfo[];
    has_remote_content?: boolean;
}

export interface NestedMessageContentResponse {
    text?: string;
    html?: string;
    attachments?: AttachmentInfo[];
    envelope: EmailEnvelope;
    has_remote_content?: boolean;
}

export const getContent = (messageContent: MessageContentResponse): string | null => {
    if (messageContent.html) {
        return messageContent.html;
    } else if (messageContent.text) {
        return messageContent.text;
    }
    return null;
};

export const load_message = async (accountId: number, id: string, blockRemoteContent = false) => {
    const params = new URLSearchParams();
    if (blockRemoteContent) {
        params.set('block_remote_content', 'true');
    }
    const qs = params.toString();
    const url = `api/v1/message-content/${accountId}/${id}${qs ? '?' + qs : ''}`;
    const response = await axiosInstance.get<MessageContentResponse>(url);
    return response.data;
};

export const load_nested_message = async (accountId: number, id: string, content_hash: string, blockRemoteContent = false) => {
    const params = new URLSearchParams({ content_hash });
    if (blockRemoteContent) {
        params.set('block_remote_content', 'true');
    }
    const response = await axiosInstance.get<NestedMessageContentResponse>(
        `api/v1/nested-message-content/${accountId}/${id}?${params.toString()}`
    );
    return response.data;
};

export const delete_messages = async (payload: Record<number, string[]>) => {
    const response = await axiosInstance.post("api/v1/delete-messages", payload);
    return response.data;
};

export const download_message = async (accountId: number, id: string) => {
    const response = await axiosInstance.get(`api/v1/download-message/${accountId}/${id}`, { responseType: 'blob' });
    const blob = new Blob([response.data]);
    saveAs(blob, `${id}.eml`);
};

export const restore_message = async (accountId: number, envelopeIds: string[]) => {
    const response = await axiosInstance.post(`api/v1/restore-messages/${accountId}`, {
        envelope_ids: envelopeIds,
    });
    return response.data;
};



export interface AttachmentMetadata {
    /**
     * Statistics of attachment file extensions (key + count).
     * @example [{ key: "pdf", count: 10 }, { key: "png", count: 5 }]
     */
    extensions: Group[];

    /**
     * Statistics of attachment categories (key + count).
     * @example [{ key: "document", count: 8 }, { key: "image", count: 6 }]
     */
    categories: Group[];

    /**
     * Statistics of attachment MIME types (Content-Type) (key + count).
     * @example [{ key: "application/pdf", count: 10 }, { key: "image/jpeg", count: 5 }]
     */
    content_types: Group[];
}


export const get_attachment_meta = async () => {
    const response = await axiosInstance.get<AttachmentMetadata>("api/v1/attachment_metadata");
    return response.data;
};

export const get_envelope = async (accountId: number, id: string) => {
    const response = await axiosInstance.get<EmailEnvelope>(`api/v1/envelope/${accountId}/${id}`);
    return response.data;
};


