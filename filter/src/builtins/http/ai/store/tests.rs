// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Praxis Contributors

//! Tests for the response store persistence layer.

use std::sync::Arc;

use serde_json::json;

use super::{
    ConversationRecord, ListParams, Order, ResponseRecord, ResponseStoreRegistry, SqliteResponseStore,
    trait_def::ResponseStore, types::StoreError,
};
use crate::builtins::http::ai::openai::responses::store::list_input_items;

// -----------------------------------------------------------------------------
// Schema Initialization
// -----------------------------------------------------------------------------

#[tokio::test]
async fn sqlite_store_initializes_schema() {
    let store = SqliteResponseStore::new("sqlite::memory:", "test_responses", "test_conversation_messages")
        .await
        .expect("store creation should succeed");

    let result = store
        .list_responses("tenant_a", &ListParams::default())
        .await
        .expect("list should succeed on empty store");

    assert!(result.data.is_empty(), "empty store should return no records");
    assert!(!result.has_more, "empty store should have no more pages");
}

// -----------------------------------------------------------------------------
// Response CRUD
// -----------------------------------------------------------------------------

#[tokio::test]
async fn upsert_and_get_response() {
    let store = make_store().await;
    let record = make_response_record("resp_1", "tenant_a", 1000);

    store.upsert_response(&record).await.expect("upsert should succeed");

    let fetched = store
        .get_response("tenant_a", "resp_1")
        .await
        .expect("get should succeed")
        .expect("record should exist");

    assert_eq!(fetched.id, "resp_1", "ID should match");
    assert_eq!(fetched.tenant_id, "tenant_a", "tenant should match");
    assert_eq!(fetched.created_at, 1000, "created_at should match");
    assert_eq!(fetched.model, "gpt-4.1", "model should match");
    assert_eq!(
        fetched.response_object,
        json!({"status": "completed"}),
        "response_object should match"
    );
}

#[tokio::test]
async fn upsert_overwrites_existing_response() {
    let store = make_store().await;
    let record = make_response_record("resp_1", "tenant_a", 1000);
    store
        .upsert_response(&record)
        .await
        .expect("first upsert should succeed");

    let updated = ResponseRecord {
        model: "gpt-4.1-mini".to_owned(),
        response_object: json!({"status": "incomplete"}),
        ..make_response_record("resp_1", "tenant_a", 1000)
    };
    store
        .upsert_response(&updated)
        .await
        .expect("second upsert should succeed");

    let fetched = store
        .get_response("tenant_a", "resp_1")
        .await
        .expect("get should succeed")
        .expect("record should exist");

    assert_eq!(fetched.model, "gpt-4.1-mini", "model should be updated");
    assert_eq!(
        fetched.response_object,
        json!({"status": "incomplete"}),
        "response_object should be updated"
    );
}

#[tokio::test]
async fn get_missing_response_returns_none() {
    let store = make_store().await;

    let result = store
        .get_response("tenant_a", "nonexistent")
        .await
        .expect("get should succeed");

    assert!(result.is_none(), "missing record should return None");
}

#[tokio::test]
async fn delete_existing_response() {
    let store = make_store().await;
    let record = make_response_record("resp_1", "tenant_a", 1000);
    store.upsert_response(&record).await.expect("upsert should succeed");

    let deleted = store
        .delete_response("tenant_a", "resp_1")
        .await
        .expect("delete should succeed");

    assert!(deleted, "delete should return true for existing record");

    let fetched = store
        .get_response("tenant_a", "resp_1")
        .await
        .expect("get should succeed");

    assert!(fetched.is_none(), "deleted record should not be retrievable");
}

#[tokio::test]
async fn delete_missing_response_returns_false() {
    let store = make_store().await;

    let deleted = store
        .delete_response("tenant_a", "nonexistent")
        .await
        .expect("delete should succeed");

    assert!(!deleted, "delete should return false for missing record");
}

// -----------------------------------------------------------------------------
// Tenant Isolation
// -----------------------------------------------------------------------------

