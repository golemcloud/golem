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

pub mod compiled_gateway_binding;
pub mod openapi;
pub mod path_pattern;
mod path_pattern_parser;
mod protobuf;
pub mod rib_compiler;
pub mod security_scheme;

use self::compiled_gateway_binding::GatewayBindingCompiled;
use self::path_pattern::AllPathPatterns;
use self::security_scheme::SecuritySchemeDetails;
use desert_rust::BinaryCodec;
use golem_common::model::account::AccountId;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::http_api_definition::RouteMethod;
use golem_common::model::security_scheme::SecuritySchemeId;
use serde::Serialize;
use std::collections::HashMap;
use golem_common::model::deployment::DeploymentRevision;

#[derive(Debug, Clone)]
pub struct CompiledRoutes {
    pub account_id: AccountId,
    pub environment_id: EnvironmentId,
    pub deployment_revision: DeploymentRevision,
    pub security_schemes: HashMap<SecuritySchemeId, SecuritySchemeDetails>,
    pub routes: Vec<CompiledRoute>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompiledRoute {
    pub method: RouteMethod,
    pub path: AllPathPatterns,
    pub binding: GatewayBindingCompiled,
    pub security_scheme: Option<SecuritySchemeId>,
}

#[derive(Debug, Clone, PartialEq, BinaryCodec, Serialize)]
#[desert(evolution())]
pub struct HttpCors {
    pub allow_origin: String,
    pub allow_methods: String,
    pub allow_headers: String,
    pub expose_headers: Option<String>,
    pub allow_credentials: Option<bool>,
    pub max_age: Option<u64>,
}

impl Default for HttpCors {
    fn default() -> HttpCors {
        HttpCors {
            allow_origin: "*".to_string(),
            allow_methods: "GET, POST, PUT, DELETE, OPTIONS".to_string(),
            allow_headers: "Content-Type, Authorization".to_string(),
            expose_headers: None,
            max_age: None,
            allow_credentials: None,
        }
    }
}
