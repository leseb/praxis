// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 Praxis Contributors

//! Regression coverage for the StreamBuffer adapter contract that
//! protocol-specific body mutators rely on.

use praxis_core::config::Config;
use praxis_test_utils::{
    filters::{BodyMutatingStreamBufferFilter, ConditionRecordingStreamBufferFilter},
    free_port, http_send, parse_body, parse_status, start_echo_backend, start_header_echo_backend,
    start_proxy_with_registry, start_uri_echo_backend,
};

#[test]
fn stream_buffer_readwrite_mutated_body_reaches_backend() {
    let backend_guard = start_echo_backend();
    let proxy_port = free_port();

    let yaml = mutator_yaml(proxy_port, backend_guard.port());
    let config = Config::from_yaml(&yaml).unwrap();
    let registry = registry_with_mutator();
    let proxy = start_proxy_with_registry(&config, &registry);

    let request = format!(
        "POST / HTTP/1.1\r\n\
         Host: localhost\r\n\
         Content-Type: application/octet-stream\r\n\
         Content-Length: 20\r\n\
         \r\n\
         original-body-here!!"
    );
    let raw = http_send(proxy.addr(), &request);

    assert_eq!(parse_status(&raw), 200);
    assert_eq!(
        parse_body(&raw),
        "mutated",
        "backend should receive the mutated body, not the original"
    );
}

#[test]
fn stream_buffer_readwrite_rewritten_path_reaches_backend() {
    let backend_guard = start_uri_echo_backend();
    let proxy_port = free_port();

    let yaml = mutator_yaml(proxy_port, backend_guard.port());
    let config = Config::from_yaml(&yaml).unwrap();
    let registry = registry_with_mutator();
    let proxy = start_proxy_with_registry(&config, &registry);

    let request = format!(
        "POST /original HTTP/1.1\r\n\
         Host: localhost\r\n\
         Content-Type: application/octet-stream\r\n\
         Content-Length: 4\r\n\
         \r\n\
         test"
    );
    let raw = http_send(proxy.addr(), &request);

    assert_eq!(parse_status(&raw), 200);
    let echoed_path = parse_body(&raw);
    assert_eq!(
        echoed_path, "/rewritten/path",
        "backend should receive the rewritten path set during body-phase pre-read"
    );
}

#[test]
fn stream_buffer_readwrite_content_length_repaired() {
    let backend_guard = start_header_echo_backend();
    let proxy_port = free_port();

    let yaml = mutator_yaml(proxy_port, backend_guard.port());
    let config = Config::from_yaml(&yaml).unwrap();
    let registry = registry_with_mutator();
    let proxy = start_proxy_with_registry(&config, &registry);

    let original = "this-is-a-longer-original-body-that-will-be-replaced";
    let request = format!(
        "POST / HTTP/1.1\r\n\
         Host: localhost\r\n\
         Content-Type: application/octet-stream\r\n\
         Content-Length: {}\r\n\
         \r\n\
         {original}",
        original.len(),
    );
    let raw = http_send(proxy.addr(), &request);

    assert_eq!(parse_status(&raw), 200);
    let echoed_headers = parse_body(&raw);
    let echoed_lower = echoed_headers.to_lowercase();
    assert!(
        echoed_lower.contains("content-length: 7"),
        "backend should receive repaired Content-Length for mutated body: {echoed_headers}"
    );
    assert!(
        !echoed_lower.contains(&format!("content-length: {}", original.len())),
        "backend should not receive original Content-Length after body mutation: {echoed_headers}"
    );
}

// -----------------------------------------------------------------------------
// Body-phase Header Writeback Tests
// -----------------------------------------------------------------------------

#[test]
fn stream_buffer_body_phase_headers_to_set_reach_backend() {
    let backend_guard = start_header_echo_backend();
    let proxy_port = free_port();

    let yaml = header_mutator_yaml(proxy_port, backend_guard.port());
    let config = Config::from_yaml(&yaml).unwrap();
    let registry = registry_with_header_mutator();
    let proxy = start_proxy_with_registry(&config, &registry);

    let request = format!(
        "POST / HTTP/1.1\r\n\
         Host: localhost\r\n\
         Content-Type: text/plain\r\n\
         Content-Length: 4\r\n\
         x-client-remove-me: present\r\n\
         \r\n\
         test"
    );
    let raw = http_send(proxy.addr(), &request);

    assert_eq!(parse_status(&raw), 200);
    let headers = parse_body(&raw);
    assert!(
        headers.contains("x-body-phase-set: from-body-filter"),
        "body-phase headers_to_set should reach backend: {headers}"
    );
}

