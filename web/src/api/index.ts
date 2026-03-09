//
// Copyright (c) 2025 rustmailer.com (https://rustmailer.com)
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


export interface PaginatedResponse<S> {
  current_page: number | null;
  page_size: number | null;
  total_items: number;
  items: S[];
  total_pages: number | null;
}


export interface EmailEnvelope {
  id: number;
  message_id: string;
  account_id: number;
  account_email?: string;
  mailbox_name?: string;
  uid: number;
  subject: string;
  text: string;
  from: string;
  to: string[];
  cc: string[];
  bcc: string[];
  date: number;
  internal_date: number;
  size: number;
  thread_id: number,
  attachment_count: number;
  tags: string[];
}