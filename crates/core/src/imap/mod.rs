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


pub mod capabilities;
pub mod client;
pub mod executor;
pub mod manager;
pub mod oauth2;
pub mod session;
pub mod stats;
#[cfg(test)]
mod tests;
pub(crate) mod uidonly;
pub(crate) mod uidonly_acquisition;
#[cfg(test)]
mod uidonly_tests;
#[cfg(test)]
pub mod mock_server;