#[test]
fn stream_buffer_body_phase_headers_to_remove_reach_backend() {
    let backend_guard = start_header_echo_backend();
    let proxy_port = free_port();

    let yaml = header_mutator_yaml(proxy_port, backend_guard.port());
    let config = Config::from_yaml(&yaml).unwrap();
    let registry = registry_with_header_mutator();
    let proxy = start_proxy_with_registry(&config, &registry);

    let request = format!(
        "POST / HTTP/1.1\r\n\
         Host: localhost\r\n\
         Content-Type: text/plain\r\n\
         Content-Length: 4\r\n\
         x-client-remove-me: present\r\n\
         \r\n\
         test"
    );
    let raw = http_send(proxy.addr(), &request);

    assert_eq!(parse_status(&raw), 200);
    let headers = parse_body(&raw);
    assert!(
        !headers.contains("x-client-remove-me"),
        "body-phase headers_to_remove should strip client header: {headers}"
    );
}

#[test]
fn stream_buffer_body_phase_reserved_header_stripped_before_upstream() {
    let backend_guard = start_header_echo_backend();
    let proxy_port = free_port();

    let yaml = header_mutator_yaml(proxy_port, backend_guard.port());
    let config = Config::from_yaml(&yaml).unwrap();
    let registry = registry_with_header_mutator();
    let proxy = start_proxy_with_registry(&config, &registry);

    let request = format!(
        "POST / HTTP/1.1\r\n\
         Host: localhost\r\n\
         Content-Type: text/plain\r\n\
         Content-Length: 4\r\n\
         \r\n\
         test"
    );
    let raw = http_send(proxy.addr(), &request);

    assert_eq!(parse_status(&raw), 200);
    let headers = parse_body(&raw);
    assert!(
        !headers.contains("x-praxis-internal-leak"),
        "reserved x-praxis-* headers set during body phase must be stripped before upstream: {headers}"
    );
}

// -----------------------------------------------------------------------------
// Body-Phase Condition Tests (issue #1091)
// -----------------------------------------------------------------------------

#[test]
fn stream_buffer_body_phase_condition_sees_promoted_header() {
    let backend_guard = start_header_echo_backend();
    let proxy_port = free_port();

    let yaml = gated_recorder_yaml(proxy_port, backend_guard.port());
    let config = Config::from_yaml(&yaml).unwrap();
    let registry = registry_with_condition_recorder();
    let proxy = start_proxy_with_registry(&config, &registry);

    // `json_body_field` promotes model -> x-praxis-gate-model during pre-read;
    // the gated recorder's condition then matches, so its body hook runs.
    let payload = r#"{"model":"guarded"}"#;
    let raw = http_send(
        proxy.addr(),
        &format!(
            "POST / HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
            payload.len()
        ),
    );

    assert_eq!(parse_status(&raw), 200);
    let headers = parse_body(&raw);
    assert!(
        headers.contains("x-gated-ran: yes"),
        "gated body filter should run when the promoted header matches: {headers}"
    );
}

#[test]
fn stream_buffer_body_phase_condition_skips_without_promotion() {
    let backend_guard = start_header_echo_backend();
    let proxy_port = free_port();

    let yaml = gated_recorder_yaml(proxy_port, backend_guard.port());
    let config = Config::from_yaml(&yaml).unwrap();
    let registry = registry_with_condition_recorder();
    let proxy = start_proxy_with_registry(&config, &registry);

    // A different model promotes x-praxis-gate-model=other, which does not
    // match the gate, so the recorder's body hook is skipped.
    let payload = r#"{"model":"other"}"#;
    let raw = http_send(
        proxy.addr(),
        &format!(
            "POST / HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
            payload.len()
        ),
    );

    assert_eq!(parse_status(&raw), 200);
    let headers = parse_body(&raw);
    assert!(
        !headers.contains("x-gated-ran"),
        "gated body filter should be skipped when the promoted header does not match: {headers}"
    );
}

