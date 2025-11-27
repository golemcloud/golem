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

use super::compiled_gateway_binding::GatewayBindingCompiled;
use super::http_middlewares::HttpMiddlewares;
use super::path_pattern::AllPathPatterns;
use golem_common::model::account::AccountId;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::http_api_definition::{
    HttpApiDefinitionId, HttpApiDefinitionRevision, RouteMethod,
};
use std::fmt::Debug;

// The Rib Expressions that exists in various parts of HttpApiDefinition (mainly in Routes)
// are compiled to form CompiledHttpApiDefinition.
// The Compilation happens during API definition registration,
// and is persisted, so that custom http requests are served by looking up
// CompiledHttpApiDefinition
#[derive(Debug, Clone, PartialEq)]
pub struct CompiledHttpApiDefinition {
    pub id: HttpApiDefinitionId,
    pub revision: HttpApiDefinitionRevision,
    pub routes: Vec<CompiledRoute>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub account_id: AccountId,
    pub environment_id: EnvironmentId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompiledRoute {
    pub method: RouteMethod,
    pub path: AllPathPatterns,
    pub binding: GatewayBindingCompiled,
    pub middlewares: Option<HttpMiddlewares>,
}
