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

use super::SecuritySchemeDetails;
use super::compiled_gateway_binding::{
    FileServerBindingCompiled, GatewayBindingCompiled, HttpCorsBindingCompiled,
    HttpHandlerBindingCompiled, IdempotencyKeyCompiled, InvocationContextCompiled,
    ResponseMappingCompiled, WorkerBindingCompiled, WorkerNameCompiled,
};
use super::path_pattern::AllPathPatterns;
use super::{CompiledRoute, CompiledRoutes, HttpCors};
use golem_common::model::Empty;
use golem_common::model::component::{ComponentName, ComponentRevision};
use golem_common::model::deployment::DeploymentRevision;
use golem_common::model::security_scheme::Provider;
use openidconnect::{ClientId, ClientSecret, RedirectUrl, Scope};
use std::collections::HashMap;
use std::ops::Deref;

impl TryFrom<golem_api_grpc::proto::golem::apidefinition::HttpCors> for HttpCors {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::apidefinition::HttpCors,
    ) -> Result<Self, Self::Error> {
        let allow_origin = value
            .allow_origin
            .ok_or_else(|| "allow_origin field missing".to_string())?;

        let allow_methods = value
            .allow_methods
            .ok_or_else(|| "allow_methods field missing".to_string())?;

        let allow_headers = value
            .allow_headers
            .ok_or_else(|| "allow_headers field missing".to_string())?;

        Ok(Self {
            allow_origin,
            allow_methods,
            allow_headers,
            expose_headers: value.expose_headers,
            allow_credentials: value.allow_credentials,
            max_age: value.max_age,
        })
    }
}

impl From<HttpCors> for golem_api_grpc::proto::golem::apidefinition::HttpCors {
    fn from(value: HttpCors) -> Self {
        Self {
            allow_origin: Some(value.allow_origin),
            allow_methods: Some(value.allow_methods),
            allow_headers: Some(value.allow_headers),
            expose_headers: value.expose_headers,
            max_age: value.max_age,
            allow_credentials: value.allow_credentials,
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::apidefinition::SecuritySchemeDetails>
    for SecuritySchemeDetails
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::apidefinition::SecuritySchemeDetails,
    ) -> Result<Self, Self::Error> {
        let provider_type =
            Provider::try_from(value.provider()).map_err(|e| format!("invalid provider: {}", e))?;

        let id = value
            .id
            .ok_or_else(|| "scheme_identifier field missing".to_string())?
            .try_into()?;

        let client_id = ClientId::new(value.client_id);

        let client_secret = ClientSecret::new(value.client_secret);

        let redirect_url = RedirectUrl::new(value.redirect_url)
            .map_err(|e| format!("Failed parsing redirect url: {e}"))?;

        let scopes = value.scopes.into_iter().map(Scope::new).collect::<Vec<_>>();

        Ok(Self {
            id,
            provider_type,
            client_id,
            client_secret,
            redirect_url,
            scopes,
        })
    }
}

impl From<SecuritySchemeDetails>
    for golem_api_grpc::proto::golem::apidefinition::SecuritySchemeDetails
{
    fn from(value: SecuritySchemeDetails) -> Self {
        Self {
            id: Some(value.id.into()),
            provider: golem_api_grpc::proto::golem::apidefinition::Provider::from(
                value.provider_type,
            )
            .into(),
            client_id: value.client_id.deref().clone(),
            client_secret: value.client_secret.secret().clone(),
            redirect_url: value.redirect_url.deref().clone(),
            scopes: value.scopes.iter().map(|s| s.deref().clone()).collect(),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::apidefinition::WorkerNameCompiled>
    for WorkerNameCompiled
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::apidefinition::WorkerNameCompiled,
    ) -> Result<Self, Self::Error> {
        let worker_name = value
            .expr
            .ok_or_else(|| "expr field missing".to_string())?
            .try_into()?;

        let compiled_worker_name = value
            .compiled_expr
            .ok_or_else(|| "compiled_expr field missing".to_string())?
            .try_into()?;

        let rib_input = value
            .rib_input
            .ok_or_else(|| "rib_input field missing".to_string())?
            .try_into()?;

        Ok(Self {
            worker_name,
            compiled_worker_name,
            rib_input,
        })
    }
}

impl TryFrom<WorkerNameCompiled>
    for golem_api_grpc::proto::golem::apidefinition::WorkerNameCompiled
{
    type Error = String;
    fn try_from(value: WorkerNameCompiled) -> Result<Self, Self::Error> {
        Ok(Self {
            expr: Some(value.worker_name.into()),
            compiled_expr: Some(value.compiled_worker_name.try_into()?),
            rib_input: Some(value.rib_input.into()),
        })
    }
}

impl TryFrom<golem_api_grpc::proto::golem::apidefinition::IdempotencyKeyCompiled>
    for IdempotencyKeyCompiled
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::apidefinition::IdempotencyKeyCompiled,
    ) -> Result<Self, Self::Error> {
        let idempotency_key = value
            .expr
            .ok_or_else(|| "expr field missing".to_string())?
            .try_into()?;

        let compiled_idempotency_key = value
            .compiled_expr
            .ok_or_else(|| "compiled_expr field missing".to_string())?
            .try_into()?;

        let rib_input = value
            .rib_input
            .ok_or_else(|| "rib_input field missing".to_string())?
            .try_into()?;

        Ok(Self {
            idempotency_key,
            compiled_idempotency_key,
            rib_input,
        })
    }
}

