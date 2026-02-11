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

pub mod api_definition_lookup;
pub mod call_agent;
mod cors;
pub mod error;
pub mod model;
pub mod poem_endpoint;
pub mod request_handler;
mod rich_request;
pub mod route_resolver;
pub mod router;
pub mod security;
pub mod webhoooks;

use self::poem_endpoint::CustomApiPoemEndpoint;
use crate::bootstrap::Services;
pub use model::*;
pub use rich_request::RichRequest;

pub fn make_custom_api_endpoint(services: &Services) -> CustomApiPoemEndpoint {
    CustomApiPoemEndpoint::new(services.request_handler.clone())
}
