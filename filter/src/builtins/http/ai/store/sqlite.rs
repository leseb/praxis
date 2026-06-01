// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Praxis Contributors

//! [`SqliteResponseStore`] — SQLite backend for the response store.

use async_trait::async_trait;
use sqlx::{Row, SqlitePool, sqlite::SqliteConnectOptions};
use tracing::info;

use super::{
    schemas::{TableNames, generate_ddl},
    store::ResponseStore,
    types::{ConversationRecord, ListParams, Order, ResponsePage, ResponseRecord, StoreError},
};

// -----------------------------------------------------------------------------
// SqliteResponseStore
// -----------------------------------------------------------------------------

/// SQLite-backed response store.
///
/// Uses [`sqlx::SqlitePool`] for async connection pooling. Table
/// names are configurable per provider (e.g., `openai_responses`,
/// `google_interactions`) to isolate data per provider.
pub struct SqliteResponseStore {
    /// Connection pool.
    pool: SqlitePool,
    /// Configured table names.
    tables: TableNames,
}

impl SqliteResponseStore {
    /// Create a new store and initialize the schema.
    ///
    /// The `database_url` is a SQLite connection string. Use
    /// `"sqlite::memory:"` for in-memory databases (testing) or
    /// `"sqlite:///path/to/db.sqlite?mode=rwc"` for file-backed.
    ///
    /// `responses_table` and `conversations_table` are the SQL
    /// table names to use. These come from the filter's YAML
    /// config (e.g., `openai_responses`).
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the connection, schema
    /// initialization, or table name validation fails.
    pub async fn new(database_url: &str, responses_table: &str, conversations_table: &str) -> Result<Self, StoreError> {
        let options: SqliteConnectOptions = database_url
            .parse()
            .map_err(|e: sqlx::Error| StoreError::Database(e.to_string()))?;

        let pool = SqlitePool::connect_with(options.create_if_missing(true))
            .await
            .map_err(|e| StoreError::Database(e.to_string()))?;

        let tables = TableNames {
            responses: responses_table.to_owned(),
            conversations: conversations_table.to_owned(),
        };
        let ddl = generate_ddl(&tables)?;
        for statement in &ddl {
            sqlx::query(statement)
                .execute(&pool)
                .await
                .map_err(|e| StoreError::Database(e.to_string()))?;
        }

        info!(
            responses = responses_table,
            conversations = conversations_table,
            "response store initialized"
        );
        Ok(Self { pool, tables })
    }
}

#[async_trait]
impl ResponseStore for SqliteResponseStore {
    async fn upsert_response(&self, record: &ResponseRecord) -> Result<(), StoreError> {
        let response_object =
            serde_json::to_string(&record.response_object).map_err(|e| StoreError::Serialization(e.to_string()))?;
        let input = serde_json::to_string(&record.input).map_err(|e| StoreError::Serialization(e.to_string()))?;
        let messages = serde_json::to_string(&record.messages).map_err(|e| StoreError::Serialization(e.to_string()))?;

        let sql = format!(
            "INSERT OR REPLACE INTO {} \
             (id, tenant_id, created_at, model, response_object, input, messages) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            self.tables.responses
        );

        sqlx::query(&sql)
            .bind(&record.id)
            .bind(&record.tenant_id)
            .bind(record.created_at)
            .bind(&record.model)
            .bind(&response_object)
            .bind(&input)
            .bind(&messages)
            .execute(&self.pool)
            .await
            .map_err(|e| StoreError::Database(e.to_string()))?;

        Ok(())
    }

    async fn get_response(&self, tenant_id: &str, id: &str) -> Result<Option<ResponseRecord>, StoreError> {
        let sql = format!(
            "SELECT id, tenant_id, created_at, model, \
                    response_object, input, messages \
             FROM {} \
             WHERE id = ? AND tenant_id = ?",
            self.tables.responses
        );

        let row = sqlx::query(&sql)
            .bind(id)
            .bind(tenant_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| StoreError::Database(e.to_string()))?;

        row.map(|r| row_to_response_record(&r)).transpose()
    }