#[test]
fn stream_buffer_body_phase_condition_reserved_header_unspoofable() {
    let backend_guard = start_header_echo_backend();
    let proxy_port = free_port();

    let yaml = gated_recorder_yaml(proxy_port, backend_guard.port());
    let config = Config::from_yaml(&yaml).unwrap();
    let registry = registry_with_condition_recorder();
    let proxy = start_proxy_with_registry(&config, &registry);

    // A client cannot supply the reserved gate header to force the gate: the
    // ingress layer rejects client-supplied reserved x-praxis-* headers.
    let payload = r#"{"model":"other"}"#;
    let raw = http_send(
        proxy.addr(),
        &format!(
            "POST / HTTP/1.1\r\nHost: localhost\r\nx-praxis-gate-model: guarded\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
            payload.len()
        ),
    );

    assert_eq!(
        parse_status(&raw),
        400,
        "a client-supplied reserved gate header must be rejected at ingress"
    );
}

// -----------------------------------------------------------------------------
// Registries
// -----------------------------------------------------------------------------

fn registry_with_mutator() -> praxis_filter::FilterRegistry {
    let mut registry = praxis_filter::FilterRegistry::with_builtins();
    registry
        .register(
            "test_body_mutator",
            praxis_filter::FilterFactory::Http(std::sync::Arc::new(|_| {
                Ok(Box::new(BodyMutatingStreamBufferFilter::default_test()))
            })),
        )
        .expect("duplicate filter name");
    registry
}

fn registry_with_header_mutator() -> praxis_filter::FilterRegistry {
    let mut registry = praxis_filter::FilterRegistry::with_builtins();
    registry
        .register(
            "test_header_mutator",
            praxis_filter::FilterFactory::Http(std::sync::Arc::new(|_| {
                Ok(Box::new(praxis_test_utils::filters::HeaderMutatingStreamBufferFilter))
            })),
        )
        .expect("duplicate filter name");
    registry
}

fn registry_with_condition_recorder() -> praxis_filter::FilterRegistry {
    let mut registry = praxis_filter::FilterRegistry::with_builtins();
    registry
        .register(
            "condition_recorder",
            praxis_filter::FilterFactory::Http(std::sync::Arc::new(|_| {
                Ok(Box::new(ConditionRecordingStreamBufferFilter))
            })),
        )
        .expect("duplicate filter name");
    registry
}

// -----------------------------------------------------------------------------
// YAML Utilities
// -----------------------------------------------------------------------------

fn mutator_yaml(proxy_port: u16, backend_port: u16) -> String {
    format!(
        r#"
listeners:
  - name: default
    address: "127.0.0.1:{proxy_port}"
    filter_chains: [main]
filter_chains:
  - name: main
    filters:
      - filter: test_body_mutator
      - filter: load_balancer
        clusters:
          - name: "backend"
            endpoints:
              - "127.0.0.1:{backend_port}"
insecure_options:
  allow_private_endpoints: true
"#,
    )
}

fn gated_recorder_yaml(proxy_port: u16, backend_port: u16) -> String {
    format!(
        r#"
listeners:
  - name: default
    address: "127.0.0.1:{proxy_port}"
    filter_chains: [main]
filter_chains:
  - name: main
    filters:
      - filter: json_body_field
        field: model
        header: x-praxis-gate-model
      - filter: condition_recorder
        conditions:
          - when:
              headers:
                x-praxis-gate-model: guarded
      - filter: router
        routes:
          - path_prefix: "/"
            cluster: backend
      - filter: load_balancer
        clusters:
          - name: "backend"
            endpoints:
              - "127.0.0.1:{backend_port}"
insecure_options:
  allow_private_endpoints: true
"#,
    )
}

fn header_mutator_yaml(proxy_port: u16, backend_port: u16) -> String {
    format!(
        r#"
listeners:
  - name: default
    address: "127.0.0.1:{proxy_port}"
    filter_chains: [main]
filter_chains:
  - name: main
    filters:
      - filter: test_header_mutator
      - filter: router
        routes:
          - path_prefix: "/"
            cluster: backend
      - filter: load_balancer
        clusters:
          - name: "backend"
            endpoints:
              - "127.0.0.1:{backend_port}"
insecure_options:
  allow_private_endpoints: true
"#,
    )
}
