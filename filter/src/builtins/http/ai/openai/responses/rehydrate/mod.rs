// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Praxis Contributors

//! Rehydrate filter: loads conversation context from
//! `previous_response_id` or `conversation.id` by fetching
//! stored history and prepending it to the current request.

use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use serde_json::Value;
use tracing::{debug, trace, warn};

use crate::{
    FilterAction, FilterError, Rejection,
    body::{BodyAccess, BodyMode, limits::MAX_JSON_BODY_BYTES},
    builtins::http::ai::store::{ResponseRecord, ResponseStore, ResponseStoreRegistry},
    filter::{HttpFilter, HttpFilterContext},
};

// -----------------------------------------------------------------------------
// Constants
// -----------------------------------------------------------------------------

/// Default store name to look up in the registry.
const DEFAULT_STORE_NAME: &str = "default";

/// Metadata key for the tenant identifier.
const TENANT_METADATA_KEY: &str = "responses.tenant_id";

/// Default tenant identifier.
const DEFAULT_TENANT_ID: &str = "default";

// -----------------------------------------------------------------------------
// RehydrateFilter
// -----------------------------------------------------------------------------

/// Loads conversation context from `previous_response_id` or
/// `conversation.id` and prepends the stored message history
/// to the current request's `input` field.
///
/// # YAML
///
/// ```yaml
/// filter: rehydrate
/// ```
pub struct RehydrateFilter;

impl RehydrateFilter {
    /// Create a filter from YAML config.
    ///
    /// This filter has no configuration fields.
    ///
    /// # Errors
    ///
    /// This function does not return errors; the `Result` return
    /// type is required by the [`FilterFactory`] signature.
    ///
    /// [`FilterFactory`]: crate::FilterFactory
    #[allow(clippy::unnecessary_wraps, reason = "signature required by FilterFactory")]
    pub fn from_config(_config: &serde_yaml::Value) -> Result<Box<dyn HttpFilter>, FilterError> {
        Ok(Box::new(Self))
    }
}

#[async_trait]
impl HttpFilter for RehydrateFilter {
    fn name(&self) -> &'static str {
        "rehydrate"
    }

    fn request_body_access(&self) -> BodyAccess {
        BodyAccess::ReadWrite
    }

    /// `StreamBuffer` so the protocol layer assembles the complete
    /// request body before delivering it at end-of-stream.
    fn request_body_mode(&self) -> BodyMode {
        BodyMode::StreamBuffer {
            max_bytes: Some(MAX_JSON_BODY_BYTES),
        }
    }

    async fn on_request(&self, _ctx: &mut HttpFilterContext<'_>) -> Result<FilterAction, FilterError> {
        Ok(FilterAction::Continue)
    }

    async fn on_request_body(
        &self,
        ctx: &mut HttpFilterContext<'_>,
        body: &mut Option<Bytes>,
        end_of_stream: bool,
    ) -> Result<FilterAction, FilterError> {
        if !end_of_stream {
            return Ok(FilterAction::Continue);
        }

        if ctx.request.method != http::Method::POST {
            return Ok(FilterAction::Continue);
        }

        if ctx.get_metadata("openai_responses_format.format") != Some("openai_responses") {
            return Ok(FilterAction::Release);
        }

        rehydrate_body(ctx, body).await
    }
}

/// Which rehydration source was found in the request body.
enum RehydrateSource {
    /// `previous_response_id` is set.
    PreviousResponse(String),
    /// `conversation.id` is set (without `previous_response_id`).
    Conversation(String),
}

/// Core rehydration: parse body, fetch stored context, assemble
/// conversation history, and write the enriched body back.
async fn rehydrate_body(
    ctx: &mut HttpFilterContext<'_>,
    body: &mut Option<Bytes>,
) -> Result<FilterAction, FilterError> {
    let Some(bytes) = body.as_ref() else {
        return Ok(FilterAction::Release);
    };

    let (mut parsed, source) = match parse_body_and_extract_source(bytes) {
        Ok(Some(pair)) => pair,
        Ok(None) => return Ok(FilterAction::Release),
        Err(action) => return Ok(action),
    };

    let tenant_id = ctx
        .get_metadata(TENANT_METADATA_KEY)
        .unwrap_or(DEFAULT_TENANT_ID)
        .to_owned();

    match source {
        RehydrateSource::PreviousResponse(prev_id) => {
            rehydrate_from_previous_response(ctx, &mut parsed, body, &tenant_id, &prev_id).await
        },
        RehydrateSource::Conversation(conv_id) => {
            rehydrate_from_conversation(ctx, &mut parsed, body, &tenant_id, &conv_id).await
        },
    }
}

