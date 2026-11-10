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

// Event bus extension point.
//
// Community edition: NoopEventBus — all events are discarded.
// Pro edition: AuditEventBus — events are persisted to audit database.
// Enterprise edition: adds SIEM webhook to the same trait impl.
//
// The open-source server emits events at key points (login, view, delete, search).
// It never reads from the event bus — events are fire-and-forget.

pub mod account_roles;
pub mod audit_detail;
pub mod event_bus;
pub mod journal;
pub mod license_gate;
pub mod retention_policy;
pub mod text_extractor;
pub mod timestamp;
pub mod user_cleanup;
