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
use crate::durable_host::http::policy::InjectedHttpHeaders;

/// Reconstructs the outgoing URI from the recorded request head.
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

/// Span attributes for the `outgoing-http-request` invocation-context span.
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

/// Selects managed headers from the guest request and resolved trace headers.
/// Derivation uses the send's `Start` index or its reserved entity logical position.
/// A guest-provided key suppresses derivation.
pub(in crate::durable_host::p3::http) fn golem_outgoing_http_headers(
    request: &wasmtime_wasi_http::p3::Request,
    trace_headers: Vec<(String, String)>,
    set_outgoing_http_idempotency_key: bool,
    start_index: OplogIndex,
    key_position: &crate::durable_host::IdempotencyKeyPosition,
) -> InjectedHttpHeaders {
    golem_managed_http_headers(
        trace_headers,
        set_outgoing_http_idempotency_key,
        request.headers.keys().map(|name| name.as_str()),
        start_index,
    )
    .resolve(|index| key_position.derive(index))
}

/// Applies the injected headers to the actual request resource (replacing any
/// existing values for the same names) so the network request carries them.
/// The guest constructed the request with immutable headers, so they are
/// briefly remarked mutable during injection.
pub(in crate::durable_host::p3::http) fn apply_headers_to_request_resource(
    view: wasmtime_wasi_http::WasiHttpCtxView<'_>,
    req: &Resource<Request>,
    headers: &[(Box<str>, Box<str>)],
) -> Result<(), WorkerExecutorError> {
    if headers.is_empty() {
        return Ok(());
    }
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
}
