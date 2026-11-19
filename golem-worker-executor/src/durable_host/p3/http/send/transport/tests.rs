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
use crate::durable_host::durability::DurableExecutionState;
use crate::durable_host::p3::http::test_support::FrameTestOplog;
use crate::durable_host::{DurableHttpHooks, IdempotencyKeyPosition};
use crate::services::oplog::{CommitLevel, Oplog, OplogError};
use async_trait::async_trait;
use golem_common::model::oplog::{AgentError, OplogErrorKind};
use golem_common::model::{Predicate, RetryPolicy, RetryPolicyState, Timestamp};
use http::HeaderValue;
use http_body_util::Empty;
use std::time::Duration;
use test_r::{test, timeout};
use wasmtime_wasi_http::FieldMap;

struct RetryRecorder {
    oplog: Arc<FrameTestOplog>,
    start: OplogIndex,
}

#[async_trait]
impl InFunctionRetryHost for RetryRecorder {
    fn current_retry_point(&self) -> OplogIndex {
        self.start
    }

    async fn named_retry_policies(&mut self) -> Vec<NamedRetryPolicy> {
        vec![NamedRetryPolicy {
            name: "transport-retry".to_string(),
            priority: 0,
            predicate: Predicate::True,
            policy: RetryPolicy::CountBox {
                max_retries: 1,
                inner: Box::new(RetryPolicy::Immediate),
            },
        }]
    }

    async fn current_retry_state_for(&self, retry_from: OplogIndex) -> Option<RetryPolicyState> {
        self.oplog
            .entries()
            .into_iter()
            .rev()
            .find_map(|entry| match entry {
                OplogEntry::Error {
                    retry_from: recorded,
                    retry_policy_state,
                    ..
                } if recorded == retry_from => retry_policy_state,
                _ => None,
            })
    }

    fn durable_execution_state(&self) -> DurableExecutionState {
        DurableExecutionState {
            is_live: true,
            snapshotting_mode: false,
            max_in_function_retry_delay: Duration::from_secs(1),
            durable_wait: false,
        }
    }

    async fn append_retry_error_entry(
        &mut self,
        retry_from: OplogIndex,
        retry_policy_state: Option<RetryPolicyState>,
        retry_at: Timestamp,
    ) -> Result<(), OplogError> {
        self.oplog
            .add(OplogEntry::error(
                None,
                OplogErrorKind::Invocation,
                None,
                AgentError::TransientError("in-function retry".to_string()),
                retry_from,
                retry_policy_state,
                Some(retry_at),
                Some(self.start),
            ))
            .await?;
        self.oplog.commit(CommitLevel::Always).await?;
        Ok(())
    }
}

