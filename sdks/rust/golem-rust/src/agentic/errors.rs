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

use crate::bindings::golem::agent::common::ValueAndType;
use crate::golem_agentic::golem::agent::common::AgentError;
use crate::value_and_type::IntoValue;

pub fn custom_error(msg: impl ToString) -> AgentError {
    AgentError::CustomError(ValueAndType {
        value: msg.to_string().into_value(),
        typ: String::get_type(),
    })
}

pub fn internal_error(msg: impl ToString) -> AgentError {
    custom_error(format!("Internal error: {}", msg.to_string()))
}

pub fn invalid_input_error(msg: impl ToString) -> AgentError {
    AgentError::InvalidInput(msg.to_string())
}

pub fn invalid_method_error(method_name: impl ToString) -> AgentError {
    AgentError::InvalidMethod(method_name.to_string())
}
