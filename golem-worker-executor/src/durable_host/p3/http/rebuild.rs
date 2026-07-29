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

use super::request_body::{
    DurableRequestBody, DurableRequestBodyDrainOutcome, RecordedRequestBodyTerminal,
    recorded_request_body_replay, scan_recorded_request_body_frames,
};
use crate::durable_host::p3::{DurableP3, durable_worker_ctx, wasi_http_view};
use crate::workerctx::WorkerCtx;
use bytes::Bytes;
use golem_common::model::oplog::OplogIndex;
use golem_common::model::oplog::payload::types::{
    SerializableHttpMethod, SerializableP3HttpClientSend, SerializableP3HttpScheme,
};
use http::{HeaderMap, HeaderName, HeaderValue};
use http_body_util::BodyExt as _;
use http_body_util::combinators::UnsyncBoxBody;
use tracing::debug;
use wasmtime::AsContextMut;
use wasmtime::component::Accessor;
use wasmtime_wasi_http::FieldMap;
use wasmtime_wasi_http::p3::bindings::http::types::ErrorCode;

pub(crate) struct P3HttpSendRebuild {
    pub(super) request: SerializableP3HttpClientSend,
    pub(super) injected_headers: Vec<(String, String)>,
    /// The recorded response status, used only to log divergence of the fresh
    /// response's status; the recorded head stays authoritative for the guest.
    pub(super) recorded_status: u16,
    /// The send's `Start` index — the `parent_start_index` key of its recorded request-body frames.
    pub(super) recorded_request_body: OplogIndex,
    /// The live send's in-memory request-body recorder, present only in the
    /// session that performed the send (a replayed response carries `None`).
    /// A resend in the same session drains and replays through this handle,
    /// which also covers a recording whose terminal frame has not landed in
    /// the oplog yet; after a restart the body is reconstructed from the
    /// recorded frames instead.
    pub(super) durable_body: Option<DurableRequestBody>,
}

/// Aborts the spawned I/O task of a re-issued request when dropped, bounding
/// its lifetime to the consume-body task that reads the rebuilt body
/// (mirroring the abort-on-drop handle the built-in `WasiHttp::send` attaches
/// to live response bodies).
pub(super) struct AbortOnDropIoTask(pub(super) tokio::task::JoinHandle<()>);

impl Drop for AbortOnDropIoTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Result of attempting to re-issue a recorded send for an incomplete
/// consume-body scope.
pub(super) enum RebuildOutcome {
    /// The request was re-issued: stream the fresh response body. The recorded
    /// response head stays authoritative for the guest — the rebuild only
    /// supplies a replacement body stream.
    Rebuilt {
        body: UnsyncBoxBody<Bytes, ErrorCode>,
        io_guard: AbortOnDropIoTask,
    },
    /// The re-issue failed on conversion or on the network: surfaced as a
    /// body-transfer error and classified for worker-level retry like any live
    /// body failure.
    Failed(ErrorCode),
    /// The recorded request body cannot be reconstructed — its recorded terminal is a guest body
    /// error or its frames cannot be read back. Fail the body transfer loud with a permanent error.
    Refused(String),
}

/// Result of re-sending a recorded request, keeping the full fresh response
/// head available for the caller to classify (used by the response-body
/// resume path, which must inspect the fresh status and `Content-Range`).
pub(super) enum ResendOutcome {
    Sent {
        response: http::Response<UnsyncBoxBody<Bytes, ErrorCode>>,
        io_guard: AbortOnDropIoTask,
    },
    /// The re-send failed on conversion or on the network.
    Failed(ErrorCode),
    /// The recorded request body cannot be reconstructed; the reason describes
    /// why (without any caller-specific context prefix).
    Refused(String),
}

