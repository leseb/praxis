// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Praxis Contributors

//! Request-scoped state for the Responses API filter set.
//!
//! [`ResponsesState`] is stored in [`RequestExtensions`] and shared
//! across filter phases. It holds the heavy data needed by the
//! validate → rehydrate → `tool_parse` → `responses_proxy` →
//! `stream_events` → `tool_dispatch` pipeline.
//!
//! [`RequestExtensions`]: crate::extensions::RequestExtensions

/// Request-scoped state shared across Responses API filters.
///
/// Stored in [`RequestExtensions`] by the validate filter and read
/// or mutated by subsequent filters. Uses [`serde_json::Value`] for
/// flexibility while the Responses API types stabilize; can be
/// refactored to typed structs later without affecting external
/// callers.
///
/// [`RequestExtensions`]: crate::extensions::RequestExtensions
pub(crate) struct ResponsesState {
    /// The current request's input items, immutable after construction.
    ///
    /// Preserved as-is so downstream filters can inspect what the
    /// client actually sent, independent of conversation history
    /// resolved by `rehydrate`.
    pub input: Vec<serde_json::Value>,

    /// Current agentic loop iteration (0-indexed). Incremented by
    /// `tool_dispatch` at the start of each new inference round.
    pub iteration: u32,

    /// Resolved conversation history sent to the backend.
    ///
    /// Initialized from the current request's input. When
    /// `previous_response_id` is set, `rehydrate` prepends stored
    /// history. `tool_dispatch` appends tool results during agentic
    /// loops. `responses_proxy` reads this as the authoritative
    /// conversation to send to the backend.
    pub messages: Vec<serde_json::Value>,

    /// Output items accumulated across the current response.
    pub output_items: Vec<serde_json::Value>,

    /// Parsed request body as received from the client.
    pub request_body: serde_json::Value,

    /// The constructed response object for the current iteration.
    pub response_object: serde_json::Value,

    /// Tool calls from the current inference response only.
    ///
    /// Cleared by `tool_dispatch` at the start of each iteration
    /// before `stream_events` writes new ones. Without explicit
    /// clearing, stale tool calls from a previous iteration cause
    /// duplicate dispatch.
    pub tool_calls: Vec<serde_json::Value>,

    /// Tool choice setting. Reset to `"auto"` by `tool_dispatch`
    /// after the first iteration; the original value from the
    /// request only applies to the first inference call.
    pub tool_choice: serde_json::Value,

    /// Processed tool definitions from the request.
    pub tools: Vec<serde_json::Value>,

    /// Token usage accumulated across all iterations within the
    /// request. `stream_events` merges per-iteration usage into
    /// the running total.
    pub usage: serde_json::Value,
}

impl ResponsesState {
    /// Create initial state from a parsed request body.
    pub(crate) fn from_request_body(body: serde_json::Value) -> Self {
        let messages = normalize_input(&body);
        let tools = extract_array_field(&body, "tools");
        let tool_choice = body
            .get("tool_choice")
            .cloned()
            .unwrap_or_else(|| serde_json::Value::String("auto".to_owned()));

        Self {
            input: messages.clone(),
            iteration: 0,
            messages,
            output_items: Vec::new(),
            request_body: body,
            response_object: serde_json::Value::Null,
            tool_calls: Vec::new(),
            tool_choice,
            tools,
            usage: serde_json::Value::Null,
        }
    }
}

/// Normalize the `input` field into a message array.
///
/// The Responses API `input` can be a string (single user message)
/// or an array of message objects. Normalizes both forms to a
/// `Vec<Value>`.
fn normalize_input(body: &serde_json::Value) -> Vec<serde_json::Value> {
    match body.get("input") {
        Some(serde_json::Value::Array(arr)) => arr.clone(),
        Some(serde_json::Value::String(s)) => {
            vec![serde_json::json!({
                "type": "message",
                "role": "user",
                "content": s,
            })]
        },
        _ => Vec::new(),
    }
}

/// Extract a JSON array field by name, defaulting to empty.
fn extract_array_field(body: &serde_json::Value, field: &str) -> Vec<serde_json::Value> {
    body.get(field)
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default()
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
    reason = "tests"
)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn from_request_body_extracts_string_input() {
        let body = json!({
            "model": "gpt-4o",
            "input": "Hello, world!"
        });
        let state = ResponsesState::from_request_body(body);
        assert_eq!(state.input.len(), 1, "string input should produce one item");
        assert_eq!(
            state.input[0]["role"], "user",
            "string input should default to user role"
        );
        assert_eq!(
            state.input[0]["type"], "message",
            "string input should produce a Responses message item"
        );
        assert_eq!(state.input[0]["content"], "Hello, world!");
    }

    #[test]
    fn from_request_body_extracts_array_input() {
        let body = json!({
            "model": "gpt-4o",
            "input": [
                {"role": "user", "content": "first"},
                {"role": "assistant", "content": "second"}
            ]
        });
        let state = ResponsesState::from_request_body(body);
        assert_eq!(state.input.len(), 2, "array input should preserve all items");
    }

    #[test]
    fn from_request_body_empty_input() {
        let body = json!({"model": "gpt-4o"});
        let state = ResponsesState::from_request_body(body);
        assert!(state.input.is_empty(), "missing input should produce empty input");
    }

    #[test]
    fn input_and_messages_start_identical() {
        let body = json!({
            "model": "gpt-4o",
            "input": [
                {"role": "user", "content": "hello"},
                {"role": "assistant", "content": "hi"}
            ]
        });
        let state = ResponsesState::from_request_body(body);
        assert_eq!(
            state.input, state.messages,
            "input and messages should be identical at construction"
        );
    }

    #[test]
    fn from_request_body_extracts_tools() {
        let body = json!({
            "model": "gpt-4o",
            "input": "test",
            "tools": [{"type": "function", "name": "get_weather"}]
        });
        let state = ResponsesState::from_request_body(body);
        assert_eq!(state.tools.len(), 1, "should extract one tool");
    }

    #[test]
    fn from_request_body_default_tool_choice() {
        let body = json!({"model": "gpt-4o", "input": "test"});
        let state = ResponsesState::from_request_body(body);
        assert_eq!(state.tool_choice, json!("auto"), "default tool_choice should be auto");
    }

    #[test]
    fn from_request_body_explicit_tool_choice() {
        let body = json!({
            "model": "gpt-4o",
            "input": "test",
            "tool_choice": "required"
        });
        let state = ResponsesState::from_request_body(body);
        assert_eq!(
            state.tool_choice,
            json!("required"),
            "should preserve explicit tool_choice"
        );
    }

    #[test]
    fn initial_state_has_zero_iteration() {
        let body = json!({"model": "gpt-4o", "input": "test"});
        let state = ResponsesState::from_request_body(body);
        assert_eq!(state.iteration, 0, "initial iteration should be 0");
    }

    #[test]
    fn initial_state_has_empty_tool_calls() {
        let body = json!({"model": "gpt-4o", "input": "test"});
        let state = ResponsesState::from_request_body(body);
        assert!(state.tool_calls.is_empty(), "initial tool_calls should be empty");
    }

    #[test]
    fn initial_state_has_null_usage() {
        let body = json!({"model": "gpt-4o", "input": "test"});
        let state = ResponsesState::from_request_body(body);
        assert!(state.usage.is_null(), "initial usage should be null");
    }

    #[test]
    fn request_body_is_preserved() {
        let body = json!({"model": "gpt-4o", "input": "hello", "temperature": 0.7});
        let state = ResponsesState::from_request_body(body.clone());
        assert_eq!(state.request_body, body, "original request body should be preserved");
    }
}
