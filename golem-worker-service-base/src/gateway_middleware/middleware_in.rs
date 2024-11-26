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

use crate::gateway_execution::gateway_http_input_executor::GatewayHttpInput;
use crate::gateway_middleware::HttpAuthenticationMiddleware;
use async_trait::async_trait;
use golem_common::SafeDisplay;

#[async_trait]
pub trait HttpMiddlewareIn<Namespace> {
    async fn process_input(
        &self,
        input: &GatewayHttpInput<Namespace>,
    ) -> Result<MiddlewareSuccess, MiddlewareInError>;
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

pub enum MiddlewareSuccess {
    PassThrough,
    Redirect(poem::Response),
}

#[async_trait]
impl<Namespace: Send + Sync> HttpMiddlewareIn<Namespace> for HttpAuthenticationMiddleware {
    async fn process_input(
        &self,
        input: &GatewayHttpInput<Namespace>,
    ) -> Result<MiddlewareSuccess, MiddlewareInError> {
        self.apply_http_auth(
            &input.http_request_details,
            &input.session_store,
            &input.identity_provider_resolver,
        )
        .await
    }
}
