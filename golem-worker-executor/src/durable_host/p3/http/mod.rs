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

mod host_types;
mod rebuild;
mod replay;
mod request_body;
mod response_body;
mod send;
mod serialization;
#[cfg(test)]
pub(super) mod test_support;

#[allow(unused_imports)]
use host_types::*;
#[allow(unused_imports)]
use rebuild::*;
use replay::*;
use request_body::*;
use response_body::*;
use send::*;
#[allow(unused_imports)]
use serialization::*;

pub(crate) use request_body::PendingHttpRequestBodyTransmission;
pub(crate) use response_body::OpenP3HttpResponseState;

use wasmtime_wasi::TrappableError;
use wasmtime_wasi_http::p3::bindings::http::types;
use wasmtime_wasi_http::p3::bindings::http::types::ErrorCode;

pub(super) type HttpError = TrappableError<ErrorCode>;
pub(super) type HeaderError = TrappableError<types::HeaderError>;
pub(super) type RequestOptionsError = TrappableError<types::RequestOptionsError>;

pub(super) type HttpResult<T> = Result<T, HttpError>;
pub(super) type HeaderResult<T> = Result<T, HeaderError>;
pub(super) type RequestOptionsResult<T> = Result<T, RequestOptionsError>;
