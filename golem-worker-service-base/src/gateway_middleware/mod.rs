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
use crate::gateway_execution::gateway_session::GatewaySessionStore;
use crate::gateway_security::SecuritySchemeWithProviderMetadata;
pub use http::*;
pub use middleware_in::*;
pub use middleware_out::*;
use std::ops::Deref;

mod http;
mod middleware_in;
mod middleware_out;

// A set of middleware can exist in a binding.
// These middlewares will be processed in a sequential order.
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
pub struct Middlewares(pub Vec<Middleware>);

impl Middlewares {
    pub fn http_middlewares(&self) -> Vec<HttpMiddleware> {
        self.0
            .iter()
            .flat_map(|m| match m {
                Middleware::Http(http_middleware) => Some(http_middleware.clone()),
            })
            .collect()
    }

    pub async fn process_middleware_in<Namespace, Response>(
        &self,
        input: &Input<Namespace>,
    ) -> Result<MiddlewareSuccess<Response>, MiddlewareInError>
    where
        HttpRequestAuthentication: MiddlewareIn<Namespace, Response>,
    {
        for middleware in self.0.iter() {
            match middleware {
                Middleware::Http(http_middleware) => match http_middleware {
                    HttpMiddleware::AddCorsHeaders(_) => {}
                    HttpMiddleware::AuthenticateRequest(auth) => {
                        let result = auth.process_input(input).await?;
                        match result {
                            MiddlewareSuccess::Redirect(response) => {
                                return Ok(MiddlewareSuccess::Redirect(response))
                            }
                            MiddlewareSuccess::PassThrough => {}
                        }
                    }
                },
            }
        }

        Ok(MiddlewareSuccess::PassThrough)
    }

    pub async fn process_middleware_out<Out>(
        &self,
        session_store: &GatewaySessionStore,
        response: &mut Out,
    ) -> Result<(), MiddlewareOutError>
    where
        Cors: MiddlewareOut<Out>,
    {
        for middleware in self.0.iter() {
            match middleware {
                Middleware::Http(http_middleware) => match http_middleware {
                    HttpMiddleware::AddCorsHeaders(cors) => {
                        cors.process_output(session_store, response).await?;
                    }
                    HttpMiddleware::AuthenticateRequest(_) => {}
                },
            }
        }

        Ok(())
    }

    pub fn add(&mut self, middleware: Middleware) {
        self.0.push(middleware);
    }

    pub fn get_cors(&self) -> Option<Cors> {
        self.0.iter().find_map(|m| m.get_cors())
    }

    pub fn get_http_authentication(&self) -> Option<HttpRequestAuthentication> {
        self.0.iter().find_map(|m| m.get_http_authentication())
    }
}

// A middleware will not add, remove or update the input to worker what-so-ever,
// as Rib is well typed and there wouldn't be any magical pre-processing such as adding a field to the worker input record.
// In other words, users need to satisfy the Rib compiler (to not complain about the input or output of worker) while registering API definition.
// That said, depending on the middleware type, gateway can make certain decisions automatically
// such as adding CORS headers to the http response body instead of asking users to do this everytime the Rib script,
// even after specifying a CORS middleware plugin. However, these automated decisions will still be rare.
// In most cases, it is best for users to do every pre-processing of input and forming the shape of response by themselves,
// as every data related to the configured middleware is made available to Rib compiler.
#[derive(Debug, Clone, PartialEq)]
pub enum Middleware {
    Http(HttpMiddleware),
}

impl Middleware {
    pub fn cors(cors: &Cors) -> Middleware {
        Middleware::Http(HttpMiddleware::cors(cors.clone()))
    }

    pub fn get_cors(&self) -> Option<Cors> {
        match self {
            Middleware::Http(HttpMiddleware::AddCorsHeaders(cors)) => Some(cors.clone()),
            _ => None,
        }
    }

    pub fn get_http_authentication(&self) -> Option<HttpRequestAuthentication> {
        match self {
            Middleware::Http(HttpMiddleware::AuthenticateRequest(auth)) => {
                Some(auth.deref().clone())
            }
            _ => None,
        }
    }

    pub fn http(http_middleware: HttpMiddleware) -> Middleware {
        Middleware::Http(http_middleware)
    }
}

impl TryFrom<golem_api_grpc::proto::golem::apidefinition::Middleware> for Middlewares {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::apidefinition::Middleware,
    ) -> Result<Self, Self::Error> {
        let mut middlewares = Vec::new();
        if let Some(cors) = value.cors {
            let cors = Cors::try_from(cors)?;
            middlewares.push(Middleware::http(HttpMiddleware::cors(cors)));
        }

        if let Some(http) = value.http_authentication {
            let auth = SecuritySchemeWithProviderMetadata::try_from(http)?;
            middlewares.push(Middleware::Http(HttpMiddleware::authenticate_request(auth)))
        }

        Ok(Middlewares(middlewares))
    }
}

impl TryFrom<Middlewares> for golem_api_grpc::proto::golem::apidefinition::Middleware {
    type Error = String;
    fn try_from(value: Middlewares) -> Result<Self, String> {
        let mut cors = None;
        let mut auth = None;

        for middleware in value.0.iter() {
            match middleware {
                Middleware::Http(http_middleware) => match http_middleware {
                    HttpMiddleware::AddCorsHeaders(cors0) => {
                        cors = Some(golem_api_grpc::proto::golem::apidefinition::CorsPreflight::from(cors0.clone()));
                    }
                    HttpMiddleware::AuthenticateRequest(http_request_authentication) => {
                        auth = Some(golem_api_grpc::proto::golem::apidefinition::SecurityWithProviderMetadata::try_from(http_request_authentication.security_scheme.clone())?)
                    }
                }
            }
        }

        Ok(golem_api_grpc::proto::golem::apidefinition::Middleware {
            cors,
            http_authentication: auth,
        })
    }
}
