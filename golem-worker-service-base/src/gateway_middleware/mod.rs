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

use crate::gateway_binding::HttpRequestDetails;
use crate::gateway_execution::gateway_session::GatewaySessionStore;
use crate::gateway_security::{IdentityProvider, SecuritySchemeWithProviderMetadata};
pub use http::*;
use std::sync::Arc;

mod http;

// Middlewares will be processed in a sequential order.
// The information contained in each middleware is made available to
// the Rib environment as a key-value pair. This implies, users can look up the data
// related to the middleware in their Rib script.
// Also, depending on the middleware type, gateway can make certain decisions
// automatically, such as making sure to add origin header into the response body
// instead of polluting the Rib script when CORS is enabled.
// However, if there are conflicts  (Example: user specified
// a CORS header already, then gateway resolves these conflicts by giving priority to user input)
// In most cases, it is best for users to do every pre-processing of input and forming the shape of response by themselves.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct HttpMiddlewares(pub Vec<HttpMiddleware>);

impl HttpMiddlewares {
    pub fn add_cors(&mut self, cors: HttpCors) {
        self.0.push(HttpMiddleware::cors(cors));
    }

    pub async fn process_middleware_in(
        &self,
        http_request_details: &HttpRequestDetails,
        session_store: &GatewaySessionStore,
        identity_provider: &Arc<dyn IdentityProvider + Sync + Send>,
    ) -> Result<MiddlewareSuccess, MiddlewareError> {
        let mut final_session_id = None;

        for middleware in self.0.iter() {
            match middleware {
                HttpMiddleware::AddCorsHeaders(_) => {}
                HttpMiddleware::AuthenticateRequest(auth) => {
                    let result = auth
                        .apply_http_auth(http_request_details, session_store, identity_provider)
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
        &self,
        response: &mut poem::Response,
    ) -> Result<(), MiddlewareError> {
        for middleware in self.0.iter() {
            match middleware {
                HttpMiddleware::AddCorsHeaders(cors) => {
                    HttpMiddleware::apply_cors(response, cors);
                }
                HttpMiddleware::AuthenticateRequest(_) => {}
            }
        }

        Ok(())
    }

    pub fn add(&mut self, middleware: HttpMiddleware) {
        self.0.push(middleware);
    }

    pub fn get_cors_middleware(&self) -> Option<HttpCors> {
        self.0.iter().find_map(|m| m.get_cors())
    }

    pub fn get_http_authentication_middleware(&self) -> Option<HttpAuthenticationMiddleware> {
        self.0.iter().find_map(|m| m.get_http_authentication())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Middleware {
    Http(HttpMiddleware),
}

impl TryFrom<golem_api_grpc::proto::golem::apidefinition::Middleware> for HttpMiddlewares {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::apidefinition::Middleware,
    ) -> Result<Self, Self::Error> {
        let mut http_middlewares = Vec::new();
        if let Some(cors) = value.cors {
            let cors = HttpCors::try_from(cors)?;
            http_middlewares.push(HttpMiddleware::cors(cors));
        }

        if let Some(http) = value.http_authentication {
            let auth = SecuritySchemeWithProviderMetadata::try_from(http)?;
            http_middlewares.push(HttpMiddleware::authenticate_request(auth))
        }

        Ok(HttpMiddlewares(http_middlewares))
    }
}

impl TryFrom<HttpMiddlewares> for golem_api_grpc::proto::golem::apidefinition::Middleware {
    type Error = String;
    fn try_from(value: HttpMiddlewares) -> Result<Self, String> {
        let mut cors = None;
        let mut auth = None;

        for http_middleware in value.0.iter() {
            match http_middleware {
                HttpMiddleware::AddCorsHeaders(cors0) => {
                    cors = Some(golem_api_grpc::proto::golem::apidefinition::CorsPreflight::from(cors0.clone()));
                }
                HttpMiddleware::AuthenticateRequest(http_request_authentication) => {
                    auth = Some(golem_api_grpc::proto::golem::apidefinition::SecurityWithProviderMetadata::try_from(http_request_authentication.security_scheme_with_metadata.clone())?)
                }
            }
        }

        Ok(golem_api_grpc::proto::golem::apidefinition::Middleware {
            cors,
            http_authentication: auth,
        })
    }
}
