// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Praxis Contributors

//! SQL schema generation for the response store.

use super::types::StoreError;

// -----------------------------------------------------------------------------
// Table Names
// -----------------------------------------------------------------------------

/// Resolved table names for a store instance.
///
/// Table names are configured via YAML (e.g.,
/// `openai_responses`, `google_interactions`). Each provider
/// chooses its own names.
pub(crate) struct TableNames {
    /// Responses table name.
    pub responses: String,
    /// Conversation messages table name.
    pub conversations: String,
}

// -----------------------------------------------------------------------------
// Schema DDL
// -----------------------------------------------------------------------------

/// Generate DDL statements for the given table names.
///
/// Each statement uses `IF NOT EXISTS` so it is safe to run on
/// every startup. The schema uses TEXT for JSON columns (standard
/// `SQLite` pattern).
///
/// # Errors
///
/// Returns [`StoreError::Database`] if table names contain
/// invalid characters.
pub(crate) fn generate_ddl(tables: &TableNames) -> Result<Vec<String>, StoreError> {
    let r = &tables.responses;
    let c = &tables.conversations;

    validate_identifier(r)?;
    validate_identifier(c)?;

    Ok(vec![
        format!(
            "CREATE TABLE IF NOT EXISTS {r} (
                tenant_id       TEXT NOT NULL,
                id              TEXT NOT NULL,
                created_at      INTEGER NOT NULL,
                model           TEXT NOT NULL,
                response_object TEXT NOT NULL,
                input           TEXT NOT NULL,
                messages        TEXT NOT NULL,
                PRIMARY KEY (tenant_id, id)
            )"
        ),
        format!("CREATE INDEX IF NOT EXISTS idx_{r}_tenant_created ON {r}(tenant_id, created_at)"),
        format!(
            "CREATE TABLE IF NOT EXISTS {c} (
                conversation_id TEXT NOT NULL,
                tenant_id       TEXT NOT NULL,
                messages        TEXT NOT NULL,
                PRIMARY KEY (conversation_id, tenant_id)
            )"
        ),
        format!("CREATE INDEX IF NOT EXISTS idx_{c}_tenant_id ON {c}(tenant_id)"),
    ])
}

/// Reject identifiers that could cause SQL injection.
fn validate_identifier(name: &str) -> Result<(), StoreError> {
    if name.is_empty() {
        return Err(StoreError::Database("table name must not be empty".to_owned()));
    }
    if !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        return Err(StoreError::Database(format!(
            "table name contains invalid characters: {name}"
        )));
    }
    Ok(())
}
