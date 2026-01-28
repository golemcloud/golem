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
use golem_wasm::ValueAndType;
use http::StatusCode;
use std::fmt;

pub enum RouteExecutionResult {
    NoBody {
        status: StatusCode,
    },
    ComponentModelJsonBody {
        body: golem_wasm::ValueAndType,
        status: StatusCode,
    },
    UnstructuredBinaryBody {
        body: BinarySource,
    },
    CustomAgentError {
        body: ValueAndType,
    },
}

impl fmt::Debug for RouteExecutionResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RouteExecutionResult::NoBody { status } => {
                f.debug_struct("NoBody").field("status", status).finish()
            }
            RouteExecutionResult::ComponentModelJsonBody { body, status } => f
                .debug_struct("ComponentModelJsonBody")
                .field("body", body)
                .field("status", status)
                .finish(),
            RouteExecutionResult::UnstructuredBinaryBody { .. } => {
                f.write_str("UnstructuredBinaryBody")
            }
            RouteExecutionResult::CustomAgentError { body } => f
                .debug_struct("CustomAgentError")
                .field("body", body)
                .finish(),
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
