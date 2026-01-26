// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use golem_common::model::agent::BinarySource;
use http::{HeaderName, StatusCode};
use std::collections::HashMap;
use std::fmt;

#[derive(Debug)]
pub struct RouteExecutionResult {
    pub status: StatusCode,
    pub headers: HashMap<HeaderName, String>,
    pub body: ResponseBody,
}

pub enum ResponseBody {
    NoBody,
    ComponentModelJsonBody { body: golem_wasm::ValueAndType },
    UnstructuredBinaryBody { body: BinarySource },
}

impl fmt::Debug for ResponseBody {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ResponseBody::NoBody => f.debug_struct("NoBody").finish(),
            ResponseBody::ComponentModelJsonBody { body } => f
                .debug_struct("ComponentModelJsonBody")
                .field("body", body)
                .finish(),
            ResponseBody::UnstructuredBinaryBody { .. } => f.write_str("UnstructuredBinaryBody"),
        }
    }
}

pub enum ParsedRequestBody {
    Unused,
    JsonBody(golem_wasm::Value),
    // Always Some initially, will be None after being consumed by handler code
    UnstructuredBinary(Option<BinarySource>),
}

impl fmt::Debug for ParsedRequestBody {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParsedRequestBody::Unused => f.write_str("Unused"),
            ParsedRequestBody::JsonBody(value) => f.debug_tuple("JsonBody").field(value).finish(),
            ParsedRequestBody::UnstructuredBinary(_) => f.write_str("UnstructuredBinary"),
        }
    }
}
