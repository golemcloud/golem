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

use crate::gateway_execution::auth_call_back_binding_handler::AuthorisationError;
use crate::gateway_execution::gateway_session::SessionId;
use golem_common::SafeDisplay;

#[derive(Debug)]
pub enum MiddlewareError {
    Unauthorized(AuthorisationError),
    InternalError(String),
}

impl SafeDisplay for MiddlewareError {
    fn to_safe_string(&self) -> String {
        match self {
            MiddlewareError::Unauthorized(msg) => format!("Unauthorized: {}", msg.to_safe_string()),
            MiddlewareError::InternalError(msg) => {
                format!("Internal Server Error: {}", msg)
            }
        }
    }
}

pub enum MiddlewareSuccess {
    PassThrough { session_id: Option<SessionId> },
    Redirect(poem::Response),
}
