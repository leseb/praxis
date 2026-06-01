// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Praxis Contributors

//! Data types for the response store persistence layer.

use std::fmt;

// -----------------------------------------------------------------------------
// Constants
// -----------------------------------------------------------------------------

/// Default page size for list operations (matches OpenAI default).
pub(crate) const DEFAULT_PAGE_LIMIT: u32 = 20;

/// Maximum page size for list operations (matches OpenAI maximum).
pub(crate) const MAX_PAGE_LIMIT: u32 = 100;

// -----------------------------------------------------------------------------
// ResponseRecord
// -----------------------------------------------------------------------------

/// A stored response record.
///
/// Holds the full response object, original input, and hidden
/// messages used for multi-turn conversation rehydration. JSON
/// columns use [`serde_json::Value`] — the store is intentionally
/// schema-agnostic about their contents.
pub struct ResponseRecord {
    /// Unique response ID (e.g., `"resp_abc123"`).
    pub id: String,

    /// Tenant ID for multi-tenant isolation.
    pub tenant_id: String,

    /// Unix timestamp when the response was created.
    pub created_at: i64,

    /// Model name used for inference.
    pub model: String,

    /// Full `ResponseResource` as JSON (the public API object).
    pub response_object: serde_json::Value,

    /// Original input as JSON (preserved for the `input_items`
    /// endpoint).
    pub input: serde_json::Value,

    /// Hidden messages as JSON — source of truth for future
    /// turns. Includes system messages and internal state not
    /// exposed in the public response object.
    pub messages: serde_json::Value,
}

// -----------------------------------------------------------------------------
// ConversationRecord
// -----------------------------------------------------------------------------

/// A stored conversation message cache.
///
/// Holds accumulated conversation messages for a conversation ID,
/// used by the rehydrate filter to load multi-turn context.
pub struct ConversationRecord {
    /// Conversation ID.
    pub conversation_id: String,

    /// Tenant ID for multi-tenant isolation.
    pub tenant_id: String,

    /// Accumulated conversation messages as JSON.
    pub messages: serde_json::Value,
}

// -----------------------------------------------------------------------------
// Pagination
// -----------------------------------------------------------------------------

/// Sort order for list operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Order {
    /// Oldest first.
    Ascending,

    /// Newest first.
    Descending,
}

impl Default for Order {
    fn default() -> Self {
        Self::Descending
    }
}

/// Cursor-based pagination parameters.
#[derive(Debug, Clone)]
pub struct ListParams {
    /// Opaque cursor for the next page. `None` starts from the
    /// beginning.
    pub cursor: Option<String>,

    /// Maximum number of items to return (clamped to
    /// `1..=[MAX_PAGE_LIMIT]`).
    pub limit: u32,

    /// Sort order.
    pub order: Order,
}

impl Default for ListParams {
    fn default() -> Self {
        Self {
            cursor: None,
            limit: DEFAULT_PAGE_LIMIT,
            order: Order::default(),
        }
    }
}

impl ListParams {
    /// Return the effective limit, clamped to `1..=[MAX_PAGE_LIMIT]`.
    pub(crate) fn effective_limit(&self) -> u32 {
        self.limit.max(1).min(MAX_PAGE_LIMIT)
    }
}

/// A page of response records.
pub struct ResponsePage {
    /// The response records in this page.
    pub data: Vec<ResponseRecord>,

    /// Cursor for the next page (`None` when no more pages).
    pub next_cursor: Option<String>,

    /// Whether more pages exist beyond this one.
    pub has_more: bool,
}

// -----------------------------------------------------------------------------
// StoreError
// -----------------------------------------------------------------------------

/// Errors from response store operations.
///
/// Variants carry `String` payloads (not typed inner errors) to
/// avoid coupling the trait to any specific database driver.
#[derive(Debug)]
pub enum StoreError {
    /// Database connection or query failure.
    Database(String),

    /// JSON serialization or deserialization failure.
    Serialization(String),

    /// Store not initialized or unavailable.
    Unavailable(String),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Database(msg) => write!(f, "database error: {msg}"),
            Self::Serialization(msg) => write!(f, "serialization error: {msg}"),
            Self::Unavailable(msg) => write!(f, "store unavailable: {msg}"),
        }
    }
}

impl std::error::Error for StoreError {}
