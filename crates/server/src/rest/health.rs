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
//

//! Liveness endpoint, deliberately separate from readiness.
//!
//! `GET /healthz` answers `200 ok` while the process can serve HTTP — in
//! normal operation **and** in maintenance mode. `/api/status` is readiness:
//! it returns `503` in maintenance mode on purpose.
//!
//! The distinction matters to orchestrators that restart a container when a
//! liveness probe fails. In maintenance mode the process must stay alive so an
//! operator can `docker exec` / `kubectl exec` into it and finish the migration;
//! pointing liveness at `/api/status` would restart the container exactly when
//! that access is needed. Kubernetes users should use `/healthz` for liveness
//! and `/api/status` for readiness.
//!
//! This endpoint performs no dependency checks: it says "the HTTP loop is
//! alive", not "the archive is healthy". It is mounted outside
//! `bichon_base_url` so a sub-path deployment keeps a stable probe path.

use poem::{handler, http::StatusCode, Response};

/// Plain liveness response: `200 ok`, never cached.
#[handler]
pub async fn healthz() -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header("Cache-Control", "no-store")
        .content_type("text/plain; charset=utf-8")
        .body("ok")
        .into()
}
