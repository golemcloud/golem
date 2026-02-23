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
use golem_common::model::agent::HttpMethod;
use golem_common::model::deployment::DeploymentRevision;
use golem_common::model::domain_registration::Domain;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::security_scheme::{SecuritySchemeId, SecuritySchemeName};
use golem_service_base::custom_api::{
    CorsOptions, PathSegment, RequestBodySchema, RouteBehaviour, RouteId, SecuritySchemeDetails,
    SessionFromHeaderRouteSecurity,
};
use std::collections::HashMap;

#[derive(Debug, BinaryCodec)]
#[desert(evolution())]
pub enum UnboundRouteSecurity {
    None,
    SessionFromHeader(SessionFromHeaderRouteSecurity),
    SecurityScheme(UnboundSecuritySchemeRouteSecurity),
}

#[derive(Debug, BinaryCodec)]
#[desert(evolution())]
pub struct UnboundSecuritySchemeRouteSecurity {
    pub security_scheme: SecuritySchemeName,
}

#[derive(Debug, BinaryCodec)]
#[desert(evolution())]
// Flattened version of golem_service_base::custom_api::CompiledRoute with late-bound references still unresolved
pub struct UnboundCompiledRoute {
    pub domain: Domain,
    pub route_id: RouteId,
    pub method: HttpMethod,
    pub path: Vec<PathSegment>,
    pub body: RequestBodySchema,
    pub behaviour: RouteBehaviour,
    pub security: UnboundRouteSecurity,
    pub cors: CorsOptions,
}

impl UnboundCompiledRoute {
    pub fn security_scheme(&self) -> Option<SecuritySchemeName> {
        match &self.security {
            UnboundRouteSecurity::SecurityScheme(inner) => Some(inner.security_scheme.clone()),
            UnboundRouteSecurity::None | UnboundRouteSecurity::SessionFromHeader(_) => None,
        }
    }
}

pub struct BoundCompiledRoute {
    pub account_id: AccountId,
    pub environment_id: EnvironmentId,
    pub deployment_revision: DeploymentRevision,
    pub security_scheme_missing: bool,
    pub security_scheme: Option<SecuritySchemeDetails>,
    pub route: UnboundCompiledRoute,
}

pub struct CompiledRoutesForDomain {
    pub security_schemes: HashMap<SecuritySchemeId, SecuritySchemeDetails>,
    pub routes: Vec<MaybeDisabledCompiledRoute>,
}

pub struct MaybeDisabledCompiledRoute {
    pub method: HttpMethod,
    pub path: Vec<PathSegment>,
    pub body: RequestBodySchema,
    pub behavior: RouteBehaviour,
    pub security_scheme_missing: bool,
    pub security_scheme: Option<SecuritySchemeId>,
    pub cors: CorsOptions,
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