#[tokio::test]
async fn tenant_isolation_on_get() {
    let store = make_store().await;
    let record = make_response_record("resp_1", "tenant_a", 1000);
    store.upsert_response(&record).await.expect("upsert should succeed");

    let result = store
        .get_response("tenant_b", "resp_1")
        .await
        .expect("get should succeed");

    assert!(result.is_none(), "tenant_b should not see tenant_a records");
}

#[tokio::test]
async fn tenant_isolation_on_delete() {
    let store = make_store().await;
    let record = make_response_record("resp_1", "tenant_a", 1000);
    store.upsert_response(&record).await.expect("upsert should succeed");

    let deleted = store
        .delete_response("tenant_b", "resp_1")
        .await
        .expect("delete should succeed");

    assert!(!deleted, "tenant_b should not be able to delete tenant_a records");

    let still_exists = store
        .get_response("tenant_a", "resp_1")
        .await
        .expect("get should succeed");

    assert!(
        still_exists.is_some(),
        "record should still exist after cross-tenant delete attempt"
    );
}

#[tokio::test]
async fn tenant_isolation_on_list() {
    let store = make_store().await;
    store
        .upsert_response(&make_response_record("resp_1", "tenant_a", 1000))
        .await
        .expect("upsert should succeed");
    store
        .upsert_response(&make_response_record("resp_2", "tenant_b", 2000))
        .await
        .expect("upsert should succeed");

    let page_a = store
        .list_responses("tenant_a", &ListParams::default())
        .await
        .expect("list should succeed");

    assert_eq!(page_a.data.len(), 1, "tenant_a should see 1 record");
    assert_eq!(page_a.data[0].id, "resp_1", "tenant_a should see resp_1");

    let page_b = store
        .list_responses("tenant_b", &ListParams::default())
        .await
        .expect("list should succeed");

    assert_eq!(page_b.data.len(), 1, "tenant_b should see 1 record");
    assert_eq!(page_b.data[0].id, "resp_2", "tenant_b should see resp_2");
}

#[tokio::test]
async fn same_response_id_can_exist_in_multiple_tenants() {
    let store = make_store().await;
    store
        .upsert_response(&make_response_record("resp_shared", "tenant_a", 1000))
        .await
        .expect("tenant_a upsert should succeed");
    store
        .upsert_response(&make_response_record("resp_shared", "tenant_b", 2000))
        .await
        .expect("tenant_b upsert should succeed");

    let tenant_a = store
        .get_response("tenant_a", "resp_shared")
        .await
        .expect("tenant_a get should succeed")
        .expect("tenant_a record should exist");
    let tenant_b = store
        .get_response("tenant_b", "resp_shared")
        .await
        .expect("tenant_b get should succeed")
        .expect("tenant_b record should exist");

    assert_eq!(tenant_a.tenant_id, "tenant_a", "tenant_a record should be isolated");
    assert_eq!(tenant_b.tenant_id, "tenant_b", "tenant_b record should be isolated");
    assert_eq!(tenant_a.created_at, 1000, "tenant_a record should not be overwritten");
    assert_eq!(tenant_b.created_at, 2000, "tenant_b record should not be overwritten");
}

// -----------------------------------------------------------------------------
// Pagination
// -----------------------------------------------------------------------------

#[tokio::test]
async fn list_responses_descending_order() {
    let store = make_store().await;
    for i in 1..=5 {
        store
            .upsert_response(&make_response_record(
                &format!("resp_{i}"),
                "tenant_a",
                i64::from(i) * 1000,
            ))
            .await
            .expect("upsert should succeed");
    }

    let page = store
        .list_responses(
            "tenant_a",
            &ListParams {
                order: Order::Descending,
                ..ListParams::default()
            },
        )
        .await
        .expect("list should succeed");

    assert_eq!(page.data.len(), 5, "should return all 5 records");
    assert_eq!(page.data[0].id, "resp_5", "newest should be first");
    assert_eq!(page.data[4].id, "resp_1", "oldest should be last");
    assert!(!page.has_more, "should have no more pages");
}

