// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 Praxis Contributors

//! Functional integration tests for the `guardrails-per-model` example config.
//!
//! Proves issue #1091: a body-inspecting `guardrails` filter runs only for the
//! model promoted from the request body, and is skipped for other models.

use std::collections::HashMap;

use praxis_test_utils::{free_port, http_send, parse_status, start_backend_with_shutdown, start_proxy};

use super::load_example_config;

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

fn proxy_for(backend_port: u16, proxy_port: u16) -> praxis_test_utils::ProxyGuard {
    let config = load_example_config(
        "security/guardrails-per-model.yaml",
        proxy_port,
        HashMap::from([("127.0.0.1:3000", backend_port)]),
    );
    start_proxy(&config)
}

fn post(addr: &str, payload: &str) -> String {
    http_send(
        addr,
        &format!(
            "POST / HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
            payload.len()
        ),
    )
}

#[test]
fn guardrails_per_model_guarded_model_forbidden_body_blocked() {
    let backend_guard = start_backend_with_shutdown("ok");
    let proxy_port = free_port();
    let proxy = proxy_for(backend_guard.port(), proxy_port);

    let raw = post(proxy.addr(), r#"{"model":"guarded-model","prompt":"forbidden"}"#);
    assert_eq!(
        parse_status(&raw),
        403,
        "guardrails should block a forbidden body for the guarded model"
    );
}

#[test]
fn guardrails_per_model_guarded_model_clean_body_allowed() {
    let backend_guard = start_backend_with_shutdown("ok");
    let proxy_port = free_port();
    let proxy = proxy_for(backend_guard.port(), proxy_port);

    let raw = post(proxy.addr(), r#"{"model":"guarded-model","prompt":"hello"}"#);
    assert_eq!(
        parse_status(&raw),
        200,
        "a clean body for the guarded model should pass guardrails"
    );
}

#[test]
fn guardrails_per_model_other_model_skips_guardrails() {
    let backend_guard = start_backend_with_shutdown("ok");
    let proxy_port = free_port();
    let proxy = proxy_for(backend_guard.port(), proxy_port);

    // A different model does not match the gate, so guardrails is skipped and
    // the forbidden body reaches the backend.
    let raw = post(proxy.addr(), r#"{"model":"other-model","prompt":"forbidden"}"#);
    assert_eq!(
        parse_status(&raw),
        200,
        "a non-guarded model should skip guardrails even with a forbidden body"
    );
}
