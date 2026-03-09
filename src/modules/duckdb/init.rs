use chrono::{NaiveDateTime, Utc};
use duckdb::{params, types::Value, DuckdbConnectionManager};
use refinery::Runner;
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::OnceLock,
};

use crate::{
    modules::{
        account::migration::AccountModel,
        context::Initialize,
        dashboard::{DashboardStats, Group, LargestEmail, TimeBucket},
        duckdb::{build::build_record_batch, refinery::DuckDBConnection},
        error::{code::ErrorCode, BichonResult},
        indexer::{
            envelope::Envelope,
            manager::{EML_INDEX_MANAGER, ENVELOPE_INDEX_MANAGER},
        },
        message::{
            content::AttachmentInfo,
            search::{SearchFilter, SortBy},
            tags::TagCount,
        },
        rest::response::DataPage,
        settings::{cli::SETTINGS, dir::DATA_DIR_MANAGER},
    },
    raise_error,
};

pub type DuckDBConn = r2d2::PooledConnection<DuckdbConnectionManager>;

pub static DUCKDBMANAGER: OnceLock<DuckDBManager> = OnceLock::new();

pub mod duckdb_tables {
    refinery::embed_migrations!("src/modules/duckdb/migrations");
}

pub fn duckdb() -> BichonResult<&'static DuckDBManager> {
    DUCKDBMANAGER.get().ok_or_else(|| {
        raise_error!(
            "DuckDB manager is not initialized".into(),
            ErrorCode::InternalError
        )
    })
}

fn debug_sql(sql: &str, args: &[Value]) -> String {
    let mut result = String::new();
    let mut parts = sql.split('?');

    for (i, part) in parts.by_ref().enumerate() {
        result.push_str(part);

        if i < args.len() {
            result.push_str(&format!("{:?}", args[i]));
        }
    }

    result
}

pub struct DuckDBManager {
    pool: r2d2::Pool<DuckdbConnectionManager>,
}

impl Initialize for DuckDBManager {
    async fn initialize() -> BichonResult<()> {
        tracing::debug!("Initializing databases");

        if !&DATA_DIR_MANAGER.envelope_dir.exists() {
            std::fs::create_dir_all(&DATA_DIR_MANAGER.envelope_dir)
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        }

        let pool = init(
            &DATA_DIR_MANAGER.envelope_dir.join("envelopes.db"),
            duckdb_tables::migrations::runner(),
        )?;
        let _ = DUCKDBMANAGER.set(DuckDBManager { pool });
        Ok(())
    }
}

impl DuckDBManager {
    pub fn conn(&self) -> BichonResult<DuckDBConn> {
        Ok(self
            .pool
            .get()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?)
    }

    pub fn shutdown(&self) -> BichonResult<()> {
        self.conn()?
            .execute("FORCE CHECKPOINT", [])
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        tracing::info!("Shutting down");
        Ok(())
    }

    pub fn validate_regex(&self, pattern: &str) -> BichonResult<()> {
        let conn = self.conn()?;
        let check_sql = "SELECT regexp_matches('', ?)";
        if let Err(e) = conn
            .prepare(check_sql)
            .and_then(|mut stmt| stmt.execute([pattern]))
        {
            return Err(raise_error!(
                format!("Invalid Regular Expression for DuckDB: {}", e).into(),
                ErrorCode::InvalidParameter
            ));
        }
        Ok(())
    }

    pub fn delete_envelopes_by_account(&self, account_id: u64) -> BichonResult<usize> {
        let mut conn = self.conn()?;
        let tx = conn
            .transaction()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

        tx.execute(
            "DELETE FROM envelope_attachments WHERE account_id = ?;",
            params![account_id],
        )
        .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

        let rows_deleted = tx
            .execute(
                "DELETE FROM envelopes WHERE account_id = ?;",
                params![account_id],
            )
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

        tx.commit()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

        if rows_deleted > 0 {
            tracing::info!(
                "Account cleanup: removed {} emails and their attachments for account {}",
                rows_deleted,
                account_id
            );
        }

        Ok(rows_deleted)
    }

