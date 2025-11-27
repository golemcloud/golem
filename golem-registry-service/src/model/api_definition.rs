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
use golem_common::model::Empty;
use golem_common::model::domain_registration::Domain;
use golem_common::model::http_api_definition::RouteMethod;
use golem_common::model::security_scheme::SecuritySchemeName;
use golem_service_base::custom_api::HttpCors;
use golem_service_base::custom_api::compiled_gateway_binding::{
    FileServerBindingCompiled, HttpHandlerBindingCompiled, WorkerBindingCompiled,
};
use golem_service_base::custom_api::path_pattern::AllPathPatterns;

#[derive(Debug, Clone, PartialEq, BinaryCodec)]
#[desert(evolution())]
// Compared to what the worker service is working with, this is missing auth callbacks and
// the materialized swagger api spec. Reason is that these can only be built once the security scheme and active routes
// are fully resolved at routing time.
pub enum GatewayBindingCompiled {
    HttpCorsPreflight(HttpCors),
    Worker(Box<WorkerBindingCompiled>),
    FileServer(Box<FileServerBindingCompiled>),
    HttpHandler(Box<HttpHandlerBindingCompiled>),
    SwaggerUi(Empty),
}

#[derive(Debug, Clone, PartialEq, BinaryCodec)]
#[desert(evolution())]
pub struct CompiledRoute {
    pub method: RouteMethod,
    pub path: AllPathPatterns,
    pub binding: GatewayBindingCompiled,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompiledRouteWithContext {
    pub domain: Domain,
    pub security_scheme: Option<SecuritySchemeName>,
    pub route: CompiledRoute,
}
