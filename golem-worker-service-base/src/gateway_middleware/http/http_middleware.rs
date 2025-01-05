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

use crate::gateway_middleware::http::authentication::HttpAuthenticationMiddleware;
use std::ops::Deref;
use std::future::Future;

use crate::gateway_middleware::http::cors::HttpCors;
use crate::gateway_security::SecuritySchemeWithProviderMetadata;
use http::header::{
    ACCESS_CONTROL_ALLOW_CREDENTIALS, ACCESS_CONTROL_ALLOW_ORIGIN, ACCESS_CONTROL_EXPOSE_HEADERS,
    ACCESS_CONTROL_ALLOW_METHODS, ACCESS_CONTROL_ALLOW_HEADERS, ACCESS_CONTROL_MAX_AGE,
    VARY,
};
use poem::{Middleware, Request, Response, Result, IntoResponse};

#[derive(Debug, Clone, PartialEq)]
pub enum HttpMiddleware {
    AddCorsHeaders(HttpCors),
    AuthenticateRequest(Box<HttpAuthenticationMiddleware>), // Middleware to authenticate before feeding the input to the binding executor
}

impl HttpMiddleware {
    pub fn get_cors(&self) -> Option<HttpCors> {
        match self {
            HttpMiddleware::AddCorsHeaders(cors) => Some(cors.clone()),
            HttpMiddleware::AuthenticateRequest(_) => None,
        }
    }

    pub fn get_http_authentication(&self) -> Option<HttpAuthenticationMiddleware> {
        match self {
            HttpMiddleware::AuthenticateRequest(authentication) => {
                Some(authentication.deref().clone())
            }
            HttpMiddleware::AddCorsHeaders(_) => None,
        }
    }

    pub fn authenticate_request(
        security_scheme: SecuritySchemeWithProviderMetadata,
    ) -> HttpMiddleware {
        HttpMiddleware::AuthenticateRequest(Box::new(HttpAuthenticationMiddleware {
            security_scheme_with_metadata: security_scheme,
        }))
    }
    pub fn cors(cors: HttpCors) -> Self {
        HttpMiddleware::AddCorsHeaders(cors)
    }

    pub fn apply_cors(response: &mut poem::Response, cors: &HttpCors) {
        // Allow Origin
        response.headers_mut().insert(
            ACCESS_CONTROL_ALLOW_ORIGIN,
            cors.get_allow_origin().parse().unwrap(),
        );

        // Allow Methods
        response.headers_mut().insert(
            ACCESS_CONTROL_ALLOW_METHODS,
            cors.get_allow_methods().parse().unwrap(),
        );

        // Allow Headers
        response.headers_mut().insert(
            ACCESS_CONTROL_ALLOW_HEADERS,
            cors.get_allow_headers().parse().unwrap(),
        );

        // Max Age
        if let Some(max_age) = cors.get_max_age() {
            response.headers_mut().insert(
                ACCESS_CONTROL_MAX_AGE,
                max_age.to_string().parse().unwrap(),
            );
        }

        // Allow Credentials
        if let Some(allow_credentials) = cors.get_allow_credentials() {
            response.headers_mut().insert(
                ACCESS_CONTROL_ALLOW_CREDENTIALS,
                allow_credentials.to_string().parse().unwrap(),
            );
        }

        // Expose Headers
        if let Some(expose_headers) = cors.get_expose_headers() {
            response.headers_mut().insert(
                ACCESS_CONTROL_EXPOSE_HEADERS,
                expose_headers.parse().unwrap(),
            );
        }

        // Vary
        if let Some(vary) = cors.get_vary() {
            response.headers_mut().insert(
                VARY,
                vary.join(", ").parse().unwrap(),
            );
        }
    }
}

#[async_trait::async_trait]
impl<E: poem::Endpoint + Send + Sync> Middleware<E> for HttpMiddleware
where
    E::Output: Send,
{
    type Output = MiddlewareImpl<E>;

    fn transform(&self, ep: E) -> Self::Output {
        MiddlewareImpl {
            inner: ep,
            middleware: self.clone(),
        }
    }
}

pub struct MiddlewareImpl<E> {
    inner: E,
    middleware: HttpMiddleware,
}

#[async_trait::async_trait]
impl<E: poem::Endpoint> poem::Endpoint for MiddlewareImpl<E> {
    type Output = Response;

    fn call(&self, req: Request) -> impl Future<Output = Result<Self::Output>> + Send {
        async move {
            match &self.middleware {
                HttpMiddleware::AddCorsHeaders(cors) => {
                    // Handle preflight OPTIONS requests
                    if req.method() == http::Method::OPTIONS {
                        let mut response = Response::default();
                        response.set_status(http::StatusCode::NO_CONTENT);
                        HttpMiddleware::apply_cors(&mut response, cors);
                        return Ok(response);
                    }

                    let response = self.inner.call(req).await?;
                    let mut response = response.into_response();
                    HttpMiddleware::apply_cors(&mut response, cors);
                    Ok(response)
                }
                HttpMiddleware::AuthenticateRequest(_auth) => {
                    // Handle authentication here if needed
                    let response = self.inner.call(req).await?;
                    Ok(response.into_response())
                }
            }
        }
    }
}
