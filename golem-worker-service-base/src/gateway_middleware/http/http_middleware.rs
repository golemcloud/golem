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

use crate::gateway_middleware::http::authentication::HttpAuthenticationMiddleware;
use std::ops::Deref;

use crate::gateway_middleware::http::cors::HttpCors;

use crate::gateway_security::SecuritySchemeWithProviderMetadata;

#[derive(Debug, Clone, PartialEq)]
pub enum HttpMiddleware {
    Cors(HttpCors),
    AuthenticateRequest(Box<HttpAuthenticationMiddleware>), // Middleware to authenticate before feeding the input to the binding executor
}

impl HttpMiddleware {
    pub fn get_cors(&self) -> Option<HttpCors> {
        match self {
            HttpMiddleware::Cors(cors) => Some(cors.clone()),
            HttpMiddleware::AuthenticateRequest(_) => None,
        }
    }

    pub fn get_http_authentication(&self) -> Option<HttpAuthenticationMiddleware> {
        match self {
            HttpMiddleware::AuthenticateRequest(authentication) => {
                Some(authentication.deref().clone())
            }
            HttpMiddleware::Cors(_) => None,
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
        HttpMiddleware::Cors(cors)
    }
}
