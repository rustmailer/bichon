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
import { PaginatedResponse } from "..";
import { TagCount } from "../search/api";


export interface AttachmentModel {
    id: string;
    envelope_id: string;
    account_id: number;
    account_email: string,
    mailbox_id: number;
    mailbox_name: string;
    subject: string;
    content_hash: string;
    from: string;
    from_name?: string;
    wire_from?: string;
    date: number;
    ingest_at: number;
    size: number;

    ext?: string;
    category: string;
    content_type: string;
    shard_id: number;
    text?: string;
    has_text: boolean;
    is_ocr: boolean;
    page_count?: number;

    is_indexed: boolean;
    is_message: boolean;
    name?: string;
    tags?: string[];
    auto_tags?: string[];
}

export const search_attachment = async (payload: Record<string, any>) => {
    const response = await axiosInstance.post<PaginatedResponse<AttachmentModel>>("api/v1/search-attachment", payload);
    return response.data;
};

export const get_all_attachment_tags = async () => {
    const response = await axiosInstance.get<TagCount[]>("api/v1/all-attachment-tags");
    return response.data;
}

export const update_attachment_tags = async (data: Record<string, any>) => {
    const response = await axiosInstance.post("api/v1//update-attachment-tags", data);
    return response.data;
};


export const get_attachment_senders = async () => {
    const response = await axiosInstance.get<string[]>("api/v1/attachment-senders");
    return response.data;
}


