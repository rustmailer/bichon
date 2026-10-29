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
import { EmailEnvelope, PaginatedResponse } from "..";

export const search_messages = async (payload: Record<string, any>) => {
    const response = await axiosInstance.post<PaginatedResponse<EmailEnvelope>>("api/v1/search-messages", payload);
    return response.data;
};

export interface TagCount {
    tag: string;
    count: number;
}

export const get_tags = async () => {
    const response = await axiosInstance.get<TagCount[]>("api/v1/all-tags");
    return response.data;
}

export const update_tags = async (data: Record<string, any>) => {
    const response = await axiosInstance.post("api/v1/update-tags", data);
    return response.data;
};


/** One entry of the contact suggestion list. `name` is only known for
 *  addresses that arrived as a sender with a decoded display name. */
export interface Contact {
    email: string;
    name?: string;
}

export const get_contacts = async () => {
    const response = await axiosInstance.get<Contact[]>("api/v1/all-contacts");
    return response.data;
}