impl TryFrom<IdempotencyKeyCompiled>
    for golem_api_grpc::proto::golem::apidefinition::IdempotencyKeyCompiled
{
    type Error = String;
    fn try_from(value: IdempotencyKeyCompiled) -> Result<Self, Self::Error> {
        Ok(Self {
            expr: Some(value.idempotency_key.into()),
            compiled_expr: Some(value.compiled_idempotency_key.try_into()?),
            rib_input: Some(value.rib_input.into()),
        })
    }
}

impl TryFrom<golem_api_grpc::proto::golem::apidefinition::InvocationContextCompiled>
    for InvocationContextCompiled
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::apidefinition::InvocationContextCompiled,
    ) -> Result<Self, Self::Error> {
        let invocation_context = value
            .expr
            .ok_or_else(|| "expr field missing".to_string())?
            .try_into()?;

        let compiled_invocation_context = value
            .compiled_expr
            .ok_or_else(|| "compiled_expr field missing".to_string())?
            .try_into()?;

        let rib_input = value
            .rib_input
            .ok_or_else(|| "rib_input field missing".to_string())?
            .try_into()?;

        Ok(Self {
            invocation_context,
            compiled_invocation_context,
            rib_input,
        })
    }
}

impl TryFrom<InvocationContextCompiled>
    for golem_api_grpc::proto::golem::apidefinition::InvocationContextCompiled
{
    type Error = String;

    fn try_from(value: InvocationContextCompiled) -> Result<Self, Self::Error> {
        Ok(Self {
            expr: Some(value.invocation_context.into()),
            compiled_expr: Some(value.compiled_invocation_context.try_into()?),
            rib_input: Some(value.rib_input.into()),
        })
    }
}

impl TryFrom<golem_api_grpc::proto::golem::apidefinition::ResponseMappingCompiled>
    for ResponseMappingCompiled
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::apidefinition::ResponseMappingCompiled,
    ) -> Result<Self, Self::Error> {
        let response_mapping_expr = value
            .expr
            .ok_or_else(|| "expr field missing".to_string())?
            .try_into()?;

        let response_mapping_compiled = value
            .compiled_expr
            .ok_or_else(|| "compiled_expr field missing".to_string())?
            .try_into()?;

        let rib_input = value
            .rib_input
            .ok_or_else(|| "rib_input field missing".to_string())?
            .try_into()?;

        let worker_calls = match value.worker_functions {
            Some(w) => Some(w.try_into()?),
            None => None,
        };

        let rib_output = match value.rib_output {
            Some(o) => Some(o.try_into()?),
            None => None,
        };

        Ok(Self {
            response_mapping_expr,
            response_mapping_compiled,
            rib_input,
            worker_calls,
            rib_output,
        })
    }
}

