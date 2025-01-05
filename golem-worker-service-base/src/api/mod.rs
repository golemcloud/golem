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

// Components and request data that can be reused for implementing server API endpoints
mod common;
mod custom_http_request_api;
mod error;
pub mod healthcheck;
mod register_api_definition_api;
pub mod routes;
pub mod wit_types_api;

pub mod rib_endpoints;

// Re-exports
pub use common::*;
pub use custom_http_request_api::*;
pub use error::*;
pub use healthcheck::*;
pub use register_api_definition_api::*;
pub use rib_endpoints::*;
pub use wit_types_api::*;
pub use routes::create_api_router;