#[tokio::test]
async fn list_responses_ascending_order() {
    let store = make_store().await;
    for i in 1..=3 {
        store
            .upsert_response(&make_response_record(
                &format!("resp_{i}"),
                "tenant_a",
                i64::from(i) * 1000,
            ))
            .await
            .expect("upsert should succeed");
    }

    let page = store
        .list_responses(
            "tenant_a",
            &ListParams {
                order: Order::Ascending,
                ..ListParams::default()
            },
        )
        .await
        .expect("list should succeed");

    assert_eq!(page.data[0].id, "resp_1", "oldest should be first");
    assert_eq!(page.data[2].id, "resp_3", "newest should be last");
}

#[tokio::test]
async fn list_responses_with_cursor_pagination() {
    let store = make_store().await;
    for i in 1..=5 {
        store
            .upsert_response(&make_response_record(
                &format!("resp_{i}"),
                "tenant_a",
                i64::from(i) * 1000,
            ))
            .await
            .expect("upsert should succeed");
    }

    let page1 = store
        .list_responses(
            "tenant_a",
            &ListParams {
                limit: 2,
                order: Order::Descending,
                ..ListParams::default()
            },
        )
        .await
        .expect("list should succeed");

    assert_eq!(page1.data.len(), 2, "first page should have 2 records");
    assert!(page1.has_more, "should have more pages");
    assert!(page1.next_cursor.is_some(), "should have a next cursor");

    let page2 = store
        .list_responses(
            "tenant_a",
            &ListParams {
                cursor: page1.next_cursor,
                limit: 2,
                order: Order::Descending,
            },
        )
        .await
        .expect("list should succeed");

    assert_eq!(page2.data.len(), 2, "second page should have 2 records");
    assert!(page2.has_more, "should have one more page");

    let page3 = store
        .list_responses(
            "tenant_a",
            &ListParams {
                cursor: page2.next_cursor,
                limit: 2,
                order: Order::Descending,
            },
        )
        .await
        .expect("list should succeed");

    assert_eq!(page3.data.len(), 1, "third page should have 1 record");
    assert!(!page3.has_more, "should have no more pages");
    assert!(page3.next_cursor.is_none(), "should have no next cursor");
}

#[tokio::test]
async fn list_empty_store_returns_empty_page() {
    let store = make_store().await;

    let page = store
        .list_responses("tenant_a", &ListParams::default())
        .await
        .expect("list should succeed");

    assert!(page.data.is_empty(), "empty store should return no records");
    assert!(!page.has_more, "should have no more pages");
    assert!(page.next_cursor.is_none(), "should have no cursor");
}

#[tokio::test]
async fn list_responses_limit_zero_clamps_to_one() {
    let store = make_store().await;
    store
        .upsert_response(&make_response_record("resp_1", "tenant_a", 1000))
        .await
        .expect("upsert should succeed");
    store
        .upsert_response(&make_response_record("resp_2", "tenant_a", 2000))
        .await
        .expect("upsert should succeed");

    let page1 = store
        .list_responses(
            "tenant_a",
            &ListParams {
                limit: 0,
                order: Order::Descending,
                ..ListParams::default()
            },
        )
        .await
        .expect("list should succeed");

    assert_eq!(page1.data.len(), 1, "limit 0 should clamp to one item");
    assert!(page1.has_more, "first page should indicate remaining records");
    assert!(page1.next_cursor.is_some(), "first page should provide a cursor");

    let page2 = store
        .list_responses(
            "tenant_a",
            &ListParams {
                cursor: page1.next_cursor,
                limit: 0,
                order: Order::Descending,
            },
        )
        .await
        .expect("list should succeed");

    assert_eq!(page2.data.len(), 1, "second page should return the remaining record");
    assert!(!page2.has_more, "second page should complete pagination");
    assert!(page2.next_cursor.is_none(), "second page should not provide a cursor");
}

// -----------------------------------------------------------------------------
// Input Items
// -----------------------------------------------------------------------------

