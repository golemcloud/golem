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

use super::HttpCors;
use super::security_scheme::SecuritySchemeWithProviderMetadata;

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

#[derive(Debug, Clone, PartialEq)]
pub enum HttpMiddleware {
    Cors(HttpCors),
    AuthenticateRequest(Box<HttpAuthenticationMiddleware>), // Middleware to authenticate before feeding the input to the binding executor
}

#[derive(Debug, Clone, PartialEq)]
pub struct HttpAuthenticationMiddleware {
    pub security_scheme_with_metadata: SecuritySchemeWithProviderMetadata,
}
