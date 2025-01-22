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

use crate::bindings::golem::durability::durability::observe_function_call;

pub enum WrappedError {
    Proxied {
        error: crate::bindings::wasi::io::error::Error,
    },
    Message {
        message: String,
    },
}

impl WrappedError {
    pub fn proxied(error: crate::bindings::wasi::io::error::Error) -> Self {
        WrappedError::Proxied { error }
    }

    pub fn message(message: &str) -> Self {
        WrappedError::Message {
            message: message.to_string(),
        }
    }
}

impl crate::bindings::exports::wasi::io::error::GuestError for WrappedError {
    fn to_debug_string(&self) -> String {
        observe_function_call("io::error", "to_debug_string");
        match self {
            WrappedError::Proxied { error } => error.to_debug_string(),
            WrappedError::Message { message } => message.clone(),
        }
    }
}

impl Drop for WrappedError {
    fn drop(&mut self) {
        observe_function_call("io::error", "drop");
    }
}

impl crate::bindings::exports::wasi::io::error::Guest for crate::Component {
    type Error = WrappedError;
}
