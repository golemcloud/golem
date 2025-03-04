// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::durable_host::http::serialized::{SerializableHttpMethod, SerializableHttpRequest};
use crate::durable_host::{
    DurabilityHost, DurableWorkerCtx, HttpRequestCloseOwner, HttpRequestState,
};
use crate::workerctx::{InvocationContextManagement, WorkerCtx};
use anyhow::anyhow;
use async_trait::async_trait;
use golem_common::model::invocation_context::AttributeValue;
use golem_common::model::oplog::DurableFunctionType;
use golem_service_base::headers::TraceContextHeaders;
use std::collections::HashMap;
use wasmtime::component::Resource;
use wasmtime_wasi_http::bindings::http::types;
use wasmtime_wasi_http::bindings::wasi::http::outgoing_handler::Host;
use wasmtime_wasi_http::types::{HostFutureIncomingResponse, HostOutgoingRequest};
use wasmtime_wasi_http::{HttpError, HttpResult};

#[async_trait]
impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn handle(
        &mut self,
        request: Resource<HostOutgoingRequest>,
        options: Option<Resource<types::RequestOptions>>,
    ) -> HttpResult<Resource<HostFutureIncomingResponse>> {
        self.observe_function_call("http::outgoing_handler", "handle");

        // Durability is handled by the WasiHttpView send_request method and the follow-up calls to await/poll the response future
        let begin_index = self
            .begin_durable_function(&DurableFunctionType::WriteRemoteBatched(None))
            .await
            .map_err(|err| HttpError::trap(anyhow!(err)))?;

        let host_request = self.table().get(&request)?;
        let uri = format!(
            "{}{}",
            host_request.authority.as_ref().unwrap_or(&String::new()),
            host_request
                .path_with_query
                .as_ref()
                .unwrap_or(&String::new())
        );
        let method = host_request.method.clone().into();

        let mut headers: HashMap<String, String> = host_request
            .headers
            .iter()
            .map(|(k, v)| {
                (
                    k.to_string(),
                    String::from_utf8_lossy(v.as_bytes()).to_string(),
                )
            })
            .collect();

        let span = self
            .start_span(&outgoing_http_request_span_attributes(&uri, &method))
            .await
            .map_err(|err| HttpError::trap(anyhow!(err)))?;

        if self.state.forward_trace_context_headers {
            let invocation_context = self
                .state
                .invocation_context
                .get_stack(span.span_id())
                .unwrap();
            let trace_context_headers =
                TraceContextHeaders::from_invocation_context(invocation_context);
            for (key, value) in trace_context_headers.to_raw_headers_map() {
                headers.insert(key, value);
            }
        }

        let result = Host::handle(&mut self.as_wasi_http_view(), request, options).await;

        match &result {
            Ok(future_incoming_response) => {
                // We have to call state.end_function to mark the completion of the remote write operation when we get a response.
                // For that we need to store begin_index and associate it with the response handle.

                let request = SerializableHttpRequest {
                    uri,
                    method,
                    headers,
                };

                let handle = future_incoming_response.rep();
                self.state.open_function_table.insert(handle, begin_index);
                self.state.open_http_requests.insert(
                    handle,
                    HttpRequestState {
                        close_owner: HttpRequestCloseOwner::FutureIncomingResponseDrop,
                        root_handle: handle,
                        request,
                        span_id: span.span_id().clone(),
                    },
                );
            }
            Err(_) => {
                self.end_durable_function(
                    &DurableFunctionType::WriteRemoteBatched(None),
                    begin_index,
                    false,
                )
                .await
                .map_err(|err| HttpError::trap(anyhow!(err)))?;
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