impl TryFrom<ResponseMappingCompiled>
    for golem_api_grpc::proto::golem::apidefinition::ResponseMappingCompiled
{
    type Error = String;

    fn try_from(value: ResponseMappingCompiled) -> Result<Self, Self::Error> {
        Ok(Self {
            expr: Some(value.response_mapping_expr.into()),
            compiled_expr: Some(value.response_mapping_compiled.try_into()?),
            rib_input: Some(value.rib_input.into()),
            worker_functions: value.worker_calls.map(|wc| wc.into()),
            rib_output: value.rib_output.map(|ro| ro.into()),
        })
    }
}

impl TryFrom<golem_api_grpc::proto::golem::apidefinition::CompiledWorkerBinding>
    for WorkerBindingCompiled
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::apidefinition::CompiledWorkerBinding,
    ) -> Result<Self, Self::Error> {
        let component_id = value
            .component_id
            .ok_or_else(|| "component_id field missing".to_string())?
            .try_into()?;

        let component_revision = ComponentRevision(value.component_revision);

        let component_name = ComponentName(value.component_name);

        let idempotency_key_compiled = match value.idempotency_key {
            Some(v) => Some(v.try_into()?),
            None => None,
        };

        let invocation_context_compiled = match value.invocation_context {
            Some(v) => Some(v.try_into()?),
            None => None,
        };

        let response_compiled = value
            .response_mapping
            .ok_or_else(|| "response_mapping field missing".to_string())?
            .try_into()?;

        Ok(Self {
            component_id,
            component_name,
            component_revision,
            idempotency_key_compiled,
            invocation_context_compiled,
            response_compiled,
        })
    }
}

impl TryFrom<WorkerBindingCompiled>
    for golem_api_grpc::proto::golem::apidefinition::CompiledWorkerBinding
{
    type Error = String;

    fn try_from(value: WorkerBindingCompiled) -> Result<Self, Self::Error> {
        Ok(Self {
            component_id: Some(value.component_id.into()),
            component_revision: value.component_revision.0,
            component_name: value.component_name.0,
            idempotency_key: value
                .idempotency_key_compiled
                .map(|v| v.try_into())
                .transpose()?,
            invocation_context: value
                .invocation_context_compiled
                .map(|v| v.try_into())
                .transpose()?,
            response_mapping: Some(value.response_compiled.try_into()?),
        })
    }
}

impl TryFrom<golem_api_grpc::proto::golem::apidefinition::CompiledHttpHandlerBinding>
    for HttpHandlerBindingCompiled
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::apidefinition::CompiledHttpHandlerBinding,
    ) -> Result<Self, Self::Error> {
        let component_id = value
            .component_id
            .ok_or_else(|| "component_id field missing".to_string())?
            .try_into()?;

        let component_revision = ComponentRevision(value.component_revision);

        let component_name = ComponentName(value.component_name);

        let worker_name_compiled = value
            .worker_name_compiled
            .ok_or_else(|| "worker_name_compiled field missing".to_string())?
            .try_into()?;

        let idempotency_key_compiled = match value.idempotency_key_compiled {
            Some(v) => Some(v.try_into()?),
            None => None,
        };

        let invocation_context_compiled = match value.invocation_context_compiled {
            Some(v) => Some(v.try_into()?),
            None => None,
        };

        Ok(Self {
            component_id,
            component_name,
            component_revision,
            worker_name_compiled,
            idempotency_key_compiled,
            invocation_context_compiled,
        })
    }
}

impl TryFrom<HttpHandlerBindingCompiled>
    for golem_api_grpc::proto::golem::apidefinition::CompiledHttpHandlerBinding
{
    type Error = String;

    fn try_from(value: HttpHandlerBindingCompiled) -> Result<Self, Self::Error> {
        Ok(Self {
            component_id: Some(value.component_id.into()),
            component_revision: value.component_revision.0,
            component_name: value.component_name.0,
            worker_name_compiled: Some(value.worker_name_compiled.try_into()?),
            idempotency_key_compiled: value
                .idempotency_key_compiled
                .map(|v| v.try_into())
                .transpose()?,
            invocation_context_compiled: value
                .invocation_context_compiled
                .map(|v| v.try_into())
                .transpose()?,
        })
    }
}