    async fn delete_response(&self, tenant_id: &str, id: &str) -> Result<bool, StoreError> {
        let sql = format!("DELETE FROM {} WHERE id = ? AND tenant_id = ?", self.tables.responses);

        let result = sqlx::query(&sql)
            .bind(id)
            .bind(tenant_id)
            .execute(&self.pool)
            .await
            .map_err(|e| StoreError::Database(e.to_string()))?;

        Ok(result.rows_affected() > 0)
    }

    async fn list_responses(&self, tenant_id: &str, params: &ListParams) -> Result<ResponsePage, StoreError> {
        let limit = params.effective_limit();
        let fetch_limit = i64::from(limit) + 1;
        let t = &self.tables.responses;

        let rows = match (&params.cursor, params.order) {
            (None, Order::Descending) => {
                let sql = format!(
                    "SELECT id, tenant_id, created_at, model, \
                            response_object, input, messages \
                     FROM {t} \
                     WHERE tenant_id = ? \
                     ORDER BY created_at DESC, id DESC \
                     LIMIT ?"
                );
                sqlx::query(&sql)
                    .bind(tenant_id)
                    .bind(fetch_limit)
                    .fetch_all(&self.pool)
                    .await
            },
            (None, Order::Ascending) => {
                let sql = format!(
                    "SELECT id, tenant_id, created_at, model, \
                            response_object, input, messages \
                     FROM {t} \
                     WHERE tenant_id = ? \
                     ORDER BY created_at ASC, id ASC \
                     LIMIT ?"
                );
                sqlx::query(&sql)
                    .bind(tenant_id)
                    .bind(fetch_limit)
                    .fetch_all(&self.pool)
                    .await
            },
            (Some(cursor), Order::Descending) => {
                let (cursor_ts, cursor_id) = parse_cursor(cursor)?;
                let sql = format!(
                    "SELECT id, tenant_id, created_at, model, \
                            response_object, input, messages \
                     FROM {t} \
                     WHERE tenant_id = ? \
                       AND (created_at < ? OR (created_at = ? AND id < ?)) \
                     ORDER BY created_at DESC, id DESC \
                     LIMIT ?"
                );
                sqlx::query(&sql)
                    .bind(tenant_id)
                    .bind(cursor_ts)
                    .bind(cursor_ts)
                    .bind(cursor_id)
                    .bind(fetch_limit)
                    .fetch_all(&self.pool)
                    .await
            },
            (Some(cursor), Order::Ascending) => {
                let (cursor_ts, cursor_id) = parse_cursor(cursor)?;
                let sql = format!(
                    "SELECT id, tenant_id, created_at, model, \
                            response_object, input, messages \
                     FROM {t} \
                     WHERE tenant_id = ? \
                       AND (created_at > ? OR (created_at = ? AND id > ?)) \
                     ORDER BY created_at ASC, id ASC \
                     LIMIT ?"
                );
                sqlx::query(&sql)
                    .bind(tenant_id)
                    .bind(cursor_ts)
                    .bind(cursor_ts)
                    .bind(cursor_id)
                    .bind(fetch_limit)
                    .fetch_all(&self.pool)
                    .await
            },
        }
        .map_err(|e| StoreError::Database(e.to_string()))?;

        let limit_usize = usize::try_from(limit).map_err(|e| StoreError::Database(e.to_string()))?;

        let has_more = rows.len() > limit_usize;

        let data: Vec<ResponseRecord> = rows
            .iter()
            .take(limit_usize)
            .map(row_to_response_record)
            .collect::<Result<Vec<_>, _>>()?;

        let next_cursor = if has_more {
            data.last().map(|r| encode_cursor(r.created_at, &r.id))
        } else {
            None
        };

        Ok(ResponsePage {
            data,
            next_cursor,
            has_more,
        })
    }