#[test]
fn input_items_from_array_input() {
    let record = ResponseRecord {
        input: json!([
            {"type": "message", "role": "user", "content": "Hello"},
            {"type": "message", "role": "user", "content": "World"},
            {"type": "message", "role": "user", "content": "!"}
        ]),
        ..make_response_record("resp_1", "tenant_a", 1000)
    };

    let page = list_input_items(
        &record,
        &ListParams {
            limit: 2,
            ..ListParams::default()
        },
    )
    .expect("list should succeed");

    assert_eq!(page.data.len(), 2, "should return 2 items");
    assert!(page.has_more, "should have more items");
    assert_eq!(
        page.next_cursor.as_deref(),
        Some("2"),
        "cursor should be the next offset"
    );

    let page2 = list_input_items(
        &record,
        &ListParams {
            cursor: page.next_cursor,
            limit: 2,
            ..ListParams::default()
        },
    )
    .expect("list should succeed");

    assert_eq!(page2.data.len(), 1, "should return remaining 1 item");
    assert!(!page2.has_more, "should have no more items");
}

#[test]
fn input_items_from_string_input() {
    let record = ResponseRecord {
        input: json!("Hello, world!"),
        ..make_response_record("resp_1", "tenant_a", 1000)
    };

    let page = list_input_items(&record, &ListParams::default()).expect("list should succeed");

    assert_eq!(page.data.len(), 1, "string input should yield 1 item");
    assert_eq!(page.data[0], json!("Hello, world!"), "item should be the string");
}

#[test]
fn input_items_limit_zero_clamps_to_one() {
    let record = ResponseRecord {
        input: json!([
            {"type": "message", "role": "user", "content": "Hello"},
            {"type": "message", "role": "user", "content": "World"}
        ]),
        ..make_response_record("resp_1", "tenant_a", 1000)
    };

    let page1 = list_input_items(
        &record,
        &ListParams {
            limit: 0,
            ..ListParams::default()
        },
    )
    .expect("list should succeed");

    assert_eq!(page1.data.len(), 1, "limit 0 should clamp to one item");
    assert!(page1.has_more, "first page should indicate remaining items");
    assert_eq!(page1.next_cursor.as_deref(), Some("1"), "cursor should advance by one");

    let page2 = list_input_items(
        &record,
        &ListParams {
            cursor: page1.next_cursor,
            limit: 0,
            ..ListParams::default()
        },
    )
    .expect("list should succeed");

    assert_eq!(page2.data.len(), 1, "second page should return the remaining item");
    assert!(!page2.has_more, "second page should complete pagination");
    assert!(page2.next_cursor.is_none(), "second page should not provide a cursor");
}

// -----------------------------------------------------------------------------
// Conversation CRUD
// -----------------------------------------------------------------------------

#[tokio::test]
async fn upsert_and_get_conversation() {
    let store = make_store().await;
    let record = ConversationRecord {
        conversation_id: "conv_1".to_owned(),
        tenant_id: "tenant_a".to_owned(),
        messages: json!([{"role": "user", "content": "Hi"}]),
    };

    store.upsert_conversation(&record).await.expect("upsert should succeed");

    let fetched = store
        .get_conversation("tenant_a", "conv_1")
        .await
        .expect("get should succeed")
        .expect("record should exist");

    assert_eq!(fetched.conversation_id, "conv_1", "conversation_id should match");
    assert_eq!(
        fetched.messages,
        json!([{"role": "user", "content": "Hi"}]),
        "messages should match"
    );
}

#[tokio::test]
async fn upsert_conversation_overwrites() {
    let store = make_store().await;
    let record = ConversationRecord {
        conversation_id: "conv_1".to_owned(),
        tenant_id: "tenant_a".to_owned(),
        messages: json!([{"role": "user", "content": "v1"}]),
    };
    store.upsert_conversation(&record).await.expect("upsert should succeed");

    let updated = ConversationRecord {
        messages: json!([{"role": "user", "content": "v2"}]),
        ..record
    };
    store
        .upsert_conversation(&updated)
        .await
        .expect("second upsert should succeed");

    let fetched = store
        .get_conversation("tenant_a", "conv_1")
        .await
        .expect("get should succeed")
        .expect("record should exist");

    assert_eq!(
        fetched.messages,
        json!([{"role": "user", "content": "v2"}]),
        "messages should be updated"
    );
}

#[tokio::test]
async fn get_missing_conversation_returns_none() {
    let store = make_store().await;

    let result = store
        .get_conversation("tenant_a", "nonexistent")
        .await
        .expect("get should succeed");

    assert!(result.is_none(), "missing conversation should return None");
}

