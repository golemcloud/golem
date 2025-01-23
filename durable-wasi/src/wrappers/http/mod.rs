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

use crate::bindings::golem::durability::durability::{
    end_durable_function, DurableFunctionType, OplogIndex,
};
use crate::bindings::wasi::logging::logging::Level;
use crate::wrappers::http::serialized::SerializableHttpRequest;
use std::cell::RefCell;
use std::collections::HashMap;

mod outgoing_handler;
pub mod serialized;
mod types;

/// Indicates which step of the http request handling is responsible for closing an open
/// http request (by calling end_function)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpRequestCloseOwner {
    FutureIncomingResponseDrop,
    IncomingResponseDrop,
    IncomingBodyDropOrFinish,
    InputStreamClosed,
}

/// State associated with ongoing http requests, on top of the underlying wasi-http implementation
#[derive(Debug, Clone)]
pub struct HttpRequestState {
    /// Who is responsible for calling end_function and removing entries from the table
    pub close_owner: HttpRequestCloseOwner,
    /// The handle of the FutureIncomingResponse that is registered into the open_function_table
    pub root_handle: u32,
    /// Information about the request to be included in the oplog
    pub request: SerializableHttpRequest,
}

thread_local! {
    /// State of ongoing http requests, key is the resource id it is most recently associated with (one state object can belong to multiple resources, but just one at once)
    pub static OPEN_HTTP_REQUESTS: RefCell<HashMap<u32, HttpRequestState>> = RefCell::new(HashMap::new());

    pub static OPEN_FUNCTION_TABLE: RefCell<HashMap<u32, OplogIndex>> = RefCell::new(HashMap::new());
}

fn end_http_request(current_handle: u32) {
    OPEN_HTTP_REQUESTS.with_borrow_mut(|open_http_requests| {
        OPEN_FUNCTION_TABLE.with_borrow_mut(|open_function_table| {
            end_http_request_borrowed(open_http_requests, open_function_table, current_handle)
        })
    })
}

pub fn end_http_request_borrowed(
    open_http_requests: &mut HashMap<u32, HttpRequestState>,
    open_function_table: &mut HashMap<u32, OplogIndex>,
    current_handle: u32,
) {
    if let Some(state) = open_http_requests.remove(&current_handle) {
        match open_function_table.get(&state.root_handle) {
            Some(begin_index) => {
                end_durable_function(DurableFunctionType::WriteRemoteBatched(None), *begin_index);
                open_function_table.remove(&state.root_handle);
                open_http_requests.remove(&current_handle);
            }
            None => {
                crate::bindings::wasi::logging::logging::log(
                            Level::Warn,
                            "",
                            &format!("No matching BeginRemoteWrite index was found when HTTP response arrived. Handle: {}; open functions: {:?}", state.root_handle, open_function_table),
                        );
            }
        }
    } else {
        crate::bindings::wasi::logging::logging::log(
                    Level::Warn,
                    "",
                    &format!("No matching HTTP request is associated with resource handle. Handle: {}, open requests: {:?}", current_handle, open_http_requests),
                );
    }
}

fn continue_http_request(
    current_handle: u32,
    new_handle: u32,
    new_close_owner: HttpRequestCloseOwner,
) {
    OPEN_HTTP_REQUESTS.with_borrow_mut(|open_http_requests| {
        continue_http_request_borrowed(
            open_http_requests,
            current_handle,
            new_handle,
            new_close_owner,
        )
    })
}

fn continue_http_request_borrowed(
    open_http_requests: &mut HashMap<u32, HttpRequestState>,
    current_handle: u32,
    new_handle: u32,
    new_close_owner: HttpRequestCloseOwner,
) {
    if let Some(mut state) = open_http_requests.remove(&current_handle) {
        state.close_owner = new_close_owner;
        open_http_requests.insert(new_handle, state);
    } else {
        crate::bindings::wasi::logging::logging::log(
            Level::Warn,
            "",
            &format!("No matching HTTP request is associated with resource handle. Handle: {}, open requests: {:?}", current_handle, open_http_requests),
        );
    }
}