async fn assert_key_on_retried_physical_request(
    logical: Option<OplogIndex>,
    guest_key: Option<&'static str>,
    expected: &str,
) {
    let start = OplogIndex::from_u64(41);
    let position = IdempotencyKeyPosition {
        parent: Uuid::parse_str("177db03d-3234-4a04-8d03-e8d042348abd")
            .unwrap()
            .into(),
        logical,
    };
    let mut derivations = 0;
    let mut hooks = DurableHttpHooks {
        connection_pool: None,
        is_replay: Arc::new(AtomicBool::new(false)),
    };
    let mut headers = HeaderMap::new();
    if let Some(key) = guest_key {
        headers.insert("idempotency-key", HeaderValue::from_static(key));
    }
    let managed = golem_managed_http_headers(
        vec![("traceparent".to_string(), "trace-value".to_string())],
        true,
        headers.contains_key("idempotency-key"),
        start,
    )
    .resolve(|index| {
        derivations += 1;
        position.derive(index)
    });
    let mut headers = FieldMap::new_immutable(&mut hooks, headers);
    apply_managed_http_headers(&mut headers, &mut hooks, 4096, &managed).unwrap();
    let oplog = FrameTestOplog::new();
    let physical = PhysicalSendRequest::<crate::workerctx::default::Context> {
        pool: None,
        hooks,
        method: http::Method::POST,
        uri: http::Uri::from_static("http://example.com/charge"),
        version: http::Version::HTTP_11,
        headers: headers.into(),
        body: DurableRequestBody::new(
            Empty::new().map_err(|never| match never {}).boxed_unsync(),
            oplog.clone(),
            start,
            true,
            Default::default(),
        ),
        options: None,
        final_transmission_tx: oneshot::channel().0,
        _phantom: PhantomData,
    };
    let mut dispatched = Vec::new();
    let first = physical
        .attempt(|request, options| {
            dispatched.push(request.headers().clone());
            async move {
                assert!(options.is_none());
                assert!(
                    request
                        .into_body()
                        .collect()
                        .await
                        .unwrap()
                        .to_bytes()
                        .is_empty()
                );
                Err(wasmtime_wasi_http::Error::from(
                    ErrorCode::ConnectionTerminated,
                ))
            }
        })
        .await;
    let Err(error) = first else {
        panic!("the first dispatch must fail");
    };
    assert!(matches!(error, ErrorCode::ConnectionTerminated));
    assert_eq!(
        classify_serializable_http_error_code(&serialize_error_code(&error)),
        HostFailureKind::Transient
    );
    let mut retry_state = InFunctionRetryState::new();
    let mut retry_host = RetryRecorder { oplog, start };
    let decision = retry_state
        .decide_retry_with_properties(
            &mut retry_host,
            "http::client::send",
            &RetryContext::http_with_response(
                "POST",
                "http://example.com/charge",
                None,
                "transient",
            ),
        )
        .await;
    assert!(matches!(
        decision,
        AsyncRetryDecision::RetryAfterDelay(Duration::ZERO)
    ));
    assert_eq!(retry_state.retry_count(), 1);
    assert!(retry_host.oplog.entries().iter().any(|entry| matches!(
        entry,
        OplogEntry::Error { retrying_start_index: Some(index), .. } if *index == start
    )));
    let (response, io, connection) = physical
        .attempt(|request, options| {
            dispatched.push(request.headers().clone());
            async move {
                assert!(options.is_none());
                assert!(
                    request
                        .into_body()
                        .collect()
                        .await
                        .unwrap()
                        .to_bytes()
                        .is_empty()
                );
                let response = http::Response::new(
                    Empty::new().map_err(|never| match never {}).boxed_unsync(),
                );
                let io: Box<dyn Future<Output = Result<(), wasmtime_wasi_http::Error>> + Send> =
                    Box::new(async { Err(ErrorCode::ConnectionReadTimeout.into()) });
                Ok((response, io, None))
            }
        })
        .await
        .unwrap();
    assert!(connection.is_none());
    assert!(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .is_empty()
    );
    assert!(matches!(
        Box::into_pin(io).await,
        Err(ErrorCode::ConnectionReadTimeout)
    ));
    assert_eq!(dispatched.len(), 2);
    dispatched.iter().for_each(|headers| {
        assert_eq!(headers["idempotency-key"], expected);
        assert_eq!(headers.get_all("idempotency-key").iter().count(), 1);
        assert_eq!(headers["traceparent"], "trace-value");
    });
    assert_eq!(derivations, usize::from(guest_key.is_none()));
}

#[test]
#[timeout("10s")]
async fn physical_attempt_retry_reuses_one_derived_key() {
    // UUIDv5 of the parent UUID and the literal UTF-8 names oplog-index-41 and oplog-index-17.
    assert_key_on_retried_physical_request(None, None, "c60894d5-972e-51e8-bfa3-9a78884b55ca")
        .await;
    assert_key_on_retried_physical_request(
        Some(OplogIndex::from_u64(17)),
        None,
        "8169f767-c476-5171-b4a3-f295e3ec9373",
    )
    .await;
}

#[test]
#[timeout("10s")]
async fn physical_attempt_retry_preserves_guest_key_without_derivation() {
    assert_key_on_retried_physical_request(None, Some("order-42-charge"), "order-42-charge").await;
}
