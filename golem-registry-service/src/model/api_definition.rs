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
use golem_common::model::domain_registration::Domain;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::http_api_definition::RouteMethod;
use golem_common::model::security_scheme::SecuritySchemeName;
use golem_service_base::custom_api::compiled_gateway_binding::GatewayBindingCompiled;
use golem_service_base::custom_api::path_pattern::AllPathPatterns;
use golem_service_base::custom_api::security_scheme::SecuritySchemeDetails;

#[derive(Debug, Clone, PartialEq, BinaryCodec)]
#[desert(evolution())]
pub struct CompiledRouteWithoutSecurity {
    pub method: RouteMethod,
    pub path: AllPathPatterns,
    pub binding: GatewayBindingCompiled,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompiledRouteWithContext {
    pub domain: Domain,
    pub security_scheme: Option<SecuritySchemeName>,
    pub route: CompiledRouteWithoutSecurity,
}

#[derive(Debug, Clone)]
pub struct CompiledRouteWithSecuritySchemeDetails {
    pub account_id: AccountId,
    pub environment_id: EnvironmentId,
    pub domain: Domain,
    pub security_scheme: Option<SecuritySchemeDetails>,
    pub route: CompiledRouteWithoutSecurity,
}