#[tokio::test]
async fn conversation_tenant_isolation() {
    let store = make_store().await;
    let record = ConversationRecord {
        conversation_id: "conv_1".to_owned(),
        tenant_id: "tenant_a".to_owned(),
        messages: json!([]),
    };
    store.upsert_conversation(&record).await.expect("upsert should succeed");

    let result = store
        .get_conversation("tenant_b", "conv_1")
        .await
        .expect("get should succeed");

    assert!(result.is_none(), "tenant_b should not see tenant_a conversation");
}

#[tokio::test]
async fn delete_existing_conversation() {
    let store = make_store().await;
    let record = ConversationRecord {
        conversation_id: "conv_1".to_owned(),
        tenant_id: "tenant_a".to_owned(),
        messages: json!([]),
    };
    store.upsert_conversation(&record).await.expect("upsert should succeed");

    let deleted = store
        .delete_conversation("tenant_a", "conv_1")
        .await
        .expect("delete should succeed");

    assert!(deleted, "delete should return true for existing conversation");

    let fetched = store
        .get_conversation("tenant_a", "conv_1")
        .await
        .expect("get should succeed");

    assert!(fetched.is_none(), "deleted conversation should not be retrievable");
}

#[tokio::test]
async fn delete_missing_conversation_returns_false() {
    let store = make_store().await;

    let deleted = store
        .delete_conversation("tenant_a", "nonexistent")
        .await
        .expect("delete should succeed");

    assert!(!deleted, "delete should return false for missing conversation");
}

#[tokio::test]
async fn delete_conversation_tenant_isolation() {
    let store = make_store().await;
    let record = ConversationRecord {
        conversation_id: "conv_1".to_owned(),
        tenant_id: "tenant_a".to_owned(),
        messages: json!([]),
    };
    store.upsert_conversation(&record).await.expect("upsert should succeed");

    let deleted = store
        .delete_conversation("tenant_b", "conv_1")
        .await
        .expect("delete should succeed");

    assert!(!deleted, "tenant_b should not be able to delete tenant_a conversation");

    let still_exists = store
        .get_conversation("tenant_a", "conv_1")
        .await
        .expect("get should succeed");

    assert!(
        still_exists.is_some(),
        "conversation should still exist after cross-tenant delete attempt"
    );
}

// -----------------------------------------------------------------------------
// Pagination Edge Cases
// -----------------------------------------------------------------------------

#[tokio::test]
async fn list_responses_invalid_cursor_returns_error() {
    let store = make_store().await;

    let err = store
        .list_responses(
            "tenant_a",
            &ListParams {
                cursor: Some("not_a_valid_cursor".to_owned()),
                ..ListParams::default()
            },
        )
        .await
        .expect_err("invalid cursor should fail");

    assert!(
        matches!(err, StoreError::Database(_)),
        "error should be Database, got: {err}"
    );
}

#[tokio::test]
async fn list_responses_non_numeric_timestamp_cursor_returns_error() {
    let store = make_store().await;

    let err = store
        .list_responses(
            "tenant_a",
            &ListParams {
                cursor: Some("abc:resp_1".to_owned()),
                ..ListParams::default()
            },
        )
        .await
        .expect_err("non-numeric timestamp cursor should fail");

    assert!(
        matches!(err, StoreError::Database(_)),
        "error should be Database, got: {err}"
    );
}

#[tokio::test]
async fn list_responses_ascending_with_cursor() {
    let store = make_store().await;
    for i in 1..=5 {
        store
            .upsert_response(&make_response_record(
                &format!("resp_{i}"),
                "tenant_a",
                i64::from(i) * 1000,
            ))
            .await
            .expect("upsert should succeed");
    }

    let page1 = store
        .list_responses(
            "tenant_a",
            &ListParams {
                limit: 2,
                order: Order::Ascending,
                ..ListParams::default()
            },
        )
        .await
        .expect("list should succeed");

    assert_eq!(page1.data.len(), 2, "first page should have 2 records");
    assert_eq!(page1.data[0].id, "resp_1", "should start with oldest");
    assert!(page1.has_more, "should have more pages");

    let page2 = store
        .list_responses(
            "tenant_a",
            &ListParams {
                cursor: page1.next_cursor,
                limit: 2,
                order: Order::Ascending,
            },
        )
        .await
        .expect("list should succeed");

    assert_eq!(page2.data.len(), 2, "second page should have 2 records");
    assert_eq!(page2.data[0].id, "resp_3", "should continue from cursor");
}

