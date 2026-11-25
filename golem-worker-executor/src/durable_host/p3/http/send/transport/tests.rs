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
use crate::durable_host::DurableHttpHooks;
use crate::durable_host::p3::http::test_support::FrameTestOplog;
use http_body_util::Empty;
use test_r::{test, timeout};

#[test]
#[timeout("10s")]
async fn physical_attempt_sends_prepared_headers_and_maps_io_errors() {
    let physical = PhysicalSendRequest::<crate::workerctx::default::Context> {
        pool: None,
        hooks: DurableHttpHooks {
            connection_pool: None,
            is_replay: Arc::new(AtomicBool::new(false)),
        },
        method: http::Method::POST,
        uri: http::Uri::from_static("http://example.com/charge"),
        version: http::Version::HTTP_11,
        headers: HeaderMap::from_iter([
            (
                http::HeaderName::from_static("idempotency-key"),
                http::HeaderValue::from_static("order-42-charge"),
            ),
            (
                http::HeaderName::from_static("traceparent"),
                http::HeaderValue::from_static("trace-value"),
            ),
        ]),
        body: DurableRequestBody::new(
            Empty::new().map_err(|never| match never {}).boxed_unsync(),
            FrameTestOplog::new(),
            OplogIndex::from_u64(41),
            true,
            Default::default(),
        ),
        options: None,
        final_transmission_tx: oneshot::channel().0,
        _phantom: PhantomData,
    };
    let (response, io, connection) = physical
        .attempt(|request, options| async move {
            assert!(options.is_none());
            assert_eq!(request.method(), http::Method::POST);
            assert_eq!(request.uri(), "http://example.com/charge");
            assert_eq!(request.headers()["idempotency-key"], "order-42-charge");
            assert_eq!(
                request.headers().get_all("idempotency-key").iter().count(),
                1
            );
            assert_eq!(request.headers()["traceparent"], "trace-value");
            assert!(
                request
                    .into_body()
                    .collect()
                    .await
                    .unwrap()
                    .to_bytes()
                    .is_empty()
            );
            let io: Box<dyn Future<Output = Result<(), wasmtime_wasi_http::Error>> + Send> =
                Box::new(async { Err(ErrorCode::ConnectionReadTimeout.into()) });
            Ok((
                http::Response::new(Empty::new().map_err(|never| match never {}).boxed_unsync()),
                io,
                None,
            ))
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
}
