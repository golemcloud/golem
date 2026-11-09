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

/// Everything needed to re-issue a recorded p3 `client::send` after a restart
/// (the P3 counterpart of P2's `rebuild_request_after_replay`): the request
/// head + options recorded in the send's `Start` payload (reconstructed
/// deterministically from the guest-rebuilt request resource during replay)
/// and the Golem-managed headers (`traceparent`/`tracestate`,
/// `idempotency-key`) re-derived from recorded state — the replayed span and
/// the send's own `Start` index — so the re-issued request is byte-identical
/// to the original in every Golem-controlled aspect.
///
/// The request body is reconstructed from its durable frame recording when the request is re-issued.
pub(in crate::durable_host::p3::http) fn outgoing_http_request_uri(
    request: &SerializableP3HttpClientSend,
) -> String {
    let scheme = match request
        .scheme
        .as_ref()
        .unwrap_or(&SerializableP3HttpScheme::Https)
    {
        SerializableP3HttpScheme::Http => "http",
        SerializableP3HttpScheme::Https | SerializableP3HttpScheme::Other(_) => "https",
    };
    format!(
        "{}://{}{}",
        scheme,
        request.authority.as_deref().unwrap_or(""),
        request.path_with_query.as_deref().unwrap_or("")
    )
}

/// Span attributes for the `outgoing-http-request` invocation-context span,
/// mirroring the P2 `http::outgoing_handler::handle` span shape.
pub(in crate::durable_host::p3::http) fn outgoing_http_request_span_attributes(
    request: &SerializableP3HttpClientSend,
) -> Vec<(String, AttributeValue)> {
    let uri = outgoing_http_request_uri(request);
    vec![
        (
            "name".to_string(),
            AttributeValue::String("outgoing-http-request".to_string()),
        ),
        ("request.uri".to_string(), AttributeValue::String(uri)),
        (
            "request.method".to_string(),
            AttributeValue::String(request.method.to_string()),
        ),
    ]
}

/// Computes the Golem-managed headers to inject into an outgoing p3 HTTP
/// request via the shared policy (see `http::policy::golem_managed_http_headers`),
/// using the send's own host-call `Start` index as the idempotency-key
/// derivation point — stable across live execution and replay, so a retried
/// send reuses the same key. `guest_headers` is the serialized request head
/// used to detect a guest-provided idempotency key.
pub(in crate::durable_host::p3::http) fn golem_outgoing_http_headers<Ctx: WorkerCtx, U: Send>(
    store: &Accessor<U, DurableP3<Ctx>>,
    span_id: &SpanId,
    start_index: OplogIndex,
    guest_headers: &HashMap<String, Vec<Vec<u8>>>,
) -> Result<Vec<(String, String)>, WorkerExecutorError> {
    store.with(|mut access| {
        let ctx = durable_worker_ctx::<Ctx, U>(access.data_mut());
        golem_managed_http_headers(
            ctx,
            span_id,
            start_index,
            guest_headers.contains_key("idempotency-key"),
        )
    })
}

/// Applies the injected headers to the actual request resource (replacing any
/// existing values for the same names) so the network request carries them.
/// The guest constructed the request with immutable headers, so they are
/// briefly remarked mutable, mirroring the P2 injection.
pub(in crate::durable_host::p3::http) fn apply_headers_to_request_resource<
    Ctx: WorkerCtx,
    U: Send,
>(
    store: &Accessor<U, DurableP3<Ctx>>,
    req: &Resource<Request>,
    headers: &[(String, String)],
) -> Result<(), WorkerExecutorError> {
    if headers.is_empty() {
        return Ok(());
    }
    let http_store = store.with_getter::<WasiHttp>(wasi_http_view::<Ctx, U>);
    http_store.with(|mut access| {
        let view = access.get();
        let field_size_limit = view.ctx.field_size_limit;
        let request = view
            .table
            .get_mut(&Resource::<wasmtime_wasi_http::p3::Request>::new_borrow(
                req.rep(),
            ))
            .map_err(|err| {
                WorkerExecutorError::runtime(format!(
                    "failed to get outgoing p3 HTTP request from table: {err}"
                ))
            })?;
        apply_managed_http_headers(&mut request.headers, view.hooks, field_size_limit, headers)
            .map_err(WorkerExecutorError::runtime)
    })
}