#[tokio::test]
async fn list_responses_limit_above_max_is_clamped() {
    let store = make_store().await;
    for i in 1..=3 {
        store
            .upsert_response(&make_response_record(
                &format!("resp_{i}"),
                "tenant_a",
                i64::from(i) * 1000,
            ))
            .await
            .expect("upsert should succeed");
    }

    let page = store
        .list_responses(
            "tenant_a",
            &ListParams {
                limit: 200,
                ..ListParams::default()
            },
        )
        .await
        .expect("list should succeed");

    assert_eq!(
        page.data.len(),
        3,
        "should return all records (limit clamped, not rejected)"
    );
}

#[test]
fn effective_limit_clamps_to_range() {
    use super::types::{DEFAULT_PAGE_LIMIT, MAX_PAGE_LIMIT};

    let params = ListParams {
        limit: 0,
        ..ListParams::default()
    };
    assert_eq!(params.effective_limit(), 1, "zero should clamp to 1");

    let params = ListParams {
        limit: 200,
        ..ListParams::default()
    };
    assert_eq!(params.effective_limit(), MAX_PAGE_LIMIT, "200 should clamp to max");

    let params = ListParams::default();
    assert_eq!(params.effective_limit(), DEFAULT_PAGE_LIMIT, "default should be 20");

    let params = ListParams {
        limit: 50,
        ..ListParams::default()
    };
    assert_eq!(params.effective_limit(), 50, "50 should pass through");
}

#[test]
fn input_items_from_empty_array() {
    let record = ResponseRecord {
        input: json!([]),
        ..make_response_record("resp_1", "tenant_a", 1000)
    };

    let page = list_input_items(&record, &ListParams::default()).expect("list should succeed");

    assert!(page.data.is_empty(), "empty array should return no items");
    assert!(!page.has_more, "should have no more items");
    assert!(page.next_cursor.is_none(), "should have no cursor");
}

// -----------------------------------------------------------------------------
// Registry
// -----------------------------------------------------------------------------

#[tokio::test]
async fn registry_register_and_get() {
    let registry = ResponseStoreRegistry::new();
    let store = make_store().await;
    registry
        .register("default", Arc::new(store))
        .expect("register should succeed");

    assert!(
        registry.get("default").is_some(),
        "registered store should be retrievable"
    );
}

#[test]
fn registry_get_missing_returns_none() {
    let registry = ResponseStoreRegistry::new();

    assert!(
        registry.get("missing").is_none(),
        "unregistered store should return None"
    );
}

#[tokio::test]
async fn registry_duplicate_registration_fails() {
    let registry = ResponseStoreRegistry::new();
    let store1 = make_store().await;
    let store2 = make_store().await;

    registry
        .register("default", Arc::new(store1))
        .expect("first register should succeed");

    let err = registry
        .register("default", Arc::new(store2))
        .expect_err("duplicate register should fail");

    assert!(
        matches!(err, StoreError::Unavailable(_)),
        "error should be Unavailable, got: {err}"
    );
}

#[test]
fn registry_default_is_empty() {
    let registry = ResponseStoreRegistry::default();
    assert!(
        registry.get("anything").is_none(),
        "default registry should have no stores"
    );
}

// -----------------------------------------------------------------------------
// Test Utilities
// -----------------------------------------------------------------------------

async fn make_store() -> SqliteResponseStore {
    SqliteResponseStore::new("sqlite::memory:", "test_responses", "test_conversation_messages")
        .await
        .expect("store creation should succeed")
}

fn make_response_record(id: &str, tenant_id: &str, created_at: i64) -> ResponseRecord {
    ResponseRecord {
        id: id.to_owned(),
        tenant_id: tenant_id.to_owned(),
        created_at,
        model: "gpt-4.1".to_owned(),
        response_object: json!({"status": "completed"}),
        input: json!("test input"),
        messages: json!([{"role": "user", "content": "hello"}]),
    }
}