/// Reconstructs the p3 request resource-equivalent from the recorded head:
/// method, scheme, authority, path, the guest-set headers plus the re-derived
/// Golem-managed headers and the caller-supplied extra headers (both replacing
/// same-name guest values, as the live injection does), the recorded
/// per-request timeout options, and the given body (empty for bodiless sends,
/// or the recorded body streamed from the oplog).
pub(super) fn build_rebuilt_request(
    rebuild: &P3HttpSendRebuild,
    extra_headers: &[(String, String)],
    body: UnsyncBoxBody<Bytes, ErrorCode>,
) -> Result<wasmtime_wasi_http::p3::Request, String> {
    let method = deserialize_http_method(&rebuild.request.method)?;
    let scheme = rebuild
        .request
        .scheme
        .as_ref()
        .map(deserialize_uri_scheme)
        .transpose()?;
    let authority = rebuild
        .request
        .authority
        .as_deref()
        .map(|authority| {
            http::uri::Authority::try_from(authority)
                .map_err(|err| format!("invalid recorded request authority {authority}: {err}"))
        })
        .transpose()?;
    let path_with_query = rebuild
        .request
        .path_with_query
        .as_deref()
        .map(|path| {
            http::uri::PathAndQuery::try_from(path)
                .map_err(|err| format!("invalid recorded request path {path}: {err}"))
        })
        .transpose()?;

    let mut headers = HeaderMap::new();
    for (name, values) in &rebuild.request.headers {
        let name = HeaderName::try_from(name.as_str())
            .map_err(|err| format!("invalid recorded request header name {name}: {err}"))?;
        for value in values {
            let value = HeaderValue::try_from(value.clone())
                .map_err(|err| format!("invalid recorded request header value: {err}"))?;
            headers.append(name.clone(), value);
        }
    }
    for (name, value) in rebuild.injected_headers.iter().chain(extra_headers) {
        let name = HeaderName::try_from(name.as_str())
            .map_err(|err| format!("invalid injected header name {name}: {err}"))?;
        let value = HeaderValue::try_from(value.as_str())
            .map_err(|err| format!("invalid injected header value for {name}: {err}"))?;
        headers.remove(&name);
        headers.append(name, value);
    }

    let options = rebuild.request.options.as_ref().map(|options| {
        std::sync::Arc::new(wasmtime_wasi_http::p3::RequestOptions {
            connect_timeout: options
                .connect_timeout_nanos
                .map(std::time::Duration::from_nanos),
            first_byte_timeout: options
                .first_byte_timeout_nanos
                .map(std::time::Duration::from_nanos),
            between_bytes_timeout: options
                .between_bytes_timeout_nanos
                .map(std::time::Duration::from_nanos),
        })
    });

    let (request, _transmission) = wasmtime_wasi_http::p3::Request::new(
        method,
        scheme,
        authority,
        path_with_query,
        FieldMap::new_immutable(headers),
        options,
        body,
    );
    Ok(request)
}

pub(super) fn deserialize_http_method(
    method: &SerializableHttpMethod,
) -> Result<http::Method, String> {
    match method {
        SerializableHttpMethod::Get => Ok(http::Method::GET),
        SerializableHttpMethod::Post => Ok(http::Method::POST),
        SerializableHttpMethod::Put => Ok(http::Method::PUT),
        SerializableHttpMethod::Delete => Ok(http::Method::DELETE),
        SerializableHttpMethod::Head => Ok(http::Method::HEAD),
        SerializableHttpMethod::Connect => Ok(http::Method::CONNECT),
        SerializableHttpMethod::Options => Ok(http::Method::OPTIONS),
        SerializableHttpMethod::Trace => Ok(http::Method::TRACE),
        SerializableHttpMethod::Patch => Ok(http::Method::PATCH),
        SerializableHttpMethod::Other(other) => http::Method::from_bytes(other.as_bytes())
            .map_err(|err| format!("invalid recorded HTTP method {other}: {err}")),
    }
}

pub(super) fn deserialize_uri_scheme(
    scheme: &SerializableP3HttpScheme,
) -> Result<http::uri::Scheme, String> {
    match scheme {
        SerializableP3HttpScheme::Http => Ok(http::uri::Scheme::HTTP),
        SerializableP3HttpScheme::Https => Ok(http::uri::Scheme::HTTPS),
        SerializableP3HttpScheme::Other(other) => other
            .parse()
            .map_err(|err| format!("invalid recorded request scheme {other}: {err}")),
    }
}

