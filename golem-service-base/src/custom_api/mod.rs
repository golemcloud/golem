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

// pub mod openapi;
pub mod path_pattern;
mod path_pattern_parser;
mod protobuf;
pub mod rib_compiler;

use self::path_pattern::AllPathPatterns;
use self::rib_compiler::{ComponentDependencyWithAgentInfo, compile_rib};
use desert_rust::BinaryCodec;
use golem_common::model::account::AccountId;
use golem_common::model::component::{ComponentId, ComponentName, ComponentRevision};
use golem_common::model::deployment::DeploymentRevision;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::http_api_definition::{
    HttpApiDefinitionId, HttpApiDefinitionName, HttpApiDefinitionVersion, RouteMethod,
};
use golem_common::model::security_scheme::{Provider, SecuritySchemeId, SecuritySchemeName};
use openidconnect::{ClientId, ClientSecret, RedirectUrl, Scope};
use rib::{
    Expr, RibByteCode, RibCompilationError, RibInputTypeInfo, RibOutputTypeInfo,
    WorkerFunctionsInRib,
};
use serde::Serialize;
use std::collections::HashMap;
use golem_common::model::agent::{AgentTypeName, CorsOptions, DataSchema, HttpMethod};

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
    pub method: HttpMethod,
    pub path: AllPathPatterns,
    pub behavior: RouteBehaviour,
    pub security_scheme: Option<SecuritySchemeId>,
    pub cors: CorsOptions
}

#[derive(Debug, Clone, PartialEq, BinaryCodec)]
#[desert(evolution())]
pub enum RouteBehaviour {
    CallAgent {
        agent_type: AgentTypeName,
        method_name: String,
        input_schema: DataSchema,
        output_schema: DataSchema
    },
    ServeSwaggerUi,
    HandleWebhookCallback
}

#[derive(Debug, Clone)]
pub struct SecuritySchemeDetails {
    pub id: SecuritySchemeId,
    pub name: SecuritySchemeName,
    pub provider_type: Provider,
    pub client_id: ClientId,
    pub client_secret: ClientSecret,
    pub redirect_url: RedirectUrl,
    pub scopes: Vec<Scope>,
}
