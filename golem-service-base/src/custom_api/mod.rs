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

pub mod openapi;
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
    pub http_api_definition_id: HttpApiDefinitionId,
    pub method: RouteMethod,
    pub path: AllPathPatterns,
    pub binding: GatewayBindingCompiled,
    pub security_scheme: Option<SecuritySchemeId>,
}

#[derive(Debug, Clone, PartialEq, BinaryCodec)]
#[desert(evolution())]
pub enum GatewayBindingCompiled {
    HttpCorsPreflight(Box<HttpCorsBindingCompiled>),
    Worker(Box<WorkerBindingCompiled>),
    FileServer(Box<FileServerBindingCompiled>),
    HttpHandler(Box<HttpHandlerBindingCompiled>),
    SwaggerUi(Box<SwaggerUiBindingCompiled>),
}

#[derive(Debug, Clone, PartialEq, BinaryCodec)]
#[desert(evolution())]
pub struct WorkerBindingCompiled {
    pub component_id: ComponentId,
    pub component_name: ComponentName,
    pub component_revision: ComponentRevision,
    pub idempotency_key_compiled: Option<IdempotencyKeyCompiled>,
    pub invocation_context_compiled: Option<InvocationContextCompiled>,
    pub response_compiled: ResponseMappingCompiled,
}

#[derive(Debug, Clone, PartialEq, BinaryCodec)]
#[desert(evolution())]
pub struct FileServerBindingCompiled {
    pub component_id: ComponentId,
    pub component_name: ComponentName,
    pub component_revision: ComponentRevision,
    pub worker_name_compiled: WorkerNameCompiled,
    pub response_compiled: ResponseMappingCompiled,
}

#[derive(Debug, Clone, PartialEq, BinaryCodec)]
#[desert(evolution())]
pub struct HttpHandlerBindingCompiled {
    pub component_id: ComponentId,
    pub component_name: ComponentName,
    pub component_revision: ComponentRevision,
    pub worker_name_compiled: WorkerNameCompiled,
    pub idempotency_key_compiled: Option<IdempotencyKeyCompiled>,
    pub invocation_context_compiled: Option<InvocationContextCompiled>,
}

#[derive(Debug, Clone, PartialEq, BinaryCodec)]
#[desert(evolution())]
pub struct HttpCorsBindingCompiled {
    pub http_cors: HttpCors,
}

#[derive(Debug, Clone, PartialEq, BinaryCodec)]
#[desert(evolution())]
pub struct SwaggerUiBindingCompiled {
    pub http_api_definition_id: HttpApiDefinitionId,
    pub http_api_definition_name: HttpApiDefinitionName,
    pub http_api_definition_version: HttpApiDefinitionVersion,
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

#[derive(Debug, Clone, PartialEq, BinaryCodec)]
#[desert(evolution())]
pub struct ResponseMappingCompiled {
    pub response_mapping_compiled: RibByteCode,
    pub rib_input: RibInputTypeInfo,
    pub worker_calls: Option<WorkerFunctionsInRib>,
    pub rib_output: Option<RibOutputTypeInfo>,
}

impl ResponseMappingCompiled {
    pub fn from_expr(
        expr: &Expr,
        component_dependency: &[ComponentDependencyWithAgentInfo],
    ) -> Result<Self, RibCompilationError> {
        let response_compiled = compile_rib(expr, component_dependency)?;

        Ok(ResponseMappingCompiled {
            response_mapping_compiled: response_compiled.byte_code,
            rib_input: response_compiled.rib_input_type_info,
            worker_calls: response_compiled.worker_invoke_calls,
            rib_output: response_compiled.rib_output_type_info,
        })
    }
}

#[derive(Debug, Clone, PartialEq, BinaryCodec)]
#[desert(evolution())]
pub struct WorkerNameCompiled {
    pub compiled_worker_name: RibByteCode,
    pub rib_input: RibInputTypeInfo,
}

impl WorkerNameCompiled {
    pub fn from_expr(expr: &Expr) -> Result<Self, RibCompilationError> {
        let compiled_worker_name = compile_rib(expr, &[])?;

        Ok(WorkerNameCompiled {
            compiled_worker_name: compiled_worker_name.byte_code,
            rib_input: compiled_worker_name.rib_input_type_info,
        })
    }
}

#[derive(Debug, Clone, PartialEq, BinaryCodec)]
#[desert(evolution())]
pub struct IdempotencyKeyCompiled {
    pub compiled_idempotency_key: RibByteCode,
    pub rib_input: RibInputTypeInfo,
}

impl IdempotencyKeyCompiled {
    pub fn from_expr(expr: &Expr) -> Result<Self, RibCompilationError> {
        let idempotency_key_compiled = compile_rib(expr, &[])?;

        Ok(IdempotencyKeyCompiled {
            compiled_idempotency_key: idempotency_key_compiled.byte_code,
            rib_input: idempotency_key_compiled.rib_input_type_info,
        })
    }
}

#[derive(Debug, Clone, PartialEq, BinaryCodec)]
#[desert(evolution())]
pub struct InvocationContextCompiled {
    pub compiled_invocation_context: RibByteCode,
    pub rib_input: RibInputTypeInfo,
}

impl InvocationContextCompiled {
    pub fn from_expr(expr: &Expr) -> Result<Self, RibCompilationError> {
        let invocation_context_compiled = compile_rib(expr, &[])?;

        Ok(InvocationContextCompiled {
            compiled_invocation_context: invocation_context_compiled.byte_code,
            rib_input: invocation_context_compiled.rib_input_type_info,
        })
    }
}
