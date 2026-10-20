// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use super::*;
use test_r::test;

/// Round-trip the `SemanticTrapRetryOverrideMarker` through an
/// `anyhow::Error`: the marker must be discoverable via
/// `find_semantic_trap_retry_override`, and the inner error must still
/// be discoverable via the chain (because the marker exposes it as
/// `source()`). Also verifies that the carried verdict — including the
/// already-jittered delay and the policy state's attempt count — is
/// returned verbatim, with no re-rolling on the trap path.
#[test]
fn semantic_trap_retry_override_round_trips_through_anyhow_error() {
    // Use a non-round delay value to make accidental re-rolling
    // (which would clobber jitter) easy to spot.
    let jittered_delay = Duration::from_millis(1873);
    let payload = SemanticTrapRetryOverride {
        retry_from: OplogIndex::from_u64(17),
        policy_name: "manifest-5xx-retry".to_string(),
        verdict: SemanticTrapRetryVerdict::Retry(jittered_delay),
        retry_policy_state: RetryPolicyState::CountBox {
            attempts: 4,
            inner: Box::new(RetryPolicyState::Counter(4)),
        },
    };

    let inner = anyhow::Error::new(ClassifiedHostError {
        kind: HostFailureKind::Transient,
        message: "HTTP response status 500 matched user-defined retry policy".to_string(),
    });

    let with_marker = anyhow::Error::new(SemanticTrapRetryOverrideMarker {
        payload: payload.clone(),
        inner,
    });

    // (1) Marker is found verbatim.
    let extracted = find_semantic_trap_retry_override(&with_marker)
        .expect("override marker must round-trip through anyhow chain");
    assert_eq!(extracted.retry_from, payload.retry_from);
    assert_eq!(extracted.policy_name, payload.policy_name);
    assert_eq!(extracted.verdict, payload.verdict);
    assert_eq!(
        format!("{:?}", extracted.retry_policy_state),
        format!("{:?}", payload.retry_policy_state),
        "retry policy state must round-trip verbatim"
    );

    // (2) Wrapping in marker MUST NOT hide the inner classified error
    //     — `TrapType::from_error` walks the same chain to classify
    //     the failure (transient vs. permanent), so this lookup must
    //     keep working.
    let inner_classified = with_marker
        .chain()
        .find_map(|e| e.downcast_ref::<ClassifiedHostError>())
        .expect("inner ClassifiedHostError must still be reachable via chain()");
    assert_eq!(inner_classified.kind, HostFailureKind::Transient);
    assert_eq!(
        inner_classified.message,
        "HTTP response status 500 matched user-defined retry policy"
    );
}

/// Adding an `anyhow::Error::context(...)` wrapper on top of the
/// marker (e.g. by an error-handling layer between the host call and
/// the trap path) must NOT hide the marker from
/// `find_semantic_trap_retry_override`. Walking `.chain()` and
/// downcasting on each `&dyn Error` must still find it.
#[test]
fn semantic_trap_retry_override_survives_anyhow_context_wrapper() {
    let payload = SemanticTrapRetryOverride {
        retry_from: OplogIndex::from_u64(23),
        policy_name: "manifest-5xx-retry".to_string(),
        verdict: SemanticTrapRetryVerdict::Retry(Duration::from_secs(2)),
        retry_policy_state: RetryPolicyState::CountBox {
            attempts: 1,
            inner: Box::new(RetryPolicyState::Counter(1)),
        },
    };

    let inner = anyhow::Error::new(ClassifiedHostError {
        kind: HostFailureKind::Transient,
        message: "boom".to_string(),
    });

    let with_marker = anyhow::Error::new(SemanticTrapRetryOverrideMarker {
        payload: payload.clone(),
        inner,
    });

    // Add an arbitrary context layer on top.
    let wrapped = with_marker.context("escalated through outer layer");

    let extracted = find_semantic_trap_retry_override(&wrapped)
        .expect("override marker must survive an outer context wrapper");
    assert_eq!(extracted.retry_from, payload.retry_from);
    assert_eq!(extracted.policy_name, payload.policy_name);
    assert_eq!(extracted.verdict, payload.verdict);
}

