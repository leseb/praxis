// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Praxis Contributors

use std::sync::Arc;

use bytes::Bytes;
use serde_json::json;

use super::*;
use crate::{
    FilterAction,
    builtins::http::ai::store::{ConversationRecord, ResponseRecord, ResponseStoreRegistry, StoreError},
};

// -----------------------------------------------------------------------------
// from_config
// -----------------------------------------------------------------------------

#[test]
fn from_config_succeeds() {
    let filter = RehydrateFilter::from_config(&serde_yaml::Value::Null).unwrap();
    assert_eq!(filter.name(), "rehydrate", "filter name should be rehydrate");
}

#[test]
fn body_access_is_read_write() {
    let filter = RehydrateFilter;
    assert_eq!(
        filter.request_body_access(),
        BodyAccess::ReadWrite,
        "filter should use read-write body access"
    );
}

// -----------------------------------------------------------------------------
// Bypass
// -----------------------------------------------------------------------------

#[tokio::test]
async fn skips_non_post_request() {
    let filter = RehydrateFilter;
    let req = crate::test_utils::make_request(http::Method::GET, "/v1/responses");
    let mut ctx = crate::test_utils::make_filter_context(&req);
    let mut body = Some(Bytes::from(r#"{"input":"test"}"#));

    let action = filter.on_request_body(&mut ctx, &mut body, true).await.unwrap();
    assert!(matches!(action, FilterAction::Continue), "non-POST should continue");
}

#[tokio::test]
async fn skips_non_responses_format() {
    let filter = RehydrateFilter;
    let req = crate::test_utils::make_request(http::Method::POST, "/v1/chat/completions");
    let mut ctx = crate::test_utils::make_filter_context(&req);
    ctx.set_metadata("openai_responses_format.format", "openai_chat_completions");
    let mut body = Some(Bytes::from(r#"{"messages":[]}"#));

    let action = filter.on_request_body(&mut ctx, &mut body, true).await.unwrap();
    assert!(
        matches!(action, FilterAction::Release),
        "non-responses format should release"
    );
}

#[tokio::test]
async fn continues_on_non_end_of_stream() {
    let filter = RehydrateFilter;
    let req = crate::test_utils::make_request(http::Method::POST, "/v1/responses");
    let mut ctx = crate::test_utils::make_filter_context(&req);
    let mut body = Some(Bytes::from(r#"{"input":"partial"}"#));

    let action = filter.on_request_body(&mut ctx, &mut body, false).await.unwrap();
    assert!(
        matches!(action, FilterAction::Continue),
        "non-end-of-stream should continue"
    );
}

// -----------------------------------------------------------------------------
// Passthrough
// -----------------------------------------------------------------------------

#[tokio::test]
async fn passthrough_when_no_previous_response_id() {
    let filter = RehydrateFilter;
    let req = crate::test_utils::make_request(http::Method::POST, "/v1/responses");
    let mut ctx = crate::test_utils::make_filter_context(&req);
    ctx.set_metadata("openai_responses_format.format", "openai_responses");
    let mut body = Some(Bytes::from(r#"{"model":"gpt-4.1","input":"Hello"}"#));

    let action = filter.on_request_body(&mut ctx, &mut body, true).await.unwrap();
    assert!(
        matches!(action, FilterAction::Release),
        "should release when no previous_response_id"
    );
    let parsed: Value = serde_json::from_slice(body.as_ref().unwrap()).unwrap();
    assert_eq!(parsed["input"], "Hello", "body should be unchanged");
}

#[tokio::test]
async fn passthrough_when_previous_response_id_is_null() {
    let filter = RehydrateFilter;
    let req = crate::test_utils::make_request(http::Method::POST, "/v1/responses");
    let mut ctx = crate::test_utils::make_filter_context(&req);
    ctx.set_metadata("openai_responses_format.format", "openai_responses");
    let mut body = Some(Bytes::from(
        r#"{"model":"gpt-4.1","input":"Hello","previous_response_id":null}"#,
    ));

    let action = filter.on_request_body(&mut ctx, &mut body, true).await.unwrap();
    assert!(
        matches!(action, FilterAction::Release),
        "should release when previous_response_id is null"
    );
}

// -----------------------------------------------------------------------------
// Rehydration
// -----------------------------------------------------------------------------

#[tokio::test]
async fn rehydrates_with_previous_response_id() {
    let messages = json!([
        {"role": "user", "content": "Hello"},
        {"role": "assistant", "content": "Hi there"}
    ]);
    let store = MockStore::with_completed_response("resp_prev", json!("Hello"), messages);
    let registry = setup_registry(store);

    let filter = RehydrateFilter;
    let req = crate::test_utils::make_request(http::Method::POST, "/v1/responses");
    let mut ctx = crate::test_utils::make_filter_context(&req);
    ctx.response_stores = Some(&registry);
    ctx.set_metadata("openai_responses_format.format", "openai_responses");
    let mut body = Some(Bytes::from(
        r#"{"model":"gpt-4.1","input":"What next?","previous_response_id":"resp_prev"}"#,
    ));

    let action = filter.on_request_body(&mut ctx, &mut body, true).await.unwrap();
    assert!(
        matches!(action, FilterAction::Release),
        "should release after rehydration"
    );

    let parsed: Value = serde_json::from_slice(body.as_ref().unwrap()).unwrap();
    let input = parsed["input"].as_array().unwrap();
    assert_eq!(input.len(), 3, "should have 2 previous messages + 1 current");
    assert_eq!(input[0]["role"], "user", "first message should be user");
    assert_eq!(input[1]["role"], "assistant", "second message should be assistant");
    assert_eq!(input[2], "What next?", "third element should be current input");
    assert_eq!(
        ctx.get_metadata("responses.previous_response_id"),
        Some("resp_prev"),
        "should set previous_response_id metadata"
    );
}

// -----------------------------------------------------------------------------
// Fallback Reconstruction
// -----------------------------------------------------------------------------

#[tokio::test]
async fn fallback_reconstruction_when_messages_empty() {
    let store = MockStore::with_completed_response("resp_prev", json!("Hello"), json!([]));
    let registry = setup_registry(store);

    let filter = RehydrateFilter;
    let req = crate::test_utils::make_request(http::Method::POST, "/v1/responses");
    let mut ctx = crate::test_utils::make_filter_context(&req);
    ctx.response_stores = Some(&registry);
    ctx.set_metadata("openai_responses_format.format", "openai_responses");
    let mut body = Some(Bytes::from(
        r#"{"input":"What next?","previous_response_id":"resp_prev"}"#,
    ));

    let action = filter.on_request_body(&mut ctx, &mut body, true).await.unwrap();
    assert!(
        matches!(action, FilterAction::Release),
        "should release after fallback rehydration"
    );

    let parsed: Value = serde_json::from_slice(body.as_ref().unwrap()).unwrap();
    let input = parsed["input"].as_array().unwrap();
    assert!(
        input.len() >= 2,
        "fallback should reconstruct from input + output (got {len})",
        len = input.len()
    );
}

#[tokio::test]
async fn fallback_reconstruction_when_messages_null() {
    let store = MockStore::with_completed_response("resp_prev", json!("Hello"), Value::Null);
    let registry = setup_registry(store);

    let filter = RehydrateFilter;
    let req = crate::test_utils::make_request(http::Method::POST, "/v1/responses");
    let mut ctx = crate::test_utils::make_filter_context(&req);
    ctx.response_stores = Some(&registry);
    ctx.set_metadata("openai_responses_format.format", "openai_responses");
    let mut body = Some(Bytes::from(
        r#"{"input":"What next?","previous_response_id":"resp_prev"}"#,
    ));

    let action = filter.on_request_body(&mut ctx, &mut body, true).await.unwrap();
    assert!(
        matches!(action, FilterAction::Release),
        "should release after null-messages fallback"
    );

    let parsed: Value = serde_json::from_slice(body.as_ref().unwrap()).unwrap();
    let input = parsed["input"].as_array().unwrap();
    assert!(
        input.len() >= 2,
        "fallback should reconstruct from input + output (got {len})",
        len = input.len()
    );
}

// -----------------------------------------------------------------------------
// Rejections
// -----------------------------------------------------------------------------

#[tokio::test]
async fn rejects_when_previous_response_not_found() {
    let store = MockStore::empty();
    let registry = setup_registry(store);

    let filter = RehydrateFilter;
    let req = crate::test_utils::make_request(http::Method::POST, "/v1/responses");
    let mut ctx = crate::test_utils::make_filter_context(&req);
    ctx.response_stores = Some(&registry);
    ctx.set_metadata("openai_responses_format.format", "openai_responses");
    let mut body = Some(Bytes::from(r#"{"input":"Hi","previous_response_id":"resp_missing"}"#));

    let action = filter.on_request_body(&mut ctx, &mut body, true).await.unwrap();
    match action {
        FilterAction::Reject(r) => assert_eq!(r.status, 400, "should reject with 400"),
        other => panic!("expected Reject, got {other:?}"),
    }
}

#[tokio::test]
async fn rejects_when_status_not_completed() {
    let store = MockStore::with_status("resp_123", "in_progress");
    let registry = setup_registry(store);

    let filter = RehydrateFilter;
    let req = crate::test_utils::make_request(http::Method::POST, "/v1/responses");
    let mut ctx = crate::test_utils::make_filter_context(&req);
    ctx.response_stores = Some(&registry);
    ctx.set_metadata("openai_responses_format.format", "openai_responses");
    let mut body = Some(Bytes::from(r#"{"input":"Hi","previous_response_id":"resp_123"}"#));

    let action = filter.on_request_body(&mut ctx, &mut body, true).await.unwrap();
    match action {
        FilterAction::Reject(r) => assert_eq!(r.status, 400, "should reject non-completed status"),
        other => panic!("expected Reject, got {other:?}"),
    }
}

#[tokio::test]
async fn rejects_when_status_incomplete() {
    let store = MockStore::with_status("resp_123", "incomplete");
    let registry = setup_registry(store);

    let filter = RehydrateFilter;
    let req = crate::test_utils::make_request(http::Method::POST, "/v1/responses");
    let mut ctx = crate::test_utils::make_filter_context(&req);
    ctx.response_stores = Some(&registry);
    ctx.set_metadata("openai_responses_format.format", "openai_responses");
    let mut body = Some(Bytes::from(r#"{"input":"Hi","previous_response_id":"resp_123"}"#));

    let action = filter.on_request_body(&mut ctx, &mut body, true).await.unwrap();
    match action {
        FilterAction::Reject(r) => assert_eq!(r.status, 400, "should reject incomplete status"),
        other => panic!("expected Reject, got {other:?}"),
    }
}

#[tokio::test]
async fn rejects_when_status_failed() {
    let store = MockStore::with_status("resp_123", "failed");
    let registry = setup_registry(store);

    let filter = RehydrateFilter;
    let req = crate::test_utils::make_request(http::Method::POST, "/v1/responses");
    let mut ctx = crate::test_utils::make_filter_context(&req);
    ctx.response_stores = Some(&registry);
    ctx.set_metadata("openai_responses_format.format", "openai_responses");
    let mut body = Some(Bytes::from(r#"{"input":"Hi","previous_response_id":"resp_123"}"#));

    let action = filter.on_request_body(&mut ctx, &mut body, true).await.unwrap();
    match action {
        FilterAction::Reject(r) => assert_eq!(r.status, 400, "should reject failed status"),
        other => panic!("expected Reject, got {other:?}"),
    }
}

#[tokio::test]
async fn rejects_when_store_unavailable() {
    let filter = RehydrateFilter;
    let req = crate::test_utils::make_request(http::Method::POST, "/v1/responses");
    let mut ctx = crate::test_utils::make_filter_context(&req);
    ctx.set_metadata("openai_responses_format.format", "openai_responses");
    let mut body = Some(Bytes::from(r#"{"input":"Hi","previous_response_id":"resp_123"}"#));

    let action = filter.on_request_body(&mut ctx, &mut body, true).await.unwrap();
    match action {
        FilterAction::Reject(r) => assert_eq!(r.status, 500, "should reject with 500 when store unavailable"),
        other => panic!("expected Reject, got {other:?}"),
    }
}

#[tokio::test]
async fn rejects_when_store_not_registered() {
    let registry = ResponseStoreRegistry::new();

    let filter = RehydrateFilter;
    let req = crate::test_utils::make_request(http::Method::POST, "/v1/responses");
    let mut ctx = crate::test_utils::make_filter_context(&req);
    ctx.response_stores = Some(&registry);
    ctx.set_metadata("openai_responses_format.format", "openai_responses");
    let mut body = Some(Bytes::from(r#"{"input":"Hi","previous_response_id":"resp_123"}"#));

    let action = filter.on_request_body(&mut ctx, &mut body, true).await.unwrap();
    match action {
        FilterAction::Reject(r) => assert_eq!(r.status, 500, "should reject with 500 when store not registered"),
        other => panic!("expected Reject, got {other:?}"),
    }
}

#[tokio::test]
async fn rejects_invalid_json_body() {
    let filter = RehydrateFilter;
    let req = crate::test_utils::make_request(http::Method::POST, "/v1/responses");
    let mut ctx = crate::test_utils::make_filter_context(&req);
    ctx.set_metadata("openai_responses_format.format", "openai_responses");
    let mut body = Some(Bytes::from("not json"));

    let action = filter.on_request_body(&mut ctx, &mut body, true).await.unwrap();
    match action {
        FilterAction::Reject(r) => assert_eq!(r.status, 400, "should reject invalid JSON"),
        other => panic!("expected Reject, got {other:?}"),
    }
}

#[tokio::test]
async fn rejects_non_string_previous_response_id() {
    let filter = RehydrateFilter;
    let req = crate::test_utils::make_request(http::Method::POST, "/v1/responses");
    let mut ctx = crate::test_utils::make_filter_context(&req);
    ctx.set_metadata("openai_responses_format.format", "openai_responses");
    let mut body = Some(Bytes::from(r#"{"input":"Hi","previous_response_id":123}"#));

    let action = filter.on_request_body(&mut ctx, &mut body, true).await.unwrap();
    match action {
        FilterAction::Reject(r) => assert_eq!(r.status, 400, "should reject non-string previous_response_id"),
        other => panic!("expected Reject, got {other:?}"),
    }
}

#[tokio::test]
async fn rejects_when_store_fetch_fails() {
    let store = MockStore::failing();
    let registry = setup_registry(store);

    let filter = RehydrateFilter;
    let req = crate::test_utils::make_request(http::Method::POST, "/v1/responses");
    let mut ctx = crate::test_utils::make_filter_context(&req);
    ctx.response_stores = Some(&registry);
    ctx.set_metadata("openai_responses_format.format", "openai_responses");
    let mut body = Some(Bytes::from(r#"{"input":"Hi","previous_response_id":"resp_123"}"#));

    let action = filter.on_request_body(&mut ctx, &mut body, true).await.unwrap();
    match action {
        FilterAction::Reject(r) => assert_eq!(r.status, 500, "should reject with 500 on store error"),
        other => panic!("expected Reject, got {other:?}"),
    }
}

// -----------------------------------------------------------------------------
// Test Utilities
// -----------------------------------------------------------------------------

struct MockStore {
    records: std::collections::HashMap<String, ResponseRecord>,
    should_fail: bool,
}

impl MockStore {
    fn with_completed_response(id: &str, input: Value, messages: Value) -> Self {
        let mut records = std::collections::HashMap::new();
        records.insert(
            id.to_owned(),
            ResponseRecord {
                id: id.to_owned(),
                tenant_id: "default".to_owned(),
                created_at: 1000,
                model: "gpt-4.1".to_owned(),
                response_object: json!({
                    "id": id,
                    "status": "completed",
                    "output": [{"type": "message", "content": [{"type": "output_text", "text": "Hi"}]}]
                }),
                input,
                messages,
            },
        );
        Self {
            records,
            should_fail: false,
        }
    }

    fn with_status(id: &str, status: &str) -> Self {
        let mut records = std::collections::HashMap::new();
        records.insert(
            id.to_owned(),
            ResponseRecord {
                id: id.to_owned(),
                tenant_id: "default".to_owned(),
                created_at: 1000,
                model: "gpt-4.1".to_owned(),
                response_object: json!({"id": id, "status": status}),
                input: json!("Hello"),
                messages: json!([]),
            },
        );
        Self {
            records,
            should_fail: false,
        }
    }

    fn empty() -> Self {
        Self {
            records: std::collections::HashMap::new(),
            should_fail: false,
        }
    }

    fn failing() -> Self {
        Self {
            records: std::collections::HashMap::new(),
            should_fail: true,
        }
    }
}

#[async_trait::async_trait]
impl ResponseStore for MockStore {
    async fn upsert_response(&self, _record: &ResponseRecord) -> Result<(), StoreError> {
        Ok(())
    }

    async fn get_response(&self, _tenant_id: &str, id: &str) -> Result<Option<ResponseRecord>, StoreError> {
        if self.should_fail {
            return Err(StoreError::Unavailable("mock failure".to_owned()));
        }
        Ok(self.records.get(id).cloned())
    }

    async fn delete_response(&self, _tenant_id: &str, _id: &str) -> Result<bool, StoreError> {
        Ok(false)
    }

    async fn upsert_conversation(&self, _record: &ConversationRecord) -> Result<(), StoreError> {
        Ok(())
    }

    async fn get_conversation(
        &self,
        _tenant_id: &str,
        _conversation_id: &str,
    ) -> Result<Option<ConversationRecord>, StoreError> {
        Ok(None)
    }

    async fn delete_conversation(&self, _tenant_id: &str, _conversation_id: &str) -> Result<bool, StoreError> {
        Ok(false)
    }
}

fn setup_registry(store: MockStore) -> ResponseStoreRegistry {
    let registry = ResponseStoreRegistry::new();
    let name: Arc<str> = Arc::from("default");
    registry.register(&name, Arc::new(store)).unwrap();
    registry
}