/// Re-issues a recorded send whose durable consume-body scope must re-execute
/// live after a restart — the P3 counterpart of P2's
/// `rebuild_request_after_replay`.
///
/// The re-issue is *recovery* of the already-recorded send, not a new
/// guest-visible call: it writes no oplog entries, does not count against HTTP
/// call limits, and starts no new span. It reuses the built-in request
/// conversion (host-header injection, default scheme) and the same connection
/// pool as the original live send. The fresh response head is discarded — the
/// recorded head, already delivered to the guest, stays authoritative; only
/// the body stream is taken.
pub(super) async fn reissue_recorded_request<Ctx: WorkerCtx, U: 'static>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
    rebuild: &P3HttpSendRebuild,
) -> RebuildOutcome {
    match resend_recorded_request(accessor, rebuild, &[]).await {
        ResendOutcome::Sent { response, io_guard } => {
            if response.status().as_u16() != rebuild.recorded_status {
                debug!(
                    recorded_status = rebuild.recorded_status,
                    fresh_status = %response.status(),
                    "re-issued p3 HTTP request returned a different status than the recorded \
                     response; the recorded head stays authoritative"
                );
            }
            RebuildOutcome::Rebuilt {
                body: response.into_body(),
                io_guard,
            }
        }
        ResendOutcome::Failed(code) => RebuildOutcome::Failed(code),
        ResendOutcome::Refused(reason) => RebuildOutcome::Refused(format!(
            "cannot rebuild the in-flight p3 HTTP send after a restart: {reason}"
        )),
    }
}

