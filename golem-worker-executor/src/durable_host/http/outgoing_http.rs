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

use crate::durable_host::durability::InFunctionRetryHost;
use crate::durable_host::http::inline_retry::spawn_http_request_with_retry;
use crate::durable_host::{
    DurabilityHost, DurableWorkerCtx, HttpRequestCloseOwner, HttpRequestState, HttpRetryEligibility,
};
use crate::services::HasWorker;
use crate::workerctx::{InvocationContextManagement, InvocationManagement, WorkerCtx};
use golem_common::model::invocation_context::AttributeValue;
use golem_common::model::oplog::types::SerializableHttpMethod;
use golem_common::model::oplog::{DurableFunctionType, HostRequestHttpRequest, PersistenceLevel};
use golem_common::model::{IdempotencyKey, RetryContext};
use golem_service_base::headers::TraceContextHeaders;
use http::{HeaderName, HeaderValue};
use std::collections::HashMap;
use std::str::FromStr;
use wasmtime::component::Resource;
use wasmtime_wasi_http::bindings::http::outgoing_handler::Host;
use wasmtime_wasi_http::bindings::http::types;
use wasmtime_wasi_http::bindings::http::types::Scheme;
use wasmtime_wasi_http::types::{HostFutureIncomingResponse, HostOutgoingRequest};
use wasmtime_wasi_http::{HttpError, HttpResult};

fn is_method_idempotent(method: &SerializableHttpMethod) -> bool {
    matches!(
        method,
        SerializableHttpMethod::Get
            | SerializableHttpMethod::Head
            | SerializableHttpMethod::Put
            | SerializableHttpMethod::Delete
            | SerializableHttpMethod::Options
    )
}

