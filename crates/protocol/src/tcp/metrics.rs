// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 Praxis Contributors

//! Prometheus metrics for TCP connection lifecycle.

use metrics::{SharedString, counter, gauge, histogram};

use crate::http::pingora::metrics::is_recorder_installed;

// -----------------------------------------------------------------------------
// Constants
// -----------------------------------------------------------------------------

/// Counter for total accepted TCP connections.
const TCP_CONNECTIONS_TOTAL: &str = "praxis_tcp_connections_total";

/// Histogram for TCP connection duration in seconds.
const TCP_CONNECTION_DURATION_SECONDS: &str = "praxis_tcp_connection_duration_seconds";

/// Gauge for currently open TCP connections per listener.
///
/// Unlike `praxis_connections_active`, which counts HTTP requests and TCP
/// sessions under one name, this series carries only TCP connections, so
/// the TCP metric family is queryable on its own.
const TCP_ACTIVE_CONNECTIONS: &str = "praxis_tcp_active_connections";

// -----------------------------------------------------------------------------
// Metric Recording
// -----------------------------------------------------------------------------

/// Increment the total TCP connections counter for the given listener.
///
/// No-op when the Prometheus recorder has not been installed
/// (i.e. when the admin interface is disabled).
pub(crate) fn record_tcp_connection_accepted(listener: SharedString) {
    if !is_recorder_installed() {
        return;
    }
    counter!(TCP_CONNECTIONS_TOTAL, "listener" => listener).increment(1);
}

/// Record TCP connection duration for a closed connection.
///
/// The `reason` label captures the disconnect cause. Sessions that reached
/// the forwarding phase report the `TcpCloseReason` they ended on
/// (`completed`, `error`, `shutdown`, `session_timeout`, `max_duration`);
/// early closes report `sni_timeout`, `filter_rejection`, `connect_failure`
/// or `peeked_write_error`.
///
/// No-op when the Prometheus recorder has not been installed
/// (i.e. when the admin interface is disabled).
pub(crate) fn record_tcp_connection_duration(listener: SharedString, reason: &'static str, duration_secs: f64) {
    if !is_recorder_installed() {
        return;
    }
    histogram!(
        TCP_CONNECTION_DURATION_SECONDS,
        "listener" => listener,
        "reason" => reason
    )
    .record(duration_secs);
}

/// RAII guard that decrements `praxis_tcp_active_connections` on drop.
///
/// Acquired once per accepted TCP connection, alongside
/// `ActiveConnectionGuard`. Every early-close path returns from the same
/// session future, so the drop covers SNI timeouts, filter rejections and
/// connect failures as well as completed sessions.
///
/// Bind the guard to a named variable: `let _ = acquire(..)` drops it
/// immediately and pins the gauge at zero.
pub(crate) struct TcpActiveConnectionGuard {
    /// Listener name label.
    listener: SharedString,
}

impl TcpActiveConnectionGuard {
    /// Increment the gauge and return a guard that decrements on drop.
    pub(crate) fn acquire(listener: SharedString) -> Self {
        if is_recorder_installed() {
            gauge!(TCP_ACTIVE_CONNECTIONS, "listener" => listener.clone()).increment(1.0);
        }
        Self { listener }
    }
}

impl Drop for TcpActiveConnectionGuard {
    fn drop(&mut self) {
        if is_recorder_installed() {
            gauge!(TCP_ACTIVE_CONNECTIONS, "listener" => self.listener.clone()).decrement(1.0);
        }
    }
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
#[expect(clippy::allow_attributes, reason = "blanket test suppressions")]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, reason = "tests")]
mod tests {
    use super::*;

    #[test]
    fn record_accepted_without_recorder_does_not_panic() {
        record_tcp_connection_accepted(SharedString::const_str("test-listener"));
    }

    #[test]
    fn record_duration_without_recorder_does_not_panic() {
        record_tcp_connection_duration(SharedString::const_str("test-listener"), "completed", 1.5);
    }

    #[test]
    fn record_zero_duration_does_not_panic() {
        record_tcp_connection_duration(SharedString::const_str("test-listener"), "sni_timeout", 0.0);
    }

    #[test]
    fn record_large_duration_does_not_panic() {
        record_tcp_connection_duration(SharedString::const_str("long-lived"), "completed", 86400.0);
    }

    #[test]
    fn active_connection_guard_without_recorder_does_not_panic() {
        let _guard = TcpActiveConnectionGuard::acquire(SharedString::const_str("test-listener"));
    }

    #[test]
    fn active_connection_guard_returns_to_zero_on_drop() {
        crate::http::pingora::metrics::install_prometheus_recorder();
        let guard = TcpActiveConnectionGuard::acquire(SharedString::const_str("tcp-gauge-listener"));
        let held = crate::http::pingora::metrics::render_prometheus().expect("recorder should render");
        assert!(
            held.contains("praxis_tcp_active_connections{listener=\"tcp-gauge-listener\"} 1"),
            "gauge should read 1 while the guard is held:\n{held}"
        );
        drop(guard);
        let released = crate::http::pingora::metrics::render_prometheus().expect("recorder should render");
        assert!(
            released.contains("praxis_tcp_active_connections{listener=\"tcp-gauge-listener\"} 0"),
            "gauge should return to 0 once the guard drops:\n{released}"
        );
    }

    #[test]
    fn forwarding_phase_reasons_appear_in_scrape() {
        crate::http::pingora::metrics::install_prometheus_recorder();
        for reason in ["error", "shutdown", "session_timeout", "max_duration"] {
            record_tcp_connection_duration(SharedString::const_str("reason-listener"), reason, 0.5);
        }
        let body = crate::http::pingora::metrics::render_prometheus().expect("recorder should render");
        for reason in ["error", "shutdown", "session_timeout", "max_duration"] {
            let needle = format!("reason=\"{reason}\"");
            assert!(
                body.contains(&needle),
                "expected `{needle}` in scrape; forwarding-phase close reasons must not collapse to `completed`:\n{body}"
            );
        }
    }
}