    pub fn delete_mailbox_envelopes(
        &self,
        account_id: u64,
        mailbox_ids: Vec<u64>,
    ) -> BichonResult<()> {
        if mailbox_ids.is_empty() {
            return Ok(());
        }
        let mut conn = self.conn()?;
        let tx = conn
            .transaction()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        {
            let placeholders = mailbox_ids
                .iter()
                .map(|_| "?")
                .collect::<Vec<_>>()
                .join(", ");

            let mut sql_params: Vec<duckdb::types::Value> = vec![account_id.into()];
            sql_params.extend(mailbox_ids.iter().map(|&id| duckdb::types::Value::from(id)));
            let param_iter = duckdb::params_from_iter(sql_params);

            let delete_att_sql = format!(
                "DELETE FROM envelope_attachments WHERE account_id = ? AND mailbox_id IN ({})",
                placeholders
            );
            tx.execute(&delete_att_sql, param_iter.clone())
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

            let delete_env_sql = format!(
                "DELETE FROM envelopes WHERE account_id = ? AND mailbox_id IN ({})",
                placeholders
            );
            let count = tx
                .execute(&delete_env_sql, param_iter)
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

            if count > 0 {
                tracing::info!(
                    "Deleted {} emails and their associated attachments for account: {}, mailboxes: {:?}",
                    count,
                    account_id,
                    mailbox_ids
                );
            }
        }

        tx.commit()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        Ok(())
    }

    pub fn append_envelopes_with_attachments(
        &self,
        items: &[(Envelope, Vec<AttachmentInfo>)],
    ) -> BichonResult<()> {
        let mut conn = self.conn()?;
        let tx = conn
            .transaction()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        {
            let mut env_appender = tx
                .appender("envelopes")
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
            let mut att_appender = tx
                .appender("envelope_attachments")
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
            for (env, atts) in items {
                env_appender
                    .append_record_batch(build_record_batch(std::slice::from_ref(env)))
                    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
                for att in atts {
                    att_appender
                        .append_row(params![
                            env.id,
                            env.account_id,
                            env.mailbox_id,
                            att.filename,
                            att.get_extension(),
                            att.get_category(),
                            att.file_type,
                            att.size as u64,
                            0,
                        ])
                        .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
                }
            }
            env_appender
                .flush()
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
            att_appender
                .flush()
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        }
        tx.commit()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        Ok(())
    }

    pub fn total_emails(
        &self,
        accounts: Option<std::collections::HashSet<u64>>,
    ) -> BichonResult<u64> {
        let conn = self.conn()?;
        let total: u64 = match accounts {
            None => conn
                .query_row("SELECT COUNT(*) FROM envelopes", [], |row| row.get(0))
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?,
            Some(accts) if !accts.is_empty() => {
                let placeholders = accts.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
                let sql = format!(
                    "SELECT COUNT(*) FROM envelopes WHERE account_id IN ({})",
                    placeholders
                );

                conn.query_row(&sql, duckdb::params_from_iter(accts), |row| row.get(0))
                    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
            }
            _ => 0,
        };

        Ok(total)
    }

    pub fn num_messages_in_thread(&self, account_id: u64, thread_id: u64) -> BichonResult<u64> {
        let conn = self.conn()?;
        let count: u64 = conn
            .query_row(
                "SELECT COUNT(*) FROM envelopes WHERE account_id = ? AND thread_id = ?;",
                params![account_id, thread_id],
                |row| row.get(0),
            )
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        Ok(count)
    }

    pub fn num_messages_in_mailbox(&self, account_id: u64, mailbox_id: u64) -> BichonResult<u64> {
        let conn = self.conn()?;
        let count: u64 = conn
            .query_row(
                "SELECT COUNT(*) FROM envelopes WHERE account_id = ? AND mailbox_id = ?;",
                params![account_id, mailbox_id],
                |row| row.get(0),
            )
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        Ok(count)
    }

