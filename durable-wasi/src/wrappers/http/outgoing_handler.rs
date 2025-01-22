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

use crate::bindings::exports::wasi::http::outgoing_handler::{
    ErrorCode, FutureIncomingResponse, OutgoingRequest, RequestOptions,
};
use crate::bindings::exports::wasi::http::types::{GuestFields, GuestOutgoingRequest};
use crate::bindings::golem::durability::durability::{
    begin_durable_function, end_durable_function, observe_function_call, DurableFunctionType,
};
use crate::bindings::wasi::http::outgoing_handler::handle;
use crate::wrappers::http::serialized::SerializableHttpRequest;
use crate::wrappers::http::types::{
    WrappedFields, WrappedFutureIncomingResponse, WrappedOutgoingRequest, WrappedRequestOptions,
};
use crate::wrappers::http::{
    HttpRequestCloseOwner, HttpRequestState, OPEN_FUNCTION_TABLE, OPEN_HTTP_REQUESTS,
};
use std::collections::HashMap;

impl crate::bindings::exports::wasi::http::outgoing_handler::Guest for crate::Component {
    fn handle(
        request: OutgoingRequest,
        options: Option<RequestOptions>,
    ) -> Result<FutureIncomingResponse, ErrorCode> {
        observe_function_call("http::outgoing_handler", "handle");

        // Durability is handled by the WasiHttpView send_request method and the follow-up calls to await/poll the response future
        let begin_index = begin_durable_function(DurableFunctionType::WriteRemoteBatched(None));

        let mut wrapped_request = request.into_inner::<WrappedOutgoingRequest>();
        let authority = wrapped_request.authority().unwrap_or(String::new());
        let path_with_query = wrapped_request.path_with_query().unwrap_or(String::new());
        let uri = format!("{}{}", authority, path_with_query);
        let method = wrapped_request.method().into();
        let headers: HashMap<String, String> = wrapped_request
            .headers()
            .get::<WrappedFields>()
            .entries()
            .into_iter()
            .map(|(k, v)| (k, String::from_utf8_lossy(&v).to_string()))
            .collect();

        let request = wrapped_request.take_inner();
        let options =
            options.map(|options| options.into_inner::<WrappedRequestOptions>().take_inner());
        let result = handle(request, options);

        match &result {
            Ok(future_incoming_response) => {
                // We have to call state.end_function to mark the completion of the remote write operation when we get a response.
                // For that we need to store begin_index and associate it with the response handle.
                let request = SerializableHttpRequest {
                    uri,
                    method,
                    headers,
                };

                let handle = future_incoming_response.handle();
                OPEN_FUNCTION_TABLE.with_borrow_mut(|open_function_table| {
                    open_function_table.insert(handle, begin_index);
                });
                OPEN_HTTP_REQUESTS.with_borrow_mut(|open_http_requests| {
                    open_http_requests.insert(
                        handle,
                        HttpRequestState {
                            close_owner: HttpRequestCloseOwner::FutureIncomingResponseDrop,
                            root_handle: handle,
                            request,
                        },
                    );
                });
            }
            Err(_) => {
                end_durable_function(DurableFunctionType::WriteRemoteBatched(None), begin_index);
            }
        };

        let response = result?;
        Ok(FutureIncomingResponse::new(WrappedFutureIncomingResponse {
            response,
        }))
    }
}
