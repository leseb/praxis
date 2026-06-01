// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Praxis Contributors

//! Input item pagination for the `OpenAI` Responses API.

use crate::builtins::http::ai::store::{ListParams, ResponseRecord, StoreError};

// -----------------------------------------------------------------------------
// InputItemPage
// -----------------------------------------------------------------------------

/// A page of input items from an `OpenAI` Responses API response.
pub struct InputItemPage {
    /// Input items as JSON values (heterogeneous types).
    pub data: Vec<serde_json::Value>,

    /// Cursor for the next page (`None` when no more pages).
    pub next_cursor: Option<String>,

    /// Whether more pages exist beyond this one.
    pub has_more: bool,
}

// -----------------------------------------------------------------------------
// Input Item Pagination
// -----------------------------------------------------------------------------

/// Extract and paginate input items from a [`ResponseRecord`].
///
/// Items are extracted from the stored `input` JSON column and
/// paginated in memory using an offset-based cursor. This is
/// specific to the `OpenAI` Responses API `/v1/responses/{id}/input_items`
/// endpoint.
///
/// # Errors
///
/// Returns [`StoreError::Database`] if the cursor is malformed.
pub fn list_input_items(record: &ResponseRecord, params: &ListParams) -> Result<InputItemPage, StoreError> {
    let items = match &record.input {
        serde_json::Value::Array(arr) => arr.clone(),
        other => vec![other.clone()],
    };

    let offset = params
        .cursor
        .as_deref()
        .map(str::parse::<usize>)
        .transpose()
        .map_err(|e| StoreError::Database(format!("invalid input_items cursor: {e}")))?
        .unwrap_or(0);

    let limit = usize::try_from(params.effective_limit()).map_err(|e| StoreError::Database(e.to_string()))?;
    let end = (offset + limit).min(items.len());
    let has_more = end < items.len();

    let data: Vec<serde_json::Value> = items.into_iter().skip(offset).take(limit).collect();

    let next_cursor = if has_more { Some(end.to_string()) } else { None };

    Ok(InputItemPage {
        data,
        next_cursor,
        has_more,
    })
}