    pub fn get_all_uids(&self, account_id: u64, mailbox_id: u64) -> BichonResult<HashSet<u32>> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare("SELECT uid FROM envelopes WHERE account_id = ? AND mailbox_id = ?;")
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        let uids: HashSet<u32> = stmt
            .query_map(params![account_id, mailbox_id], |row| {
                let uid: u64 = row.get(0)?;
                Ok(uid as u32)
            })
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
            .filter_map(|r| r.ok())
            .collect();
        Ok(uids)
    }

    pub fn get_max_uid(&self, account_id: u64, mailbox_id: u64) -> BichonResult<Option<u64>> {
        let conn = self.conn()?;
        let max_uid: Option<u64> = conn
            .query_row(
                "SELECT MAX(uid) FROM envelopes WHERE account_id = ? AND mailbox_id = ?;",
                params![account_id, mailbox_id],
                |row| row.get(0),
            )
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

        Ok(max_uid)
    }

    pub fn get_envelope_by_id(
        &self,
        account_id: u64,
        envelope_id: u64,
    ) -> BichonResult<Option<Envelope>> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare("SELECT * FROM envelopes WHERE account_id = ? AND id = ? LIMIT 1;")
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

        let envelope_res = stmt.query_row(params![account_id, envelope_id], |row| {
            Envelope::from_row(row)
        });

        match envelope_res {
            Ok(env) => Ok(Some(env)),
            Err(duckdb::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(raise_error!(format!("{:#?}", e), ErrorCode::InternalError)),
        }
    }

    pub fn get_envelopes_by_ids(
        &self,
        account_id: u64,
        envelope_ids: &[u64],
    ) -> BichonResult<Vec<Envelope>> {
        if envelope_ids.is_empty() {
            return Ok(vec![]);
        }
        let conn = self.conn()?;
        let ids_str = envelope_ids
            .iter()
            .map(|id| id.to_string())
            .collect::<Vec<_>>()
            .join(",");

        let query = format!(
            "SELECT * FROM envelopes WHERE account_id = ? AND id IN ({})",
            ids_str
        );

        let mut stmt = conn
            .prepare(&query)
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

        let rows = stmt
            .query_map(params![account_id], |row| Envelope::from_row(row))
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

        let mut result = Vec::new();
        for row in rows {
            result.push(
                row.map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?,
            );
        }

        Ok(result)
    }

    pub fn get_all_tags(&self, accounts: Option<HashSet<u64>>) -> BichonResult<Vec<TagCount>> {
        let conn = self.conn()?;
        let mut sql = "
            SELECT t.tag, COUNT(*) as count 
            FROM (
                SELECT unnest(tags) as tag, account_id 
                FROM envelopes
                WHERE len(tags) > 0
            ) t 
        "
        .to_string();

        let mut params_vec: Vec<duckdb::types::Value> = Vec::new();
        if let Some(ref acc_set) = accounts {
            if !acc_set.is_empty() {
                let placeholders = acc_set.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
                sql.push_str(&format!(" WHERE t.account_id IN ({})", placeholders));
                for id in acc_set {
                    params_vec.push((*id).into());
                }
            }
        }

        sql.push_str(" GROUP BY t.tag ORDER BY count DESC, t.tag ASC;");

        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

        let tag_counts = stmt
            .query_map(duckdb::params_from_iter(params_vec), |row| {
                Ok(TagCount {
                    tag: row.get(0)?,
                    count: row.get(1)?,
                })
            })
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

        Ok(tag_counts)
    }

    pub fn get_all_contacts(
        &self,
        accounts: Option<HashSet<u64>>,
    ) -> BichonResult<HashSet<String>> {
        let conn = self.conn()?;
        let mut sql = r#"
            SELECT DISTINCT LOWER(contact) AS contact
            FROM (
                SELECT sender AS contact, account_id
                FROM envelopes
                WHERE sender IS NOT NULL AND sender != ''

                UNION ALL

                SELECT unnest(recipients) AS contact, account_id
                FROM envelopes
                WHERE length(recipients) > 0

                UNION ALL

                SELECT unnest(cc) AS contact, account_id
                FROM envelopes
                WHERE length(cc) > 0

                UNION ALL

                SELECT unnest(bcc) AS contact, account_id
                FROM envelopes
                WHERE length(bcc) > 0
            ) t
            WHERE contact IS NOT NULL AND contact != ''
        "#
        .to_string();

        let mut params_vec: Vec<duckdb::types::Value> = Vec::new();

        if let Some(ref acc_set) = accounts {
            if !acc_set.is_empty() {
                let placeholders = vec!["?"; acc_set.len()].join(", ");
                sql.push_str(&format!(" AND account_id IN ({})", placeholders));

                for &id in acc_set {
                    params_vec.push(id.into());
                }
            }
        }

        sql.push_str(" ORDER BY contact;");

        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

        let contacts: HashSet<String> = stmt
            .query_map(duckdb::params_from_iter(params_vec), |row| {
                let email: String = row.get(0)?;
                Ok(email)
            })
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
            .collect::<Result<HashSet<_>, _>>()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

        Ok(contacts)
    }

    pub fn update_envelope_tags(
        &self,
        updates: HashMap<u64, Vec<u64>>, // account_id -> [envelope_id1, ...]
        tags: Vec<String>,
    ) -> BichonResult<()> {
        let mut conn = self.conn()?;
        let tags_json = serde_json::to_string(&tags)
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        let tx = conn
            .transaction()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        for (account_id, ids) in updates {
            if ids.is_empty() {
                continue;
            }
            let ids_str = ids
                .iter()
                .map(|id| id.to_string())
                .collect::<Vec<_>>()
                .join(",");
            let query = format!(
                "UPDATE envelopes 
                SET tags = CAST(json(?) AS VARCHAR[])
                WHERE account_id = ? 
                AND id IN ({})",
                ids_str
            );
            tx.execute(&query, params![tags_json, account_id])
                .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        }
        tx.commit()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        Ok(())
    }

    pub fn delete_envelopes_multi_account(
        &self,
        deletes: HashMap<u64, Vec<u64>>, // account_id -> [envelope_id1, ...]
    ) -> BichonResult<()> {
        let mut conn = self.conn()?;
        let tx = conn
            .transaction()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        for (account_id, ids) in deletes {
            if ids.is_empty() {
                continue;
            }
            for chunk in ids.chunks(100) {
                let ids_str = chunk
                    .iter()
                    .map(|id| id.to_string())
                    .collect::<Vec<_>>()
                    .join(",");

                let del_attachments_query = format!(
                    "DELETE FROM envelope_attachments 
                 WHERE account_id = ? 
                 AND envelope_id IN ({})",
                    ids_str
                );

                tx.execute(&del_attachments_query, params![account_id])
                    .map_err(|e| {
                        raise_error!(
                            format!("Delete attachments fail: {:#?}", e),
                            ErrorCode::InternalError
                        )
                    })?;

                let query = format!(
                    "DELETE FROM envelopes 
                 WHERE account_id = ? 
                 AND id IN ({})",
                    ids_str
                );
                tx.execute(&query, params![account_id])
                    .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
            }
        }
        tx.commit()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        Ok(())
    }

    pub fn top_10_largest_emails(
        &self,
        accounts: Option<HashSet<u64>>,
    ) -> BichonResult<Vec<LargestEmail>> {
        let conn = self.conn()?;
        let mut sql = r#"
            SELECT id, subject, size_bytes
            FROM envelopes
        "#
        .to_string();

        let mut params_vec: Vec<duckdb::types::Value> = Vec::new();
        if let Some(acc_set) = accounts {
            if !acc_set.is_empty() {
                let placeholders = acc_set.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
                sql.push_str(&format!(" WHERE account_id IN ({})", placeholders));

                for id in acc_set {
                    params_vec.push(id.into());
                }
            }
        }

        sql.push_str(" ORDER BY size_bytes DESC LIMIT 10;");
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        let results = stmt
            .query_map(duckdb::params_from_iter(params_vec), |row| {
                Ok(LargestEmail {
                    id: row.get(0)?,
                    subject: row.get(1)?,
                    size_bytes: row.get(2)?,
                })
            })
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

        Ok(results)
    }

    pub fn get_dashboard_stats(
        &self,
        accounts: Option<HashSet<u64>>,
    ) -> BichonResult<DashboardStats> {
        let conn = self.conn()?;

        let mut account_filter = String::new();
        let mut params: Vec<duckdb::types::Value> = Vec::new();

        if let Some(acc_set) = accounts {
            if !acc_set.is_empty() {
                let placeholders = acc_set.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
                account_filter = format!("WHERE account_id IN ({})", placeholders);
                for id in acc_set {
                    params.push(id.into());
                }
            }
        }

        let mut stats = DashboardStats::default();

        {
            let sql = format!(
                r#"
                SELECT COALESCE(SUM(size_bytes), 0) AS total_size
                FROM envelopes
                {account_filter}
                "#
            );

            let mut stmt = conn.prepare(&sql).map_err(|e| {
                raise_error!(
                    format!("Prepare total_size failed: {}", e),
                    ErrorCode::InternalError
                )
            })?;

            let mut rows = stmt
                .query(duckdb::params_from_iter(params.clone()))
                .map_err(|e| {
                    raise_error!(
                        format!("Failed to execute total_size query: {}", e),
                        ErrorCode::InternalError
                    )
                })?;

            if let Some(row) = rows.next().map_err(|e| {
                raise_error!(
                    format!("Failed to fetch row from total_size query: {}", e),
                    ErrorCode::InternalError
                )
            })? {
                stats.total_size_bytes = row
                    .get(0)
                    .map_err(|e| {
                        raise_error!(
                            format!("Failed to read total_size value: {}", e),
                            ErrorCode::InternalError
                        )
                    })
                    .unwrap_or(0);
            }
        }

        {
            let thirty_days_ago = Utc::now() - chrono::Duration::days(30);
            let thirty_days_ago_ms = thirty_days_ago.timestamp_millis();

            let sql = format!(
                r#"
                SELECT 
                    date_trunc('day', to_timestamp(sent_at / 1000)) AS day,
                    COUNT(*) AS cnt
                FROM envelopes
                {account_filter}
                WHERE sent_at >= ?
                GROUP BY day
                ORDER BY day ASC
                "#
            );

            let mut stmt = conn.prepare(&sql).map_err(|e| {
                raise_error!(
                    format!("Prepare recent_activity failed: {}", e),
                    ErrorCode::InternalError
                )
            })?;

            let mut query_params = params.clone();
            query_params.push(thirty_days_ago_ms.into());

            let mut rows = stmt
                .query(duckdb::params_from_iter(query_params))
                .map_err(|e| {
                    raise_error!(
                        format!("Failed to execute recent_activity query: {}", e),
                        ErrorCode::InternalError
                    )
                })?;

            let mut recent_activity = Vec::new();

            while let Some(row) = rows.next().map_err(|e| {
                raise_error!(
                    format!("Failed to fetch row from recent_activity query: {}", e),
                    ErrorCode::InternalError
                )
            })? {
                let day: NaiveDateTime = row.get(0).map_err(|e| {
                    raise_error!(
                        format!("Failed to read recent_activity value: {}", e),
                        ErrorCode::InternalError
                    )
                })?;
                let timestamp_ms = day.and_utc().timestamp_millis();

                recent_activity.push(TimeBucket {
                    timestamp_ms,
                    count: row.get(1).map_err(|e| {
                        raise_error!(
                            format!("Failed to read recent_activity value: {}", e),
                            ErrorCode::InternalError
                        )
                    })?,
                });
            }

            stats.recent_activity = recent_activity;
        }

        {
            let sql = format!(
                r#"
                SELECT sender, COUNT(*) AS cnt
                FROM envelopes
                {account_filter}
                WHERE sender IS NOT NULL AND sender != ''
                GROUP BY sender
                ORDER BY cnt DESC, sender ASC
                LIMIT 10
                "#
            );

            let mut stmt = conn.prepare(&sql).map_err(|e| {
                raise_error!(
                    format!("Prepare top_senders failed: {}", e),
                    ErrorCode::InternalError
                )
            })?;

            let mut rows = stmt
                .query(duckdb::params_from_iter(params.clone()))
                .map_err(|e| {
                    raise_error!(
                        format!("Failed to execute top_senders query: {}", e),
                        ErrorCode::InternalError
                    )
                })?;

            let mut top_senders = Vec::new();

            while let Some(row) = rows.next().map_err(|e| {
                raise_error!(
                    format!("Failed to fetch row from top_senders query: {}", e),
                    ErrorCode::InternalError
                )
            })? {
                top_senders.push(Group {
                    key: row.get(0).map_err(|e| {
                        raise_error!(
                            format!("Failed to read top_senders value: {}", e),
                            ErrorCode::InternalError
                        )
                    })?,
                    count: row.get(1).map_err(|e| {
                        raise_error!(
                            format!("Failed to read top_senders value: {}", e),
                            ErrorCode::InternalError
                        )
                    })?,
                });
            }

            stats.top_senders = top_senders;
        }
        {
            let sql = format!(
                r#"
                SELECT account_id, COUNT(*) AS cnt
                FROM envelopes
                {account_filter}
                GROUP BY account_id
                ORDER BY cnt DESC
                LIMIT 10
                "#
            );

            let mut stmt = conn.prepare(&sql).map_err(|e| {
                raise_error!(
                    format!("Prepare top_accounts failed: {}", e),
                    ErrorCode::InternalError
                )
            })?;

            let mut rows = stmt
                .query(duckdb::params_from_iter(params.clone()))
                .map_err(|e| {
                    raise_error!(
                        format!("Failed to execute top_accounts query: {}", e),
                        ErrorCode::InternalError
                    )
                })?;

            let mut top_accounts = Vec::new();

            while let Some(row) = rows.next().map_err(|e| {
                raise_error!(
                    format!("Failed to fetch row from top_accounts query: {}", e),
                    ErrorCode::InternalError
                )
            })? {
                let account_id: u64 = row.get(0).map_err(|e| {
                    raise_error!(
                        format!("Failed to read top_accounts value: {}", e),
                        ErrorCode::InternalError
                    )
                })?;
                let count: u64 = row.get(1).map_err(|e| {
                    raise_error!(
                        format!("Failed to read top_accounts value: {}", e),
                        ErrorCode::InternalError
                    )
                })?;

                if let Ok(account) = AccountModel::get(account_id) {
                    top_accounts.push(Group {
                        key: account.email,
                        count,
                    });
                } else {
                    tracing::warn!(account_id, "account not found in top accounts query");
                    tokio::spawn(async move {
                        if let Err(e) = ENVELOPE_INDEX_MANAGER
                            .delete_account_envelopes(account_id)
                            .await
                        {
                            tracing::error!(
                                account_id = account_id,
                                error = %e,
                                "failed to cleanup envelope index"
                            );
                        }
                        if let Err(e) = EML_INDEX_MANAGER.delete_account_envelopes(account_id).await
                        {
                            tracing::error!(
                                account_id = account_id,
                                error = %e,
                                "failed to cleanup eml index"
                            );
                        }
                    });
                }
            }

            stats.top_accounts = top_accounts;
        }

        {
            let sql = format!(
                r#"
                SELECT has_attachment, COUNT(*) AS cnt
                FROM envelopes
                {account_filter}
                GROUP BY has_attachment
                "#
            );

            let mut stmt = conn.prepare(&sql).map_err(|e| {
                raise_error!(
                    format!("Prepare attachment_stats failed: {}", e),
                    ErrorCode::InternalError
                )
            })?;

            let mut rows = stmt.query(duckdb::params_from_iter(params)).map_err(|e| {
                raise_error!(
                    format!("Failed to execute attachment_stats query: {}", e),
                    ErrorCode::InternalError
                )
            })?;

            while let Some(row) = rows.next().map_err(|e| {
                raise_error!(
                    format!("Failed to fetch row from attachment_stats query: {}", e),
                    ErrorCode::InternalError
                )
            })? {
                let has_attachment: bool = row.get(0).map_err(|e| {
                    raise_error!(
                        format!("Failed to read attachment_stats value: {}", e),
                        ErrorCode::InternalError
                    )
                })?;
                let cnt: u64 = row.get(1).map_err(|e| {
                    raise_error!(
                        format!("Failed to read attachment_stats value: {}", e),
                        ErrorCode::InternalError
                    )
                })?;

                if has_attachment {
                    stats.with_attachment_count = cnt;
                } else {
                    stats.without_attachment_count = cnt;
                }
            }
        }

        Ok(stats)
    }

    pub fn list_mailbox_envelopes(
        &self,
        account_id: u64,
        mailbox_id: u64,
        page: u64,
        page_size: u64,
        desc: bool,
    ) -> BichonResult<DataPage<Envelope>> {
        let conn = self.conn()?;
        let current_page = page.max(1);

        let total_items: u64 = conn
            .query_row(
                "SELECT COUNT(*) FROM envelopes WHERE account_id = ? AND mailbox_id = ?",
                params![account_id, mailbox_id],
                |row| row.get(0),
            )
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

        let total_pages = if total_items == 0 {
            0
        } else {
            (total_items + page_size - 1) / page_size
        };

        let offset = (current_page - 1) * page_size;
        let order_direction = if desc { "DESC" } else { "ASC" };
        let query = format!(
            "SELECT * FROM envelopes 
            WHERE account_id = ? AND mailbox_id = ? 
            ORDER BY sent_at {} 
            LIMIT ? OFFSET ?",
            order_direction
        );

        let mut stmt = conn
            .prepare(&query)
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        let items = stmt
            .query_map(params![account_id, mailbox_id, page_size, offset], |row| {
                Envelope::from_row(row)
            })
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

        Ok(DataPage {
            current_page: Some(current_page),
            page_size: Some(page_size),
            total_items,
            items,
            total_pages: Some(total_pages),
        })
    }

    pub fn list_thread_envelopes(
        &self,
        account_id: u64,
        thread_id: u64,
        page: u64,
        page_size: u64,
        desc: bool,
    ) -> BichonResult<DataPage<Envelope>> {
        let conn = self.conn()?;
        let current_page = page.max(1);

        let total_items: u64 = conn
            .query_row(
                "SELECT COUNT(*) FROM envelopes WHERE account_id = ? AND thread_id = ?",
                params![account_id, thread_id],
                |row| row.get(0),
            )
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

        let total_pages = if total_items == 0 {
            0
        } else {
            (total_items + page_size - 1) / page_size
        };

        let offset = (current_page - 1) * page_size;

        let order_direction = if desc { "DESC" } else { "ASC" };
        let query = format!(
            "SELECT * FROM envelopes 
         WHERE account_id = ? AND thread_id = ? 
         ORDER BY sent_at {} 
         LIMIT ? OFFSET ?",
            order_direction
        );

        let mut stmt = conn
            .prepare(&query)
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

        let items = stmt
            .query_map(params![account_id, thread_id, page_size, offset], |row| {
                Envelope::from_row(row)
            })
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

        Ok(DataPage {
            current_page: Some(current_page),
            page_size: Some(page_size),
            total_items,
            items,
            total_pages: Some(total_pages),
        })
    }

    pub fn search(
        &self,
        accounts: Option<HashSet<u64>>,
        filter: SearchFilter,
        page: u64,
        page_size: u64,
        desc: bool,
        sort_by: SortBy,
    ) -> BichonResult<DataPage<Envelope>> {
        let page = page.max(1);
        let page_size = page_size.clamp(1, 500);
        let offset = (page - 1) * page_size;

        let allowed_accounts: Option<HashSet<u64>> = match (accounts, filter.account_ids.clone()) {
            (Some(user_set), Some(filter_set)) => {
                let intersection: HashSet<u64> =
                    user_set.intersection(&filter_set).cloned().collect();
                Some(intersection)
            }
            (Some(user_set), None) => Some(user_set),
            (None, Some(filter_set)) => Some(filter_set),
            (None, None) => None,
        };

        if let Some(ref set) = allowed_accounts {
            if set.is_empty() {
                return Ok(DataPage {
                    current_page: Some(page),
                    page_size: Some(page_size),
                    total_items: 0,
                    items: Vec::new(),
                    total_pages: Some(0),
                });
            }
        }

        let mut base_sql = String::from(" FROM envelopes e ");

        let need_join_attachment = filter.attachment_name.is_some();

        if need_join_attachment {
            base_sql.push_str(
                "
            LEFT JOIN envelope_attachments a
              ON a.account_id = e.account_id
             AND a.mailbox_id = e.mailbox_id
             AND a.envelope_id = e.id
            ",
            );
        }

        base_sql.push_str(" WHERE 1=1 ");
        let mut args: Vec<Value> = Vec::new();

        if let Some(set) = allowed_accounts {
            base_sql.push_str(" AND e.account_id IN (");
            for (i, _) in set.iter().enumerate() {
                if i > 0 {
                    base_sql.push_str(",");
                }
                base_sql.push_str("?");
            }
            base_sql.push_str(")");
            for id in set {
                args.push(id.into());
            }
        }

        if let Some(mailboxes) = filter.mailbox_ids {
            if !mailboxes.is_empty() {
                base_sql.push_str(" AND e.mailbox_id IN (");
                for (i, _) in mailboxes.iter().enumerate() {
                    if i > 0 {
                        base_sql.push_str(",");
                    }
                    base_sql.push_str("?");
                }
                base_sql.push_str(")");
                for id in mailboxes {
                    args.push(id.into());
                }
            }
        }

        if let Some(text) = filter.text {
            let pattern = format!("(?i){}", text);
            base_sql.push_str(
                "
            AND (
                regexp_matches(coalesce(e.subject, ''), ?)
                OR regexp_matches(coalesce(e.body, ''), ?)
            )
            ",
            );
            args.push(pattern.clone().into());
            args.push(pattern.into());
        }

        if let Some(from) = filter.from {
            base_sql.push_str(" AND e.sender ILIKE ? ");
            args.push(format!("%{}%", from).into());
        }

        if let Some(to) = filter.to {
            base_sql.push_str(
                "
            AND EXISTS (
                SELECT 1 FROM UNNEST(e.recipients) r
                WHERE r::VARCHAR ILIKE ?
            )
            ",
            );
            args.push(format!("%{}%", to).into());
        }

        if let Some(cc) = filter.cc {
            base_sql.push_str(
                "
            AND EXISTS (
                SELECT 1 FROM UNNEST(e.cc) r
                WHERE r::VARCHAR ILIKE ?
            )
            ",
            );
            args.push(format!("%{}%", cc).into());
        }

        if let Some(bcc) = filter.bcc {
            base_sql.push_str(
                "
            AND EXISTS (
                SELECT 1 FROM UNNEST(e.bcc) r
                WHERE r::VARCHAR ILIKE ?
            )
            ",
            );
            args.push(format!("%{}%", bcc).into());
        }

        if let Some(since) = filter.since {
            base_sql.push_str(" AND e.sent_at >= ? ");
            args.push(since.into());
        }

        if let Some(before) = filter.before {
            base_sql.push_str(" AND e.sent_at <= ? ");
            args.push(before.into());
        }

        if let Some(min) = filter.min_size {
            base_sql.push_str(" AND e.size_bytes >= ? ");
            args.push(min.into());
        }

        if let Some(max) = filter.max_size {
            base_sql.push_str(" AND e.size_bytes <= ? ");
            args.push(max.into());
        }

        if let Some(mid) = filter.message_id {
            base_sql.push_str(" AND e.message_id ILIKE ? ");
            args.push(format!("%{}%", mid).into());
        }

        if let Some(has) = filter.has_attachment {
            base_sql.push_str(" AND e.has_attachment = ? ");
            args.push(has.into());
        }

        if let Some(tags) = filter.tags {
            if !tags.is_empty() {
                base_sql.push_str(" AND (");
                for i in 0..tags.len() {
                    if i > 0 {
                        base_sql.push_str(" OR ");
                    }
                    base_sql.push_str("list_contains(e.tags, ?)");
                }
                base_sql.push_str(") ");
                for tag in tags {
                    args.push(tag.into());
                }
            }
        }

        if let Some(name) = filter.attachment_name {
            base_sql.push_str(" AND a.filename ILIKE ? ");
            args.push(format!("%{}%", name).into());
        }

        let count_sql = if need_join_attachment {
            format!("SELECT COUNT(DISTINCT e.id) {}", base_sql)
        } else {
            format!("SELECT COUNT(*) {}", base_sql)
        };

        println!("================ COUNT SQL ================");
        println!("FINAL: {}", debug_sql(&count_sql, &args));
        println!("===========================================");

        let total_items: u64 = self
            .conn()?
            .query_row(&count_sql, duckdb::params_from_iter(args.iter()), |row| {
                row.get(0)
            })
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

        if total_items == 0 {
            return Ok(DataPage {
                current_page: Some(page),
                page_size: Some(page_size),
                total_items: 0,
                items: Vec::new(),
                total_pages: Some(0),
            });
        }

        let sort_column = match sort_by {
            SortBy::DATE => "e.sent_at",
            SortBy::SIZE => "e.size_bytes",
        };

        let order = if desc { "DESC" } else { "ASC" };

        let select_prefix = if need_join_attachment {
            "SELECT DISTINCT e.*"
        } else {
            "SELECT e.*"
        };

        let data_sql = format!(
            "{}{}ORDER BY {} {}
            LIMIT ? OFFSET ?
            ",
            select_prefix, base_sql, sort_column, order
        );

        let mut data_args = args.clone();
        data_args.push(page_size.into());
        data_args.push(offset.into());
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(&data_sql)
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

        let items = stmt
            .query_map(duckdb::params_from_iter(data_args.iter()), |row| {
                Envelope::from_row(row)
            })
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

        let total_pages = if total_items == 0 {
            Some(0)
        } else {
            Some((total_items + page_size - 1) / page_size)
        };

        Ok(DataPage {
            current_page: Some(page),
            page_size: Some(page_size),
            total_items,
            items,
            total_pages,
        })
    }
}

