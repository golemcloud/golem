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

#[derive(Debug)]
pub enum ParsedRequestBody {
    Unused,
    JsonBody(golem_wasm::Value),
    // Always Some initially, will be None after being consumed by handler code
    UnstructuredBinary(Option<BinarySource>),
}