impl TryFrom<golem_api_grpc::proto::golem::apidefinition::CompiledFileServerBinding>
    for FileServerBindingCompiled
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::apidefinition::CompiledFileServerBinding,
    ) -> Result<Self, Self::Error> {
        let component_id = value
            .component_id
            .ok_or_else(|| "component_id field missing".to_string())?
            .try_into()?;

        let component_revision = ComponentRevision(value.component_revision);

        let component_name = ComponentName(value.component_name);

        let worker_name_compiled = value
            .worker_name_compiled
            .ok_or_else(|| "worker_name_compiled field missing".to_string())?
            .try_into()?;

        let response_compiled = value
            .response_compiled
            .ok_or_else(|| "response_compiled field missing".to_string())?
            .try_into()?;

        Ok(Self {
            component_id,
            component_name,
            component_revision,
            worker_name_compiled,
            response_compiled,
        })
    }
}

impl TryFrom<FileServerBindingCompiled>
    for golem_api_grpc::proto::golem::apidefinition::CompiledFileServerBinding
{
    type Error = String;

    fn try_from(value: FileServerBindingCompiled) -> Result<Self, Self::Error> {
        Ok(Self {
            component_id: Some(value.component_id.into()),
            component_revision: value.component_revision.0,
            component_name: value.component_name.0,
            worker_name_compiled: Some(value.worker_name_compiled.try_into()?),
            response_compiled: Some(value.response_compiled.try_into()?),
        })
    }
}

impl TryFrom<golem_api_grpc::proto::golem::apidefinition::CompiledHttpCorsBinding>
    for HttpCorsBindingCompiled
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::apidefinition::CompiledHttpCorsBinding,
    ) -> Result<Self, Self::Error> {
        let http_cors = value
            .http_cors
            .ok_or_else(|| "http_cors field missing".to_string())?
            .try_into()?;

        Ok(HttpCorsBindingCompiled { http_cors })
    }
}

impl From<HttpCorsBindingCompiled>
    for golem_api_grpc::proto::golem::apidefinition::CompiledHttpCorsBinding
{
    fn from(value: HttpCorsBindingCompiled) -> Self {
        Self {
            http_cors: Some(value.http_cors.into()),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::apidefinition::CompiledGatewayBinding>
    for GatewayBindingCompiled
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::apidefinition::CompiledGatewayBinding,
    ) -> Result<Self, Self::Error> {
        match value.binding.ok_or_else(|| "binding field missing".to_string())? {
            golem_api_grpc::proto::golem::apidefinition::compiled_gateway_binding::Binding::WorkerBinding(v) => {
                Ok(GatewayBindingCompiled::Worker(Box::new(v.try_into()?)))
            }
            golem_api_grpc::proto::golem::apidefinition::compiled_gateway_binding::Binding::HttpHandlerBinding(v) => {
                Ok(GatewayBindingCompiled::HttpHandler(Box::new(v.try_into()?)))
            }
            golem_api_grpc::proto::golem::apidefinition::compiled_gateway_binding::Binding::FileServerBinding(v) => {
                Ok(GatewayBindingCompiled::FileServer(Box::new(v.try_into()?)))
            }
            golem_api_grpc::proto::golem::apidefinition::compiled_gateway_binding::Binding::HttpCorsBinding(v) => {
                Ok(GatewayBindingCompiled::HttpCorsPreflight(Box::new(v.try_into()?)))
            }
            golem_api_grpc::proto::golem::apidefinition::compiled_gateway_binding::Binding::SwaggerUiBinding(_) => {
                Ok(GatewayBindingCompiled::SwaggerUi(Empty { }))
            }
        }
    }
}

impl TryFrom<GatewayBindingCompiled>
    for golem_api_grpc::proto::golem::apidefinition::CompiledGatewayBinding
{
    type Error = String;

    fn try_from(value: GatewayBindingCompiled) -> Result<Self, Self::Error> {
        let binding = match value {
            GatewayBindingCompiled::Worker(b) => {
                golem_api_grpc::proto::golem::apidefinition::compiled_gateway_binding::Binding::WorkerBinding(
                    (*b).try_into()?
                )
            }
            GatewayBindingCompiled::HttpHandler(b) => {
                golem_api_grpc::proto::golem::apidefinition::compiled_gateway_binding::Binding::HttpHandlerBinding(
                    (*b).try_into()?
                )
            }
            GatewayBindingCompiled::FileServer(b) => {
                golem_api_grpc::proto::golem::apidefinition::compiled_gateway_binding::Binding::FileServerBinding(
                    (*b).try_into()?
                )
            }
            GatewayBindingCompiled::HttpCorsPreflight(b) => {
                golem_api_grpc::proto::golem::apidefinition::compiled_gateway_binding::Binding::HttpCorsBinding(
                    (*b).into()
                )
            }
            GatewayBindingCompiled::SwaggerUi(_) => {
                golem_api_grpc::proto::golem::apidefinition::compiled_gateway_binding::Binding::SwaggerUiBinding(golem_api_grpc::proto::golem::common::Empty { })
            }
        };

        Ok(Self {
            binding: Some(binding),
        })
    }
}

impl TryFrom<golem_api_grpc::proto::golem::apidefinition::CompiledHttpRoute> for CompiledRoute {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::apidefinition::CompiledHttpRoute,
    ) -> Result<Self, Self::Error> {
        let method = value.method().try_into()?;

        let path =
            AllPathPatterns::parse(&value.path).map_err(|e| format!("Failed parsing path: {e}"))?;

        let binding = value
            .binding
            .ok_or_else(|| "binding field missing".to_string())?
            .try_into()?;

        let security_scheme = match value.security_scheme_id {
            Some(id) => Some(id.try_into()?),
            None => None,
        };

        Ok(Self {
            method,
            path,
            binding,
            security_scheme,
        })
    }
}