pub fn init(
    path: &PathBuf,
    mut migrations_runner: Runner,
) -> BichonResult<r2d2::Pool<DuckdbConnectionManager>> {
    let mut flags = duckdb::Config::default()
        .enable_autoload_extension(true)
        .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
        .access_mode(duckdb::AccessMode::ReadWrite)
        .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
        .with("enable_fsst_vectors", "true")
        .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?
        .with("allocator_background_threads", "true")
        .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

    if let Some(memory_limit) = &SETTINGS.bichon_duckdb_max_memory {
        flags = flags
            .max_memory(memory_limit)
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
    }

    if let Some(threads) = &SETTINGS.bichon_duckdb_threads {
        flags = flags
            .threads(*threads)
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
    }

    let conn = DuckdbConnectionManager::file_with_flags(path, flags)
        .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;

    let pool = r2d2::Pool::new(conn)
        .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
    {
        let conn = pool
            .get()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        conn.execute("PRAGMA enable_checkpoint_on_shutdown", [])
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        conn.pragma_update(None, "autoload_known_extensions", &"true")
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        conn.pragma_update(None, "allow_community_extensions", &"false")
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
    }

    {
        let conn = pool
            .get()
            .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
        migrations_runner.set_migration_table_name("migrations");
        for migration in migrations_runner.run_iter(&mut DuckDBConnection(conn)) {
            match migration {
                Ok(migration) => {
                    tracing::info!("Applied migration: {}", migration);
                }
                Err(err) => {
                    return Err(raise_error!(
                        format!("Failed to apply migration: {}", err),
                        ErrorCode::InternalError
                    ));
                }
            }
        }
    }

    Ok(pool)
}
