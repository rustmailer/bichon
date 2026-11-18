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

/** The server's message when it sent one, otherwise the client error text.
 *
 *  Axios' own `error.message` is only "Request failed with status code 429";
 *  the explanation the API returns lives in the response body. Callers that
 *  show an error to the operator should prefer the body. */
export function errorMessage(error: unknown): string {
  const data = (error as { response?: { data?: { message?: string } } })
    ?.response?.data?.message
  if (typeof data === 'string' && data) return data
  if (error instanceof Error) return error.message
  return String(error)
}

/** True when the server refused the request because another job of the same
 *  kind is still running (the batch-delete single-flight limit). */
export function isTooManyRequests(error: unknown): boolean {
  return (error as { response?: { status?: number } })?.response?.status === 429
}
