// Copyright 2024 Golem Cloud
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

use crate::gateway_execution::gateway_session::GatewaySessionStore;
use crate::gateway_middleware::{HttpCors, HttpMiddleware};
use async_trait::async_trait;
use golem_common::SafeDisplay;

// Implementation note: We have multiple `Middleware` (see `Middlewares`).
// While some middlewares are  specific to `MiddlewareIn` other are specific to `MiddlewareOut`.
// This ensures orthogonality in middleware usages, such that, at type level, we distinguish whether it is
// used only to process input (of api gateway) or used only to manipulate the output (of probably rib evaluation result)
// These middlewares are protocol-independent. It's input `GatewayRequestDetails`,
// with is an enum of different types (protocols) of input the protocol type.
// An `enum` over type parameter is used merely for simplicity.
// The middleware decides which protocol out of `GatewayRequestDetails` to process.
// This approach centralizes middleware management, and easy to add new middlewares without worrying about protocols.
#[async_trait]
pub trait HttpMiddlewareOut<Response> {
    async fn process_output(
        &self,
        session_store: &GatewaySessionStore,
        input: &mut Response,
    ) -> Result<(), MiddlewareOutError>;
}

pub enum MiddlewareOutError {
    InternalError(String),
}

impl SafeDisplay for MiddlewareOutError {
    fn to_safe_string(&self) -> String {
        match self {
            MiddlewareOutError::InternalError(error) => error.to_string(),
        }
    }
}

#[async_trait]
impl HttpMiddlewareOut<poem::Response> for HttpCors {
    async fn process_output(
        &self,
        _session_store: &GatewaySessionStore,
        input: &mut poem::Response,
    ) -> Result<(), MiddlewareOutError> {
        HttpMiddleware::apply_cors(input, self);
        Ok(())
    }
}