/// Rehydrate using a previous response record.
async fn rehydrate_from_previous_response(
    ctx: &mut HttpFilterContext<'_>,
    parsed: &mut Value,
    body: &mut Option<Bytes>,
    tenant_id: &str,
    prev_id: &str,
) -> Result<FilterAction, FilterError> {
    let record = match fetch_previous_response(ctx, tenant_id, prev_id).await {
        Ok(r) => r,
        Err(action) => return Ok(action),
    };

    if let Err(action) = validate_response_status(&record) {
        return Ok(action);
    }

    write_enriched_body(parsed, &record, body)?;

    debug!(previous_response_id = %prev_id, "request rehydrated");
    ctx.set_metadata("responses.previous_response_id", prev_id);

    Ok(FilterAction::Release)
}

/// Rehydrate using stored conversation messages.
async fn rehydrate_from_conversation(
    ctx: &mut HttpFilterContext<'_>,
    parsed: &mut Value,
    body: &mut Option<Bytes>,
    tenant_id: &str,
    conv_id: &str,
) -> Result<FilterAction, FilterError> {
    let record = match fetch_conversation(ctx, tenant_id, conv_id).await {
        Ok(r) => r,
        Err(action) => return Ok(action),
    };

    let current_input = parsed.get("input").cloned().unwrap_or(Value::Null);
    parsed["input"] = assemble_conversation(record.messages, current_input);

    let new_body = serde_json::to_vec(&*parsed)
        .map_err(|e| -> FilterError { format!("failed to serialize rehydrated body: {e}").into() })?;
    *body = Some(Bytes::from(new_body));

    debug!(conversation_id = %conv_id, "request rehydrated from conversation");
    ctx.set_metadata("responses.conversation_id", conv_id);

    Ok(FilterAction::Release)
}

/// Parse the request body and determine the rehydration source.
///
/// Priority: `previous_response_id` > `conversation.id` > passthrough.
/// Returns `Ok(None)` when neither field triggers rehydration.
fn parse_body_and_extract_source(bytes: &[u8]) -> Result<Option<(Value, RehydrateSource)>, FilterAction> {
    let parsed: Value = serde_json::from_slice(bytes).map_err(|e| {
        debug!(error = %e, "rehydrate: invalid request JSON");
        reject_invalid(&format!("invalid request body: {e}"))
    })?;

    let prev_id = match parsed.get("previous_response_id") {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Null) | None => None,
        Some(_) => return Err(reject_invalid("previous_response_id must be a string")),
    };

    if let Some(id) = prev_id {
        return Ok(Some((parsed, RehydrateSource::PreviousResponse(id))));
    }

    let conv_id = parsed
        .get("conversation")
        .map(extract_conversation_id)
        .transpose()?
        .flatten();

    if let Some(id) = conv_id {
        return Ok(Some((parsed, RehydrateSource::Conversation(id))));
    }

    Ok(None)
}

/// Extract `id` from a `conversation` object.
///
/// Returns `Ok(None)` when the conversation object is null or has
/// no `id` field. Returns `Err` for type violations.
fn extract_conversation_id(conv: &Value) -> Result<Option<String>, FilterAction> {
    if conv.is_null() {
        return Ok(None);
    }

    let Some(obj) = conv.as_object() else {
        return Err(reject_invalid("conversation must be an object"));
    };

    match obj.get("id") {
        Some(Value::String(s)) if !s.is_empty() => Ok(Some(s.clone())),
        Some(Value::String(_)) => Err(reject_invalid("conversation.id must not be empty")),
        Some(Value::Null) | None => Ok(None),
        Some(_) => Err(reject_invalid("conversation.id must be a string")),
    }
}

/// Assemble conversation history and write the enriched body.
fn write_enriched_body(
    parsed: &mut Value,
    record: &ResponseRecord,
    body: &mut Option<Bytes>,
) -> Result<(), FilterError> {
    let previous_messages = assemble_previous_messages(record);
    let current_input = parsed.get("input").cloned().unwrap_or(Value::Null);
    parsed["input"] = assemble_conversation(previous_messages, current_input);

    let new_body = serde_json::to_vec(&*parsed)
        .map_err(|e| -> FilterError { format!("failed to serialize rehydrated body: {e}").into() })?;
    *body = Some(Bytes::from(new_body));

    Ok(())
}

// -----------------------------------------------------------------------------
// Fetch & Validate
// -----------------------------------------------------------------------------