    async fn upsert_conversation(&self, record: &ConversationRecord) -> Result<(), StoreError> {
        let messages = serde_json::to_string(&record.messages).map_err(|e| StoreError::Serialization(e.to_string()))?;

        let sql = format!(
            "INSERT OR REPLACE INTO {} \
             (conversation_id, tenant_id, messages) \
             VALUES (?, ?, ?)",
            self.tables.conversations
        );

        sqlx::query(&sql)
            .bind(&record.conversation_id)
            .bind(&record.tenant_id)
            .bind(&messages)
            .execute(&self.pool)
            .await
            .map_err(|e| StoreError::Database(e.to_string()))?;

        Ok(())
    }

    async fn get_conversation(
        &self,
        tenant_id: &str,
        conversation_id: &str,
    ) -> Result<Option<ConversationRecord>, StoreError> {
        let sql = format!(
            "SELECT conversation_id, tenant_id, messages \
             FROM {} \
             WHERE conversation_id = ? AND tenant_id = ?",
            self.tables.conversations
        );

        let row = sqlx::query(&sql)
            .bind(conversation_id)
            .bind(tenant_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| StoreError::Database(e.to_string()))?;

        row.map(|r| {
            let messages_json: String = r.try_get("messages").map_err(|e| StoreError::Database(e.to_string()))?;
            let messages: serde_json::Value =
                serde_json::from_str(&messages_json).map_err(|e| StoreError::Serialization(e.to_string()))?;
            Ok(ConversationRecord {
                conversation_id: r
                    .try_get("conversation_id")
                    .map_err(|e| StoreError::Database(e.to_string()))?,
                tenant_id: r
                    .try_get("tenant_id")
                    .map_err(|e| StoreError::Database(e.to_string()))?,
                messages,
            })
        })
        .transpose()
    }
}

// -----------------------------------------------------------------------------
// Cursor Helpers
// -----------------------------------------------------------------------------

/// Encode a `(created_at, id)` pair as a cursor string.
fn encode_cursor(created_at: i64, id: &str) -> String {
    format!("{created_at}:{id}")
}

/// Decode a cursor string into a `(created_at, id)` pair.
fn parse_cursor(cursor: &str) -> Result<(i64, &str), StoreError> {
    let (ts_str, id) = cursor
        .split_once(':')
        .ok_or_else(|| StoreError::Database(format!("invalid cursor format: {cursor}")))?;
    let ts: i64 = ts_str
        .parse()
        .map_err(|e| StoreError::Database(format!("invalid cursor timestamp: {e}")))?;
    Ok((ts, id))
}

/// Convert a sqlx row to a [`ResponseRecord`].
fn row_to_response_record(row: &sqlx::sqlite::SqliteRow) -> Result<ResponseRecord, StoreError> {
    let response_object_json: String = row
        .try_get("response_object")
        .map_err(|e| StoreError::Database(e.to_string()))?;
    let input_json: String = row.try_get("input").map_err(|e| StoreError::Database(e.to_string()))?;
    let messages_json: String = row
        .try_get("messages")
        .map_err(|e| StoreError::Database(e.to_string()))?;

    Ok(ResponseRecord {
        id: row.try_get("id").map_err(|e| StoreError::Database(e.to_string()))?,
        tenant_id: row
            .try_get("tenant_id")
            .map_err(|e| StoreError::Database(e.to_string()))?,
        created_at: row
            .try_get("created_at")
            .map_err(|e| StoreError::Database(e.to_string()))?,
        model: row.try_get("model").map_err(|e| StoreError::Database(e.to_string()))?,
        response_object: serde_json::from_str(&response_object_json)
            .map_err(|e| StoreError::Serialization(e.to_string()))?,
        input: serde_json::from_str(&input_json).map_err(|e| StoreError::Serialization(e.to_string()))?,
        messages: serde_json::from_str(&messages_json).map_err(|e| StoreError::Serialization(e.to_string()))?,
    })
}
