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

use std::sync::{Arc, LazyLock};

use crate::modules::indexer::fields::*;
use tantivy::schema::INDEXED;
use tantivy::schema::{Schema, FAST, STORED};

static EML_FIELDS: LazyLock<Arc<EmlFields>> = LazyLock::new(|| {
    let (_, fields) = SchemaTools::create_eml_schema();
    Arc::new(fields)
});

pub struct SchemaTools;

impl SchemaTools {
    pub fn eml_schema() -> Schema {
        let (schema, _) = Self::create_eml_schema();
        schema
    }

    pub fn eml_fields() -> &'static EmlFields {
        &EML_FIELDS
    }

    pub fn create_eml_schema() -> (Schema, EmlFields) {
        let mut builder = Schema::builder();
        let f_id = builder.add_u64_field(F_ID, INDEXED | FAST);
        let f_account_id = builder.add_u64_field(F_ACCOUNT_ID, INDEXED | STORED | FAST);
        let f_mailbox_id = builder.add_u64_field(F_MAILBOX_ID, INDEXED | STORED | FAST);
        let f_eml = builder.add_bytes_field(F_EML, STORED);
        let fields = EmlFields {
            f_id,
            f_account_id,
            f_mailbox_id,
            f_eml,
        };
        (builder.build(), fields)
    }
}
