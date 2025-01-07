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

use crate::gateway_middleware::http::authentication::HttpAuthenticationMiddleware;
use std::ops::Deref;

use crate::gateway_middleware::http::cors::HttpCors;
use crate::gateway_security::SecuritySchemeWithProviderMetadata;
use http::header::{
    ACCESS_CONTROL_ALLOW_CREDENTIALS, ACCESS_CONTROL_ALLOW_ORIGIN, ACCESS_CONTROL_EXPOSE_HEADERS,
};

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
        response.headers_mut().insert(
            ACCESS_CONTROL_ALLOW_ORIGIN,
            // hot path, and this unwrap will not fail unless we bypassed it during configuration
            cors.get_allow_origin().clone().parse().unwrap(),
        );

        if let Some(allow_credentials) = &cors.get_allow_credentials() {
            response.headers_mut().insert(
                ACCESS_CONTROL_ALLOW_CREDENTIALS,
                // hot path, and this unwrap will not fail unless we bypassed it during configuration
                allow_credentials.to_string().clone().parse().unwrap(),
            );
        }

        if let Some(expose_headers) = &cors.get_expose_headers() {
            response.headers_mut().insert(
                ACCESS_CONTROL_EXPOSE_HEADERS,
                // hot path, and this unwrap will not fail unless we bypassed it during configuration
                expose_headers.clone().parse().unwrap(),
            );
        }
    }
}
