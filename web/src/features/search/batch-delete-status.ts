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

import type { BatchDeleteJob, BatchDeleteStatus } from '@/api/batch-delete/api'

export const ACTIVE_STATUSES: BatchDeleteStatus[] = ['running', 'cancelling']

export function isActive(job: BatchDeleteJob) {
  return ACTIVE_STATUSES.includes(job.status)
}

export function statusVariant(
  status: BatchDeleteStatus
): 'default' | 'secondary' | 'destructive' | 'outline' {
  switch (status) {
    case 'running':
    case 'cancelling':
      return 'default'
    case 'success':
      return 'secondary'
    case 'failed':
      return 'destructive'
    default:
      return 'outline'
  }
}

export function jobLabel(job: BatchDeleteJob): string {
  if (job.label) return job.label
  const text = job.filter?.text
  return typeof text === 'string' && text.length > 0 ? text : job.job_id
}
