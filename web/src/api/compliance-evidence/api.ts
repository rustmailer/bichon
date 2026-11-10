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

/** Download an evidence pack for an inclusive epoch-millisecond range. */
export async function download_evidence_pack(
  from_ms: number,
  to_ms: number
): Promise<void> {
  let response
  try {
    response = await axiosInstance.post(
      'api/v1/compliance/evidence-pack',
      { from_ms, to_ms },
      { responseType: 'blob' }
    )
  } catch (e) {
    const data = (e as { response?: { data?: unknown } }).response?.data
    if (data instanceof Blob) {
      const text = await data.text()
      try {
        const parsed = JSON.parse(text) as { message?: string }
        throw new Error(parsed.message || text)
      } catch (parseError) {
        if (parseError instanceof SyntaxError) throw new Error(text)
        throw parseError
      }
    }
    throw e
  }
  const disposition = response.headers['content-disposition'] as
    | string
    | undefined
  const match = disposition?.match(/filename="?([^";]+)"?/)
  const filename = match?.[1] ?? `evidence-pack-${from_ms}-${to_ms}.zip`

  const url = URL.createObjectURL(response.data as Blob)
  const link = document.createElement('a')
  link.href = url
  link.download = filename
  document.body.appendChild(link)
  link.click()
  link.remove()
  URL.revokeObjectURL(url)
}
