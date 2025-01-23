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

use crate::bindings::exports::wasi::keyvalue::wasi_keyvalue_error::Error;
use crate::bindings::golem::durability::durability::observe_function_call;

pub enum WrappedError {
    Proxied { error: crate::bindings::wasi::keyvalue::wasi_keyvalue_error::Error },
    Message { message: String }
}

impl crate::bindings::exports::wasi::keyvalue::wasi_keyvalue_error::GuestError for WrappedError {
    fn trace(&self) -> String {
        observe_function_call("keyvalue::wasi_cloud_error", "trace");
        self.error.trace()
    }
}

impl Drop for WrappedError {
    fn drop(&mut self) {
        observe_function_call("keyvalue::wasi_cloud_error", "drop");
    }
}

impl crate::bindings::exports::wasi::keyvalue::wasi_keyvalue_error::Guest for crate::Component {
    type Error = WrappedError;
}

impl From<&crate::bindings::wasi::keyvalue::wasi_keyvalue_error::Error> for Error {
    fn from(value: crate::bindings::wasi::keyvalue::wasi_keyvalue_error::Error) -> Self {
        Error::new(WrappedError { error: value })
    }
}
