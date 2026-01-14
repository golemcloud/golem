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

use desert_rust::BinaryCodec;
use golem_common::model::account::AccountId;
use golem_common::model::deployment::DeploymentRevision;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::http_api_definition::{
    HttpApiDefinitionId, HttpApiDefinitionName, HttpApiDefinitionVersion, RouteMethod,
};
use golem_common::model::security_scheme::{SecuritySchemeId, SecuritySchemeName};
use golem_service_base::custom_api::RouteBehaviour;
use golem_service_base::custom_api::SecuritySchemeDetails;
use golem_service_base::custom_api::path_pattern::AllPathPatterns;
use std::collections::HashMap;
use golem_common::model::agent::{CorsOptions, HeaderVariable, HttpMethod, PathSegment, PathSegmentNode, QueryVariable};

#[derive(Debug, Clone, PartialEq, BinaryCodec)]
#[desert(evolution())]
pub struct CompiledRouteWithDynamicReferences {
    pub method: HttpMethod,
    pub path: Vec<PathSegmentNode>,
    pub header_vars: Vec<HeaderVariable>,
    pub query_vars: Vec<QueryVariable>,
    pub behaviour: RouteBehaviour,
    pub security_scheme: Option<SecuritySchemeName>,
    pub cors: CorsOptions
}

#[derive(Debug, Clone, PartialEq, BinaryCodec)]
#[desert(evolution())]
pub struct CompiledRouteWithoutSecurity {
    pub method: HttpMethod,
    pub path: AllPathPatterns,
    pub behaviour: RouteBehaviour,
    pub cors: CorsOptions
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompiledRouteWithContext {
    pub security_scheme: Option<SecuritySchemeName>,
    pub route: CompiledRouteWithoutSecurity,
}

#[derive(Debug, Clone)]
pub struct CompiledRouteWithSecuritySchemeDetails {
    pub account_id: AccountId,
    pub environment_id: EnvironmentId,
    pub http_api_definition_id: HttpApiDefinitionId,
    pub deployment_revision: DeploymentRevision,
    pub security_scheme_missing: bool,
    pub security_scheme: Option<SecuritySchemeDetails>,
    pub route: CompiledRouteWithoutSecurity,
}

#[derive(Debug, Clone)]
pub struct MaybeDisabledCompiledRoute {
    pub security_scheme_missing: bool,
    pub security_scheme: Option<SecuritySchemeId>,
    pub method: HttpMethod,
    pub path: AllPathPatterns,
    pub binding: RouteBehaviour,
}

// impl golem_service_base::custom_api::openapi::HttpApiRoute for MaybeDisabledCompiledRoute {
//     fn security_scheme_missing(&self) -> bool {
//         self.security_scheme_missing
//     }
//     fn security_scheme(&self) -> Option<SecuritySchemeId> {
//         self.security_scheme
//     }
//     fn method(&self) -> &RouteMethod {
//         &self.method
//     }
//     fn path(&self) -> &AllPathPatterns {
//         &self.path
//     }
//     fn binding(&self) -> &RouteBehaviour {
//         &self.binding
//     }
// }

#[derive(Debug, Clone)]
pub struct CompiledRoutesForHttpApiDefinition {
    pub http_api_definition_id: HttpApiDefinitionId,
    pub http_api_definition_name: HttpApiDefinitionName,
    pub http_api_definition_version: HttpApiDefinitionVersion,
    pub security_schemes: HashMap<SecuritySchemeId, SecuritySchemeDetails>,
    pub routes: Vec<MaybeDisabledCompiledRoute>,
}
