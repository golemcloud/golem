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

mod auth;
mod cors;

use self::auth::apply_http_auth;
use self::cors::{add_cors_headers_to_response, apply_cors, CorsError};
use crate::gateway_execution::auth_call_back_binding_handler::AuthorisationError;
use crate::gateway_execution::gateway_session_store::{GatewaySessionStore, SessionId};
use crate::gateway_execution::request::RichRequest;
use crate::gateway_security::IdentityProvider;
use golem_common::SafeDisplay;
use golem_service_base::worker_api::http_middlewares::{HttpMiddleware, HttpMiddlewares};
use std::sync::Arc;

pub enum MiddlewareSuccess {
    PassThrough { session_id: Option<SessionId> },
    Redirect(poem::Response),
}

#[derive(Debug)]
pub enum MiddlewareError {
    Unauthorized(AuthorisationError),
    CorsError(CorsError),
    InternalError(String),
}

impl SafeDisplay for MiddlewareError {
    fn to_safe_string(&self) -> String {
        match self {
            MiddlewareError::Unauthorized(msg) => format!("Unauthorized: {}", msg.to_safe_string()),
            MiddlewareError::CorsError(error) => match error {
                CorsError::OriginNotAllowed => "CORS Error: Origin not allowed".to_string(),
                CorsError::MethodNotAllowed => "CORS Error: Method not allowed".to_string(),
                CorsError::HeadersNotAllowed => "CORS Error: Headers not allowed".to_string(),
            },
            MiddlewareError::InternalError(msg) => {
                format!("Internal Server Error: {msg}")
            }
        }
    }
}

pub async fn process_middleware_in(
    middlewars: &HttpMiddlewares,
    rich_request: &RichRequest,
    session_store: &Arc<dyn GatewaySessionStore>,
    identity_provider: &Arc<dyn IdentityProvider>,
) -> Result<MiddlewareSuccess, MiddlewareError> {
    let mut final_session_id = None;

    for middleware in &middlewars.0 {
        match middleware {
            HttpMiddleware::Cors(cors) => {
                apply_cors(cors, rich_request).map_err(MiddlewareError::CorsError)?;
            }
            HttpMiddleware::AuthenticateRequest(auth) => {
                let result = apply_http_auth(
                    &auth.security_scheme_with_metadata,
                    rich_request,
                    session_store,
                    identity_provider,
                )
                .await?;

                match result {
                    MiddlewareSuccess::Redirect(response) => {
                        return Ok(MiddlewareSuccess::Redirect(response))
                    }
                    MiddlewareSuccess::PassThrough { session_id } => {
                        final_session_id = session_id;
                    }
                }
            }
        }
    }

    Ok(MiddlewareSuccess::PassThrough {
        session_id: final_session_id,
    })
}

pub async fn process_middleware_out(
    middlewares: &HttpMiddlewares,
    response: &mut poem::Response,
) -> Result<(), MiddlewareError> {
    for middleware in &middlewares.0 {
        match middleware {
            HttpMiddleware::Cors(cors) => {
                add_cors_headers_to_response(cors, response);
            }
            HttpMiddleware::AuthenticateRequest(_) => {}
        }
    }

    Ok(())
}
