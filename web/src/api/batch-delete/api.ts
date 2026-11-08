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

import axiosInstance from '@/api/axiosInstance'

export type BatchDeleteStatus =
  | 'running'
  | 'cancelling'
  | 'cancelled'
  | 'success'
  | 'failed'
  | 'interrupted'

export interface BatchDeleteJob {
  job_id: string
  user_id: number
  username: string
  created_at: number
  started_at?: number | null
  finished_at?: number | null
  status: BatchDeleteStatus
  filter: Record<string, unknown>
  label?: string | null
  account_ids: number[]
  total: number
  processed: number
  deleted: number
  failed: number
  error?: string | null
  cancel_requested: boolean
}

export interface BatchDeletePreview {
  /** Messages that will be deleted (delete permission held). */
  total: number
  /** Every message matching the filter in the caller's readable scope. */
  matched: number
  /** Matching messages on accounts the caller cannot delete. */
  skipped_no_permission: number
  /** Source accounts the job will touch. */
  account_ids: number[]
  /** Source accounts whose messages are skipped. */
  skipped_account_ids: number[]
  held_account_ids: number[]
}

export interface BatchDeleteRequest {
  filter: Record<string, unknown>
  label?: string
}

export const preview_batch_delete = async (payload: BatchDeleteRequest) => {
  const response = await axiosInstance.post<BatchDeletePreview>(
    'api/v1/batch-delete/preview',
    payload
  )
  return response.data
}

export const create_batch_delete = async (payload: BatchDeleteRequest) => {
  const response = await axiosInstance.post<BatchDeleteJob | { pending?: boolean }>(
    'api/v1/batch-delete',
    payload
  )
  return response.data
}

export const list_batch_deletes = async () => {
  const response = await axiosInstance.get<BatchDeleteJob[]>('api/v1/batch-delete')
  return response.data
}

export const get_batch_delete = async (jobId: string) => {
  const response = await axiosInstance.get<BatchDeleteJob>(
    `api/v1/batch-delete/${jobId}`
  )
  return response.data
}

export const cancel_batch_delete = async (jobId: string) => {
  const response = await axiosInstance.post<BatchDeleteJob>(
    `api/v1/batch-delete/${jobId}/cancel`
  )
  return response.data
}