pub(crate) fn maybe_enable_http_background_retry<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    handle: u32,
) -> HttpResult<()> {
    let state = match ctx.state.open_http_requests.get(&handle) {
        Some(state) => state.clone(),
        None => return Ok(()),
    };

    if state.retry.has_background_retry {
        return Ok(());
    }

    let durable_state = ctx.durable_execution_state();
    let named_retry_policies = ctx.state.named_retry_policies().to_vec();
    let method_eligible =
        durable_state.assume_idempotence || is_method_idempotent(&state.request.method);
    let body_ready_for_retry = state.retry.body_finished || state.output_stream_rep.is_none();
    let has_policies = !named_retry_policies.is_empty();

    let enable_background_retry = durable_state.is_live
        && durable_state.snapshotting_mode.is_none()
        && durable_state.persistence_level != PersistenceLevel::PersistNothing
        && !ctx.in_atomic_region()
        && has_policies
        && method_eligible
        && body_ready_for_retry
        && !state.retry.has_unreconstructable_body
        && !state.retry.has_outgoing_trailers
        && !state.retry.output_stream_subscribed;

    if !enable_background_retry {
        return Ok(());
    }

    let future_res = ctx
        .table()
        .get_mut(&Resource::<HostFutureIncomingResponse>::new_borrow(handle))?;
    let old = std::mem::replace(future_res, HostFutureIncomingResponse::Consumed);
    let wrapped = if let HostFutureIncomingResponse::Pending(orig_handle) = old {
        let mut retry_properties =
            RetryContext::http(&state.request.method.to_string(), &state.request.uri);
        ctx.state.enrich_retry_properties(&mut retry_properties);
        let retry_handle = spawn_http_request_with_retry(
            orig_handle,
            state.request.clone(),
            state.outgoing_request_config(),
            ctx.wasi_http.connection_pool.clone(),
            ctx.public_state.worker(),
            named_retry_policies,
            retry_properties,
            durable_state.max_in_function_retry_delay,
            state.begin_index,
            ctx.execution_status.clone(),
        );
        HostFutureIncomingResponse::pending(retry_handle)
    } else {
        old
    };
    let wrapped_is_pending = matches!(&wrapped, HostFutureIncomingResponse::Pending(_));
    *ctx.table()
        .get_mut(&Resource::<HostFutureIncomingResponse>::new_borrow(handle))? = wrapped;

    if let Some(state) = ctx.state.open_http_requests.get_mut(&handle) {
        state.retry.has_background_retry = wrapped_is_pending;
    }

    Ok(())
}

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn handle(
        &mut self,
        request: Resource<HostOutgoingRequest>,
        options: Option<Resource<types::RequestOptions>>,
    ) -> HttpResult<Resource<HostFutureIncomingResponse>> {
        self.observe_function_call("http::outgoing_handler", "handle");

        // Check the per-invocation HTTP call limit before initiating the call.
        // Only counted in live mode; replay is a no-op.
        self.state
            .check_and_increment_http_call_count()
            .map_err(|trap| HttpError::trap(wasmtime::Error::from(trap)))?;

        // Record against the monthly account-level HTTP call quota (live mode only).
        // Returns Err(WorkerMonthlyHttpCallBudgetExhausted) when exhausted,
        // which maps to RetryDecision::TryStop — suspending the worker until
        // the registry replenishes the budget (e.g. next billing month).
        self.record_monthly_http_call()
            .map_err(|e| HttpError::trap(wasmtime::Error::from_anyhow(e)))?;

        // Durability is handled by the WasiHttpView send_request method and the follow-up calls to await/poll the response future
        let begin_index = self
            .begin_durable_function(&DurableFunctionType::WriteRemoteBatched(None))
            .await
            .map_err(|err| HttpError::trap(wasmtime::Error::msg(err.to_string())))?;

        let host_request = self.table().get(&request)?;
        let scheme = match host_request.scheme.as_ref().unwrap_or(&Scheme::Https) {
            Scheme::Http => "http",
            Scheme::Https | Scheme::Other(_) => "https",
        };
        let uri = format!(
            "{}://{}{}",
            scheme,
            host_request.authority.as_ref().unwrap_or(&String::new()),
            host_request
                .path_with_query
                .as_ref()
                .unwrap_or(&String::new())
        );
        let method = host_request.method.clone().into();

        let mut headers: HashMap<String, String> = host_request
            .headers
            .as_ref()
            .iter()
            .map(|(k, v)| {
                (
                    k.to_string(),
                    String::from_utf8_lossy(v.as_bytes()).to_string(),
                )
            })
            .collect();

        let span = self
            .start_span(&outgoing_http_request_span_attributes(&uri, &method), false)
            .await
            .map_err(|err| HttpError::trap(wasmtime::Error::msg(err.to_string())))?;

        if self.state.forward_trace_context_headers {
            let invocation_context = self
                .state
                .invocation_context
                .get_stack(span.span_id())
                .unwrap();
            let host_request = self.table().get_mut(&request)?;

            let trace_context_headers =
                TraceContextHeaders::from_invocation_context(invocation_context);
            for (key, value) in trace_context_headers.to_raw_headers_map() {
                let header_name = HeaderName::from_str(&key).unwrap();
                host_request.headers.remove_all(&header_name);
                host_request
                    .headers
                    .append(&header_name, HeaderValue::from_str(&value).unwrap())
                    .map_err(HttpError::trap)?;
                headers.insert(key, value);
            }
        }

        if self.state.set_outgoing_http_idempotency_key {
            let current_idempotency_key = self
                .get_current_idempotency_key()
                .await
                .unwrap_or(IdempotencyKey::fresh());
            let idempotency_key = IdempotencyKey::derived(&current_idempotency_key, begin_index);

            let header_name = HeaderName::from_static("idempotency-key");

            let host_request = self.table().get_mut(&request)?;
            if !host_request.headers.as_ref().contains_key(&header_name) {
                host_request
                    .headers
                    .append(
                        &header_name,
                        HeaderValue::from_str(&idempotency_key.to_string()).unwrap(),
                    )
                    .map_err(HttpError::trap)?;
            }
        }

        let request_rep = request.rep();

        let host_request = self.table().get(&request)?;
        let use_tls = match host_request.scheme.as_ref().unwrap_or(&Scheme::Https) {
            Scheme::Http => false,
            Scheme::Https | Scheme::Other(_) => true,
        };

        let default_timeout = std::time::Duration::from_secs(600);
        let opts = options.as_ref().and_then(|o| self.table().get(o).ok());
        let connect_timeout = opts
            .and_then(|o| o.connect_timeout)
            .unwrap_or(default_timeout);
        let first_byte_timeout = opts
            .and_then(|o| o.first_byte_timeout)
            .unwrap_or(default_timeout);
        let between_bytes_timeout = opts
            .and_then(|o| o.between_bytes_timeout)
            .unwrap_or(default_timeout);

        // Capture pending request/body/stream mappings before calling handle().
        // The WASI implementation may drop the outgoing request resource as part of
        // handle(), which would otherwise clear these pending entries before we can
        // transfer them into HttpRequestState.
        let pending_outgoing_body_rep = self
            .state
            .pending_http_outgoing_request_body
            .remove(&request_rep);
        let pending_retry = self
            .state
            .pending_http_retry_eligibility
            .remove(&request_rep)
            .unwrap_or_default();
        let pending_output_stream_rep = pending_outgoing_body_rep.and_then(|body_rep| {
            self.state
                .pending_http_outgoing_body_stream
                .remove(&body_rep)
        });

        let result = Host::handle(&mut self.as_wasi_http_view(), request, options).await;

        match &result {
            Ok(future_incoming_response) => {
                // We have to call state.end_function to mark the completion of the remote write operation when we get a response.
                // For that we need to store begin_index and associate it with the response handle.

                let request = HostRequestHttpRequest {
                    uri,
                    method,
                    headers,
                };

                let handle = future_incoming_response.rep();
                let outgoing_body_rep = pending_outgoing_body_rep;
                let output_stream_rep = pending_output_stream_rep;

                self.state.open_http_requests.insert(
                    handle,
                    HttpRequestState {
                        close_owner: HttpRequestCloseOwner::FutureIncomingResponseDrop,
                        begin_index,
                        request,
                        span_id: span.span_id().clone(),
                        body_handle: None,
                        response_status: None,
                        outgoing_body_rep,
                        output_stream_rep,
                        use_tls,
                        connect_timeout,
                        first_byte_timeout,
                        between_bytes_timeout,
                        retry: HttpRetryEligibility {
                            has_background_retry: false,
                            ..pending_retry
                        },
                    },
                );

                maybe_enable_http_background_retry(self, handle)?;
            }
            Err(err) => {
                tracing::error!("!!! ERROR FROM handle(): {err:?}");
                self.end_durable_function(
                    &DurableFunctionType::WriteRemoteBatched(None),
                    begin_index,
                    false,
                )
                .await
                .map_err(|err| HttpError::trap(wasmtime::Error::msg(err.to_string())))?;
            }
        }

        result
    }
}

fn outgoing_http_request_span_attributes(
    uri: &str,
    method: &SerializableHttpMethod,
) -> Vec<(String, AttributeValue)> {
    vec![
        (
            "name".to_string(),
            AttributeValue::String("outgoing-http-request".to_string()),
        ),
        (
            "request.uri".to_string(),
            AttributeValue::String(uri.to_string()),
        ),
        (
            "request.method".to_string(),
            AttributeValue::String(method.to_string()),
        ),
    ]
}
