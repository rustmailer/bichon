use arrow::array::{BooleanArray, Int32Array, Int64Array, ListBuilder, StringBuilder, UInt64Array};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use std::sync::Arc;

use crate::modules::indexer::envelope::Envelope;

pub const DEFAULT_SHARD_ID: u64 = 0;

pub fn build_record_batch(items: &[Envelope]) -> RecordBatch {
    let capacity = items.len();

    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::UInt64, false),
        Field::new("account_id", DataType::UInt64, false),
        Field::new("mailbox_id", DataType::UInt64, false),
        Field::new("uid", DataType::UInt64, false),
        Field::new("subject", DataType::Utf8, true),
        Field::new("body", DataType::Utf8, true),
        Field::new("sender", DataType::Utf8, true),
        Field::new(
            "recipients",
            DataType::List(Arc::new(Field::new("item", DataType::Utf8, true))),
            true,
        ),
        Field::new(
            "cc",
            DataType::List(Arc::new(Field::new("item", DataType::Utf8, true))),
            true,
        ),
        Field::new(
            "bcc",
            DataType::List(Arc::new(Field::new("item", DataType::Utf8, true))),
            true,
        ),
        Field::new("sent_at", DataType::Int64, true),
        Field::new("received_at", DataType::Int64, true),
        Field::new("size_bytes", DataType::UInt64, true),
        Field::new("thread_id", DataType::UInt64, true),
        Field::new("message_id", DataType::Utf8, true),
        Field::new("has_attachment", DataType::Boolean, false),
        Field::new("attachment_count", DataType::Int32, false),
        Field::new(
            "tags",
            DataType::List(Arc::new(Field::new("item", DataType::Utf8, true))),
            true,
        ),
        Field::new("shard_id", DataType::UInt64, false),
    ]));

    let mut id_b = UInt64Array::builder(capacity);
    let mut account_id_b = UInt64Array::builder(capacity);
    let mut mailbox_id_b = UInt64Array::builder(capacity);
    let mut uid_b = UInt64Array::builder(capacity);
    let mut subject_b = StringBuilder::with_capacity(capacity, capacity * 20);
    let mut body_b = StringBuilder::with_capacity(capacity, capacity * 100);
    let mut from_b = StringBuilder::with_capacity(capacity, capacity * 20);

    let mut to_b = ListBuilder::new(StringBuilder::new());
    let mut cc_b = ListBuilder::new(StringBuilder::new());
    let mut bcc_b = ListBuilder::new(StringBuilder::new());

    let mut date_b = Int64Array::builder(capacity);
    let mut internal_date_b = Int64Array::builder(capacity);
    let mut size_b = UInt64Array::builder(capacity);
    let mut thread_id_b = UInt64Array::builder(capacity);
    let mut msg_id_b = StringBuilder::with_capacity(capacity, capacity * 30);
    let mut has_att_b = BooleanArray::builder(capacity);
    let mut att_count_b = Int32Array::builder(capacity);
    let mut tags_b = ListBuilder::new(StringBuilder::new());
    let mut shard_id_b = UInt64Array::builder(capacity);

    for e in items {
        id_b.append_value(e.id);
        account_id_b.append_value(e.account_id);
        mailbox_id_b.append_value(e.mailbox_id);
        uid_b.append_value(e.uid as u64);
        subject_b.append_value(&e.subject);
        body_b.append_value(&e.text);
        from_b.append_value(&e.from);

        for addr in &e.to {
            to_b.values().append_value(addr);
        }
        to_b.append(true);

        for addr in &e.cc {
            cc_b.values().append_value(addr);
        }
        cc_b.append(true);

        for addr in &e.bcc {
            bcc_b.values().append_value(addr);
        }
        bcc_b.append(true);

        date_b.append_value(e.date);
        internal_date_b.append_value(e.internal_date);
        size_b.append_value(e.size as u64);
        thread_id_b.append_value(e.thread_id);
        msg_id_b.append_value(&e.message_id);

        has_att_b.append_value(e.attachment_count > 0);
        att_count_b.append_value(e.attachment_count as i32);
        tags_b.append(true);
        shard_id_b.append_value(DEFAULT_SHARD_ID);
    }

    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(id_b.finish()),
            Arc::new(account_id_b.finish()),
            Arc::new(mailbox_id_b.finish()),
            Arc::new(uid_b.finish()),
            Arc::new(subject_b.finish()),
            Arc::new(body_b.finish()),
            Arc::new(from_b.finish()),
            Arc::new(to_b.finish()),
            Arc::new(cc_b.finish()),
            Arc::new(bcc_b.finish()),
            Arc::new(date_b.finish()),
            Arc::new(internal_date_b.finish()),
            Arc::new(size_b.finish()),
            Arc::new(thread_id_b.finish()),
            Arc::new(msg_id_b.finish()),
            Arc::new(has_att_b.finish()),
            Arc::new(att_count_b.finish()),
            Arc::new(tags_b.finish()),
            Arc::new(shard_id_b.finish()),
        ],
    )
    .expect("Failed to build RecordBatch")
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use duckdb::{Connection, Result};

    #[test]
    fn test_envelope_ingestion_and_query() -> Result<()> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS envelopes (
                -- internal id (tantivy f_id)
                id                UBIGINT NOT NULL,

                -- account / mailbox / uid
                account_id        UBIGINT NOT NULL,
                mailbox_id        UBIGINT NOT NULL,
                uid               UBIGINT NOT NULL,

                -- headers / content
                subject           TEXT,
                body              TEXT,

                sender         TEXT,
                recipients           VARCHAR[],
                cc           VARCHAR[],
                bcc          VARCHAR[],

                -- dates
                sent_at           BIGINT,
                received_at  BIGINT,

                -- size
                size_bytes              UBIGINT,

                -- thread
                thread_id         UBIGINT,

                -- message-id
                message_id        TEXT,

                -- attachment summary
                has_attachment    BOOLEAN NOT NULL,
                attachment_count  INTEGER NOT NULL CHECK (attachment_count >= 0),
                tags              VARCHAR[],
                shard_id      UBIGINT NOT NULL
            );
            "#,
        )?;

        let items = vec![Envelope {
            id: 101,
            account_id: 1,
            mailbox_id: 1,
            uid: 50,
            subject: "Testing Arrow".to_string(),
            text: "Content".to_string(),
            from: "sender@test.com".to_string(),
            to: vec!["user1@test.com".to_string(), "user2@test.com".to_string()],
            cc: vec!["manager@test.com".to_string()],
            bcc: vec![],
            date: 1000,
            internal_date: 1001,
            size: 2048,
            thread_id: 1,
            message_id: "id123".to_string(),
            attachment_count: 1,
            tags: None,
            account_email: None,
            mailbox_name: None,
        }];

        {
            let batch = build_record_batch(&items);
            let mut appender = conn.appender("envelopes")?;
            appender.append_record_batch(batch)?;
            appender.flush()?;
        }

        let mut stmt = conn.prepare("SELECT subject, size_bytes FROM envelopes WHERE id = 101")?;
        let mut rows = stmt.query([])?;
        if let Some(row) = rows.next()? {
            let subject: String = row.get(0)?;
            let size: u64 = row.get(1)?;
            assert_eq!(subject, "Testing Arrow");
            assert_eq!(size, 2048);
        }

        let mut stmt = conn.prepare(
            "SELECT count(*) FROM envelopes WHERE list_contains(\"recipients\", 'user2@test.com')",
        )?;
        let count: i64 = stmt.query_row([], |r| r.get(0))?;
        assert_eq!(count, 1);

        let has_att: bool = conn.query_row(
            "SELECT has_attachment FROM envelopes WHERE id = 101",
            [],
            |r| r.get(0),
        )?;
        assert!(has_att);

        println!("Integration test for ingestion and query passed!");
        Ok(())
    }
}