/// `find_semantic_trap_retry_override` must return `None` for a plain
/// failure that has no marker attached — this is the "guest-originated
/// trap" path where the trap recovery must fall through to the
/// rich-context resolver and not silently honour a stale override.
#[test]
fn find_semantic_trap_retry_override_returns_none_when_marker_absent() {
    let bare = anyhow::Error::new(ClassifiedHostError {
        kind: HostFailureKind::Transient,
        message: "generic failure".to_string(),
    });
    assert!(find_semantic_trap_retry_override(&bare).is_none());

    let with_context = bare.context("escalated to trap");
    assert!(find_semantic_trap_retry_override(&with_context).is_none());
}

/// Every terminal-step conversion (`complete` / `complete_access` / `cancel` / the dropped-call
/// drain) funnels through [`TerminalCallError::into_marked_anyhow`]. That single funnel must
/// preserve *both* halves of the carrier: the call-owned [`DurableCallTrapContext`] (so
/// `TrapType::from_error` groups the failure against the call's own scope) *and* the original
/// `WorkerExecutorError` as the reachable root cause (so the same path can still downcast it to
/// classify e.g. `ReadOnlyViolation` / unexpected-oplog-entry failures).
#[test]
fn terminal_call_error_preserves_marker_and_inner_worker_executor_error() {
    let context = DurableCallTrapContext {
        retry_from: OplogIndex::from_u64(17),
    };
    let terminal = TerminalCallError::new(
        WorkerExecutorError::ReadOnlyViolation {
            method: "agent::read-only-method".to_string(),
            host_function: "test:host-function".to_string(),
        },
        context,
    );

    let error = terminal.into_marked_anyhow();

    // (1) The call-owned marker is recoverable verbatim.
    let marker = find_durable_call_trap_context(&error)
        .expect("terminal failure must carry its call-owned trap context");
    assert_eq!(marker, context);

    // (2) The original WorkerExecutorError is still the root cause and downcastable from the
    //     chain, so classification on the trap path keeps working.
    let inner = error
        .root_cause()
        .downcast_ref::<WorkerExecutorError>()
        .expect("inner WorkerExecutorError must remain reachable as the root cause");
    match inner {
        WorkerExecutorError::ReadOnlyViolation {
            method,
            host_function,
        } => {
            assert_eq!(method, "agent::read-only-method");
            assert_eq!(host_function, "test:host-function");
        }
        other => panic!("expected ReadOnlyViolation root cause, got {other:?}"),
    }
}

/// Terminal-step failures don't always reach the invocation loop as an `anyhow::Error` directly:
/// the stream paths in `io/streams.rs` carry them as `wasmtime_wasi::p2::StreamError::Trap`, and
/// other host paths as `wasmtime::Error`, before the loop converts back to `anyhow::Error` for
/// `TrapType::from_error`. Those explicit `From<TerminalCallError>` funnels must preserve the
/// call-owned marker across the round trip, otherwise the marker would silently vanish on exactly
/// those surfaces and the classifier would fall back to ambient state.
#[test]
fn terminal_call_error_marker_survives_wasmtime_and_stream_conversions() {
    let context = DurableCallTrapContext {
        retry_from: OplogIndex::from_u64(23),
    };

    // wasmtime::Error funnel -> back to anyhow (as the invocation loop does).
    let via_wasmtime: wasmtime::Error =
        TerminalCallError::new(WorkerExecutorError::runtime("boom"), context).into();
    let back: anyhow::Error = via_wasmtime.into();
    assert_eq!(
        find_durable_call_trap_context(&back),
        Some(context),
        "marker must survive the wasmtime::Error round trip"
    );

    // StreamError::Trap funnel -> unwrap the trap -> back to anyhow.
    let via_stream: wasmtime_wasi::p2::StreamError =
        TerminalCallError::new(WorkerExecutorError::runtime("boom"), context).into();
    match via_stream {
        wasmtime_wasi::p2::StreamError::Trap(trap) => {
            let back: anyhow::Error = trap.into();
            assert_eq!(
                find_durable_call_trap_context(&back),
                Some(context),
                "marker must survive the StreamError::Trap round trip"
            );
        }
        other => panic!("expected StreamError::Trap, got {other:?}"),
    }
}
