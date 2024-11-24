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

use crate::gateway_binding::GatewayRequestDetails;
use crate::gateway_execution::gateway_input_executor::Input;
use crate::gateway_middleware::HttpRequestAuthentication;
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
pub trait MiddlewareIn<Namespace, Out> {
    async fn process_input(
        &self,
        input: &Input<Namespace>,
    ) -> Result<MiddlewareSuccess<Out>, MiddlewareInError>;
}

pub enum MiddlewareInError {
    Unauthorized(String),
    InternalServerError(String),
}

impl SafeDisplay for MiddlewareInError {
    fn to_safe_string(&self) -> String {
        match self {
            MiddlewareInError::Unauthorized(msg) => format!("Unauthorized: {}", msg),
            MiddlewareInError::InternalServerError(msg) => {
                format!("Internal Server Error: {}", msg)
            }
        }
    }
}

pub enum MiddlewareSuccess<Out> {
    PassThrough,
    Redirect(Out),
}

#[async_trait]
impl<Namespace: Send + Sync> MiddlewareIn<Namespace, poem::Response> for HttpRequestAuthentication {
    async fn process_input(
        &self,
        input: &Input<Namespace>,
    ) -> Result<MiddlewareSuccess<poem::Response>, MiddlewareInError> {
        match &input.resolved_gateway_binding.request_details {
            GatewayRequestDetails::Http(http_request) => {
                self.apply_http_auth(
                    http_request,
                    &input.session_store,
                    &input.identity_provider_resolver,
                )
                .await
            }
        }
    }
}