impl TryFrom<CompiledRoute> for golem_api_grpc::proto::golem::apidefinition::CompiledHttpRoute {
    type Error = String;

    fn try_from(value: CompiledRoute) -> Result<Self, Self::Error> {
        Ok(Self {
            method: golem_api_grpc::proto::golem::apidefinition::HttpMethod::from(value.method)
                .into(),
            path: value.path.to_string(),
            binding: Some(value.binding.try_into()?),
            security_scheme_id: value.security_scheme.map(|id| id.into()),
        })
    }
}

impl TryFrom<golem_api_grpc::proto::golem::apidefinition::CompiledRoutes> for CompiledRoutes {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::apidefinition::CompiledRoutes,
    ) -> Result<Self, Self::Error> {
        let account_id = value
            .account_id
            .ok_or_else(|| "account_id field missing".to_string())?
            .try_into()?;

        let environment_id = value
            .environment_id
            .ok_or_else(|| "environment_id field missing".to_string())?
            .try_into()?;

        let deployment_revision = DeploymentRevision(value.deployment_revision);

        let security_schemes = value
            .security_schemes
            .into_iter()
            .map(|s| {
                let s_converted: SecuritySchemeDetails = s.try_into()?;
                Ok((s_converted.id, s_converted))
            })
            .collect::<Result<HashMap<_, _>, String>>()?;

        let routes = value
            .compiled_routes
            .into_iter()
            .map(|r| r.try_into())
            .collect::<Result<Vec<_>, String>>()?;

        Ok(Self {
            account_id,
            environment_id,
            deployment_revision,
            security_schemes,
            routes,
        })
    }
}

impl TryFrom<CompiledRoutes> for golem_api_grpc::proto::golem::apidefinition::CompiledRoutes {
    type Error = String;

    fn try_from(value: CompiledRoutes) -> Result<Self, Self::Error> {
        Ok(Self {
            account_id: Some(value.account_id.into()),
            environment_id: Some(value.environment_id.into()),
            deployment_revision: value.deployment_revision.0,
            security_schemes: value
                .security_schemes
                .into_values()
                .map(|v| v.into())
                .collect(),
            compiled_routes: value
                .routes
                .into_iter()
                .map(|r| r.try_into())
                .collect::<Result<Vec<_>, String>>()?,
        })
    }
}