/// Resolve the response store from the per-request registry.
fn resolve_store(ctx: &HttpFilterContext<'_>) -> Result<Arc<dyn ResponseStore>, FilterAction> {
    let registry: &ResponseStoreRegistry = ctx.response_stores.ok_or_else(|| {
        warn!("rehydrate: response store registry not available");
        reject_server_error("response store is not available")
    })?;

    registry.get(DEFAULT_STORE_NAME).ok_or_else(|| {
        warn!("rehydrate: default response store not registered");
        reject_server_error("response store is not available")
    })
}

/// Fetch the previous response record from the store.
async fn fetch_previous_response(
    ctx: &HttpFilterContext<'_>,
    tenant_id: &str,
    prev_id: &str,
) -> Result<ResponseRecord, FilterAction> {
    let store = resolve_store(ctx)?;

    let record = store.get_response(tenant_id, prev_id).await.map_err(|e| {
        warn!(error = %e, "rehydrate: failed to fetch previous response");
        reject_server_error("failed to fetch previous response")
    })?;

    record.ok_or_else(|| {
        debug!(id = %prev_id, "rehydrate: previous response not found");
        reject_invalid(&format!("response '{prev_id}' not found"))
    })
}

/// Fetch a conversation record from the store.
async fn fetch_conversation(
    ctx: &HttpFilterContext<'_>,
    tenant_id: &str,
    conv_id: &str,
) -> Result<crate::builtins::http::ai::store::ConversationRecord, FilterAction> {
    let store = resolve_store(ctx)?;

    let record = store.get_conversation(tenant_id, conv_id).await.map_err(|e| {
        warn!(error = %e, "rehydrate: failed to fetch conversation");
        reject_server_error("failed to fetch conversation")
    })?;

    record.ok_or_else(|| {
        debug!(id = %conv_id, "rehydrate: conversation not found");
        reject_invalid(&format!("conversation '{conv_id}' not found"))
    })
}

/// Validate that the stored response has status `"completed"`.
fn validate_response_status(record: &ResponseRecord) -> Result<(), FilterAction> {
    let status = record
        .response_object
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("unknown");

    if status != "completed" {
        return Err(reject_invalid(&format!(
            "cannot continue from response with status '{status}'"
        )));
    }

    Ok(())
}

// -----------------------------------------------------------------------------
// Message Assembly
// -----------------------------------------------------------------------------

/// Extract previous messages from the stored record.
///
/// Uses `ResponseRecord.messages` as the source of truth.
/// Falls back to reconstructing from `input` + `output` when
/// messages is null or empty.
fn assemble_previous_messages(record: &ResponseRecord) -> Value {
    if let Some(arr) = record.messages.as_array()
        && !arr.is_empty()
    {
        return record.messages.clone();
    }

    trace!("rehydrate: messages empty, reconstructing from input + output");
    let mut reconstructed = Vec::new();

    if !record.input.is_null() {
        if let Some(arr) = record.input.as_array() {
            reconstructed.extend(arr.iter().cloned());
        } else {
            reconstructed.push(record.input.clone());
        }
    }

    if let Some(arr) = record.response_object.get("output").and_then(Value::as_array) {
        reconstructed.extend(arr.iter().cloned());
    }

    Value::Array(reconstructed)
}

/// Concatenate previous messages with the current input.
fn assemble_conversation(previous: Value, current: Value) -> Value {
    let mut items = match previous {
        Value::Array(arr) => arr,
        other => vec![other],
    };

    match current {
        Value::Array(arr) => items.extend(arr),
        other => items.push(other),
    }

    Value::Array(items)
}

// -----------------------------------------------------------------------------
// Rejection Helpers
// -----------------------------------------------------------------------------

/// Build a 400 rejection with a JSON error body.
fn reject_invalid(message: &str) -> FilterAction {
    let body = serde_json::json!({
        "error": {
            "message": message,
            "type": "invalid_request_error"
        }
    })
    .to_string();

    FilterAction::Reject(
        Rejection::status(400)
            .with_header("content-type", "application/json")
            .with_body(Bytes::from(body)),
    )
}

/// Build a 500 rejection with a JSON error body.
fn reject_server_error(message: &str) -> FilterAction {
    let body = serde_json::json!({
        "error": {
            "message": message,
            "type": "server_error"
        }
    })
    .to_string();

    FilterAction::Reject(
        Rejection::status(500)
            .with_header("content-type", "application/json")
            .with_body(Bytes::from(body)),
    )
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::needless_raw_strings,
    clippy::needless_raw_string_hashes,
    clippy::too_many_lines,
    reason = "tests"
)]
mod tests;