/// Re-sends a recorded request, optionally with extra injected headers (e.g. a
/// `Range` header for response-body resume), returning the full fresh
/// response so the caller can classify its status and headers. Like the
/// restart re-issue, this is recovery of the already-recorded send: it writes
/// no oplog entries, does not count against HTTP call limits, and starts no
/// new span.
pub(super) async fn resend_recorded_request<Ctx: WorkerCtx, U: 'static>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
    rebuild: &P3HttpSendRebuild,
    extra_headers: &[(String, String)],
) -> ResendOutcome {
    let body = if let Some(durable_body) = &rebuild.durable_body {
        // Same-session resend: drain the guest body to its terminal through
        // the live recorder (completing a recording whose terminal frame has
        // not landed yet) and stream the replay view. The previous attempt's
        // live view may still be registered if its connection was torn down
        // without dropping the body — abandon it so the drain can proceed.
        durable_body.abandon_active_live_view();
        match durable_body.drain_to_terminal().await {
            DurableRequestBodyDrainOutcome::Replayable => durable_body.replayer().boxed_unsync(),
            DurableRequestBodyDrainOutcome::NotReplayable => {
                return ResendOutcome::Refused(
                    "the request body could not be replayed (guest body error or a \
                     frame-recording failure)"
                        .to_string(),
                );
            }
        }
    } else {
        let send_start_index = rebuild.recorded_request_body;
        let oplog = accessor.with(|mut access| {
            let ctx = durable_worker_ctx::<Ctx, U>(access.data_mut());
            ctx.state.oplog.clone()
        });
        let scan = match scan_recorded_request_body_frames(oplog.clone(), send_start_index).await {
            Ok(scan) => scan,
            Err(error) => {
                return ResendOutcome::Refused(format!(
                    "the recorded request-body frames could not be read back: {error}"
                ));
            }
        };
        match scan.terminal {
            Some(RecordedRequestBodyTerminal::End) => recorded_request_body_replay(oplog, &scan),
            Some(RecordedRequestBodyTerminal::Error(error)) => {
                return ResendOutcome::Refused(format!(
                    "the recorded request body ended with a guest body error: {error:?}"
                ));
            }
            None => {
                return ResendOutcome::Refused(
                    "the recorded request body is incomplete (no terminal frame was recorded)"
                        .to_string(),
                );
            }
        }
    };

    let request = match build_rebuilt_request(rebuild, extra_headers, body) {
        Ok(request) => request,
        Err(message) => return ResendOutcome::Failed(ErrorCode::InternalError(Some(message))),
    };

    let converted = accessor.with(|mut access| {
        let pool = {
            let ctx = durable_worker_ctx::<Ctx, U>(access.data_mut());
            ctx.wasi_http.connection_pool.clone()
        };
        let converted = request.into_http_with_getter(
            access.as_context_mut(),
            async { Ok(()) },
            wasi_http_view::<Ctx, U>,
        );
        (pool, converted)
    });
    let (pool, converted) = converted;
    let (http_request, options) = match converted {
        Ok(converted) => converted,
        Err(error) => {
            return match error.downcast_ref() {
                Some(code) => ResendOutcome::Failed(code.clone()),
                None => ResendOutcome::Failed(ErrorCode::InternalError(Some(format!(
                    "failed to convert the rebuilt p3 HTTP request: {error:?}"
                )))),
            };
        }
    };
    let options = options.as_deref().copied();

    let sent = match pool {
        Some(pool) => pool
            .pooled_send_request_p3(http_request, options)
            .await
            .map(|(response, io, _pooled_connection)| (response, io)),
        None => match wasmtime_wasi_http::p3::default_send_request(http_request, options).await {
            Ok((response, io)) => Ok((
                response.map(http_body_util::BodyExt::boxed_unsync),
                Box::new(io) as Box<dyn std::future::Future<Output = Result<(), ErrorCode>> + Send>,
            )),
            Err(error) => Err(error),
        },
    };
    match sent {
        Ok((response, io)) => {
            let io = Box::into_pin(io);
            let io_task = tokio::task::spawn(async move {
                let result = io.await;
                debug!(?result, "re-issued p3 HTTP request I/O future finished");
            });
            ResendOutcome::Sent {
                response,
                io_guard: AbortOnDropIoTask(io_task),
            }
        }
        Err(code) => ResendOutcome::Failed(code),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::oplog::payload::types::*;
    use http_body_util::Empty;
    use std::collections::HashMap;
    use test_r::test;

    fn rebuild_head(
        method: SerializableHttpMethod,
        headers: HashMap<String, Vec<Vec<u8>>>,
    ) -> SerializableP3HttpClientSend {
        SerializableP3HttpClientSend {
            method,
            scheme: Some(SerializableP3HttpScheme::Http),
            authority: Some("localhost:1234".to_string()),
            path_with_query: Some("/stream?x=1".to_string()),
            headers,
            options: None,
        }
    }

    /// The rebuilt request must carry the recorded head exactly — method,
    /// scheme, authority, path, guest headers — plus the re-derived
    /// Golem-managed headers replacing same-name guest values (as the live
    /// injection does), the recorded per-request timeout options, and an empty
    /// body.
    #[test]
    fn rebuilt_request_matches_recorded_head() {
        let mut head = rebuild_head(
            SerializableHttpMethod::Get,
            HashMap::from([
                ("x-test".to_string(), vec![b"guest".to_vec()]),
                ("idempotency-key".to_string(), vec![b"stale".to_vec()]),
            ]),
        );
        head.options = Some(SerializableP3HttpRequestOptions {
            connect_timeout_nanos: Some(1_000_000_000),
            first_byte_timeout_nanos: None,
            between_bytes_timeout_nanos: Some(2_000_000_000),
        });
        let rebuild = P3HttpSendRebuild {
            request: head,
            injected_headers: vec![
                ("idempotency-key".to_string(), "derived-key".to_string()),
                ("traceparent".to_string(), "00-abc-def-01".to_string()),
            ],
            recorded_status: 200,
            recorded_request_body: OplogIndex::INITIAL,
            durable_body: None,
        };

        let request = build_rebuilt_request(
            &rebuild,
            &[("range".to_string(), "bytes=100-".to_string())],
            Empty::<Bytes>::new()
                .map_err(|never| match never {})
                .boxed_unsync(),
        )
        .expect("rebuild request should build");

        assert_eq!(request.method, http::Method::GET);
        assert_eq!(request.scheme, Some(http::uri::Scheme::HTTP));
        assert_eq!(
            request.authority.as_ref().map(|a| a.as_str()),
            Some("localhost:1234")
        );
        assert_eq!(
            request.path_with_query.as_ref().map(|p| p.as_str()),
            Some("/stream?x=1")
        );

        let headers = &request.headers;
        assert_eq!(headers.get("x-test").unwrap(), "guest");
        // The injected value replaces the guest-set one.
        let idempotency_values: Vec<_> = headers.get_all("idempotency-key").iter().collect();
        assert_eq!(idempotency_values, vec!["derived-key"]);
        assert_eq!(headers.get("traceparent").unwrap(), "00-abc-def-01");
        // The caller-supplied extra headers are applied like injected ones.
        assert_eq!(headers.get("range").unwrap(), "bytes=100-");

        let options = request.options.expect("recorded options should be carried");
        assert_eq!(
            options.connect_timeout,
            Some(std::time::Duration::from_secs(1))
        );
        assert_eq!(options.first_byte_timeout, None);
        assert_eq!(
            options.between_bytes_timeout,
            Some(std::time::Duration::from_secs(2))
        );
    }
}
