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

/// Maximum length for a table name identifier.
const MAX_IDENTIFIER_LEN: usize = 128;

/// Reject identifiers that could cause SQL injection or invalid DDL.
fn validate_identifier(name: &str) -> Result<(), StoreError> {
    if name.is_empty() {
        return Err(StoreError::Database("table name must not be empty".to_owned()));
    }
    if name.len() > MAX_IDENTIFIER_LEN {
        return Err(StoreError::Database(format!(
            "table name exceeds {MAX_IDENTIFIER_LEN} characters: {name}"
        )));
    }
    if !name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') {
        return Err(StoreError::Database(format!(
            "table name must start with a letter or underscore: {name}"
        )));
    }
    if !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        return Err(StoreError::Database(format!(
            "table name contains invalid characters: {name}"
        )));
    }
    Ok(())
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    reason = "tests"
)]
mod tests {
    use super::*;

    #[test]
    fn valid_table_name() {
        validate_identifier("openai_responses").expect("valid name should pass");
    }

    #[test]
    fn valid_name_with_underscore_prefix() {
        validate_identifier("_internal").expect("underscore prefix should pass");
    }

    #[test]
    fn reject_empty_name() {
        let err = validate_identifier("").unwrap_err();
        assert!(err.to_string().contains("empty"), "should reject empty name: {err}");
    }

    #[test]
    fn reject_name_starting_with_digit() {
        let err = validate_identifier("123responses").unwrap_err();
        assert!(
            err.to_string().contains("start with"),
            "should reject digit prefix: {err}"
        );
    }

    #[test]
    fn reject_special_characters() {
        let err = validate_identifier("drop; DROP TABLE").unwrap_err();
        assert!(
            err.to_string().contains("invalid characters"),
            "should reject special chars: {err}"
        );
    }

    #[test]
    fn reject_hyphen() {
        let err = validate_identifier("my-table").unwrap_err();
        assert!(
            err.to_string().contains("invalid characters"),
            "should reject hyphen: {err}"
        );
    }

    #[test]
    fn reject_excessively_long_name() {
        let long = "a".repeat(MAX_IDENTIFIER_LEN + 1);
        let err = validate_identifier(&long).unwrap_err();
        assert!(err.to_string().contains("exceeds"), "should reject long name: {err}");
    }

    #[test]
    fn generate_ddl_produces_valid_statements() {
        let tables = TableNames {
            responses: "test_responses".to_owned(),
            conversations: "test_conversations".to_owned(),
        };
        let ddl = generate_ddl(&tables).expect("valid names should produce DDL");
        assert_eq!(ddl.len(), 4, "should produce 4 DDL statements");
        assert!(
            ddl[0].contains("test_responses"),
            "first statement should reference responses table"
        );
    }

    #[test]
    fn generate_ddl_rejects_invalid_name() {
        let tables = TableNames {
            responses: "valid_name".to_owned(),
            conversations: "1invalid".to_owned(),
        };
        let err = generate_ddl(&tables).unwrap_err();
        assert!(
            err.to_string().contains("start with"),
            "should reject invalid conversation table name: {err}"
        );
    }
}
