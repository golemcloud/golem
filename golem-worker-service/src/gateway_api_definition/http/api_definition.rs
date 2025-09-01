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

use crate::gateway_api_definition::http::path_pattern_parser::parse_path_pattern;
use crate::gateway_api_definition::http::{
    HttpApiDefinitionRequest, OpenApiHttpApiDefinition, RouteRequest,
};
use crate::gateway_api_definition::{ApiDefinitionId, ApiVersion, HasGolemBindings};
use crate::gateway_api_definition_transformer::transform_http_api_definition;
use crate::gateway_binding::SwaggerUiBinding;
use crate::gateway_binding::{
    FileServerBindingCompiled, GatewayBinding, GatewayBindingCompiled, IdempotencyKeyCompiled,
    InvocationContextCompiled, ResponseMappingCompiled, StaticBinding, WorkerNameCompiled,
};
use crate::gateway_binding::{HttpHandlerBindingCompiled, WorkerBindingCompiled};
use crate::gateway_middleware::{
    HttpAuthenticationMiddleware, HttpCors, HttpMiddleware, HttpMiddlewares,
};
use crate::gateway_security::SecuritySchemeReference;
use crate::service::gateway::api_definition::ApiDefinitionError;
use crate::service::gateway::api_definition_validator::ValidationErrors;
use crate::service::gateway::security_scheme::SecuritySchemeService;
use crate::service::gateway::BoxConversionContext;
use bincode::{Decode, Encode};
use golem_common::model::auth::Namespace;
use golem_common::model::component::VersionedComponentId;
use golem_service_base::model::Component;
use golem_wasm_ast::analysis::{AnalysedExport, AnalysedType};
use poem_openapi::Enum;
use rib::{ComponentDependency, ComponentDependencyKey, RibCompilationError, RibInputTypeInfo};
use serde::de::Error;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fmt::{Debug, Display, Formatter};
use std::ops::Deref;
use std::str::FromStr;
use std::sync::Arc;
use Iterator;

#[derive(Debug, Clone, PartialEq)]
pub struct HttpApiDefinition {
    pub id: ApiDefinitionId,
    pub version: ApiVersion,
    pub routes: Vec<Route>,
    pub draft: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl HttpApiDefinition {
    pub fn id(&self) -> ApiDefinitionId {
        self.id.clone()
    }

    pub fn version(&self) -> ApiVersion {
        self.version.clone()
    }
    pub fn security_schemes(&self) -> Vec<SecuritySchemeReference> {
        self.routes
            .iter()
            .filter_map(|route| {
                route
                    .middlewares
                    .clone()
                    .and_then(|x| x.get_http_authentication_middleware())
            })
            .map(|x| SecuritySchemeReference::from(x.security_scheme_with_metadata))
            .collect()
    }

    pub async fn from_http_api_definition_request(
        namespace: &Namespace,
        request: HttpApiDefinitionRequest,
        created_at: chrono::DateTime<chrono::Utc>,
        security_scheme_service: &Arc<dyn SecuritySchemeService>,
    ) -> Result<Self, ApiDefinitionError> {
        let mut registry = HashMap::new();

        let security_schemes_in_definition = request
            .routes
            .iter()
            .filter_map(|route| {
                route.security.as_ref().map(|security_scheme_reference| {
                    security_scheme_reference.security_scheme_identifier.clone()
                })
            })
            .collect::<HashSet<_>>();

        for security_scheme_identifier in security_schemes_in_definition {
            let security_scheme = security_scheme_service
                .get(&security_scheme_identifier, namespace)
                .await
                .map_err(ApiDefinitionError::SecuritySchemeError)?;

            registry.insert(security_scheme_identifier, security_scheme);
        }

        let mut routes = vec![];

        for route in request.routes {
            let mut http_middlewares = vec![];

            if let Some(security) = &route.security {
                let security_scheme = security_scheme_service
                    .get(&security.security_scheme_identifier, namespace)
                    .await
                    .map_err(ApiDefinitionError::SecuritySchemeError)?;

                http_middlewares.push(HttpMiddleware::authenticate_request(security_scheme));
            }

            routes.push(Route {
                method: route.method,
                path: route.path,
                middlewares: if http_middlewares.is_empty() {
                    None
                } else {
                    Some(HttpMiddlewares(http_middlewares))
                },

                binding: route.binding,
            })
        }

        let mut http_api_definition = HttpApiDefinition {
            id: request.id,
            version: request.version,
            routes,
            draft: request.draft,
            created_at,
        };

        transform_http_api_definition(&mut http_api_definition).map_err(|error| {
            ApiDefinitionError::ValidationError(ValidationErrors {
                errors: vec![error.to_string()],
            })
        })?;

        Ok(http_api_definition)
    }
}

impl From<HttpApiDefinition> for HttpApiDefinitionRequest {
    fn from(value: HttpApiDefinition) -> Self {
        Self {
            id: value.id(),
            version: value.version(),
            routes: value.routes.into_iter().map(RouteRequest::from).collect(),
            draft: value.draft,
        }
    }
}

impl HasGolemBindings for HttpApiDefinition {
    fn get_bindings(&self) -> Vec<GatewayBinding> {
        self.routes
            .iter()
            .map(|route| route.binding.clone())
            .collect()
    }
}

impl From<CompiledHttpApiDefinition> for HttpApiDefinition {
    fn from(compiled_http_api_definition: CompiledHttpApiDefinition) -> Self {
        Self {
            id: compiled_http_api_definition.id,
            version: compiled_http_api_definition.version,
            routes: compiled_http_api_definition
                .routes
                .into_iter()
                .map(Route::from)
                .collect(),
            draft: compiled_http_api_definition.draft,
            created_at: compiled_http_api_definition.created_at,
        }
    }
}

// The Rib Expressions that exists in various parts of HttpApiDefinition (mainly in Routes)
// are compiled to form CompiledHttpApiDefinition.
// The Compilation happens during API definition registration,
// and is persisted, so that custom http requests are served by looking up
// CompiledHttpApiDefinition
#[derive(Debug, Clone, PartialEq)]
pub struct CompiledHttpApiDefinition {
    pub id: ApiDefinitionId,
    pub version: ApiVersion,
    pub routes: Vec<CompiledRoute>,
    pub draft: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub namespace: Namespace,
}

impl CompiledHttpApiDefinition {
    pub fn remove_auth_call_back_routes(
        &self,
        auth_routes: &[CompiledAuthCallBackRoute],
    ) -> CompiledHttpApiDefinition {
        let new_routes = self
            .routes
            .iter()
            .filter(|route| {
                route
                    .as_auth_callback_route()
                    .map(|auth_route| !auth_routes.contains(&auth_route))
                    .unwrap_or(true)
            })
            .cloned()
            .collect::<Vec<_>>();

        CompiledHttpApiDefinition {
            id: self.id.clone(),
            version: self.version.clone(),
            routes: new_routes,
            draft: self.draft,
            created_at: self.created_at,
            namespace: self.namespace.clone(),
        }
    }

    pub fn from_http_api_definition(
        http_api_definition: &HttpApiDefinition,
        metadata_dictionary: &ComponentMetadataDictionary,
        namespace: &Namespace,
        conversion_context: &BoxConversionContext<'_>,
    ) -> Result<Self, RouteCompilationErrors> {
        let mut compiled_routes = vec![];

        for route in &http_api_definition.routes {
            let compiled_route = CompiledRoute::from_route(route, metadata_dictionary)?;
            compiled_routes.push(compiled_route);
        }

        let mut result = CompiledHttpApiDefinition {
            id: http_api_definition.id.clone(),
            version: http_api_definition.version.clone(),
            routes: compiled_routes,
            draft: http_api_definition.draft,
            created_at: http_api_definition.created_at,
            namespace: namespace.clone(),
        };
        // Update SwaggerUI routes with actual OpenAPI spec
        result.update_swagger_ui_openapi_specs(conversion_context);
        Ok(result)
    }

    pub fn update_swagger_ui_openapi_specs(
        &mut self,
        conversion_context: &BoxConversionContext<'_>,
    ) {
        let openapi_spec_json = self.generate_openapi_spec_json(conversion_context).ok();

        // Update all SwaggerUI routes with the actual OpenAPI spec
        for route in &mut self.routes {
            if let crate::gateway_binding::GatewayBindingCompiled::SwaggerUi(swagger_binding) =
                &mut route.binding
            {
                swagger_binding.openapi_spec_json = openapi_spec_json.clone();
            }
        }
    }

    /// Generates the OpenAPI specification JSON for this compiled API definition
    fn generate_openapi_spec_json(
        &self,
        conversion_context: &BoxConversionContext<'_>,
    ) -> Result<String, String> {
        let openapi = futures::executor::block_on(
            OpenApiHttpApiDefinition::from_compiled_http_api_definition(self, conversion_context),
        )?;
        serde_json::to_string_pretty(&openapi.0)
            .map_err(|e| format!("Failed to serialize OpenAPI to JSON: {e}"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Encode, Decode, Enum)]
pub enum MethodPattern {
    Get,
    Connect,
    Post,
    Delete,
    Put,
    Patch,
    Options,
    Trace,
    Head,
}

impl MethodPattern {
    pub fn is_connect(&self) -> bool {
        matches!(self, MethodPattern::Connect)
    }

    pub fn is_delete(&self) -> bool {
        matches!(self, MethodPattern::Delete)
    }

    pub fn is_get(&self) -> bool {
        matches!(self, MethodPattern::Get)
    }

    pub fn is_head(&self) -> bool {
        matches!(self, MethodPattern::Head)
    }
    pub fn is_post(&self) -> bool {
        matches!(self, MethodPattern::Post)
    }

    pub fn is_put(&self) -> bool {
        matches!(self, MethodPattern::Put)
    }

    pub fn is_options(&self) -> bool {
        matches!(self, MethodPattern::Options)
    }

    pub fn is_patch(&self) -> bool {
        matches!(self, MethodPattern::Patch)
    }

    pub fn is_trace(&self) -> bool {
        matches!(self, MethodPattern::Trace)
    }
}

impl Display for MethodPattern {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            MethodPattern::Get => write!(f, "GET"),
            MethodPattern::Connect => write!(f, "CONNECT"),
            MethodPattern::Post => write!(f, "POST"),
            MethodPattern::Delete => {
                write!(f, "DELETE")
            }
            MethodPattern::Put => write!(f, "PUT"),
            MethodPattern::Patch => write!(f, "PATCH"),
            MethodPattern::Options => write!(f, "OPTIONS"),
            MethodPattern::Trace => write!(f, "TRACE"),
            MethodPattern::Head => write!(f, "HEAD"),
        }
    }
}

impl FromStr for MethodPattern {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "get" => Ok(MethodPattern::Get),
            "connect" => Ok(MethodPattern::Connect),
            "post" => Ok(MethodPattern::Post),
            "delete" => Ok(MethodPattern::Delete),
            "put" => Ok(MethodPattern::Put),
            "patch" => Ok(MethodPattern::Patch),
            "options" => Ok(MethodPattern::Options),
            "trace" => Ok(MethodPattern::Trace),
            "head" => Ok(MethodPattern::Head),
            _ => Err(format!("Failed to parse method '{s}'")),
        }
    }
}

impl TryFrom<i32> for MethodPattern {
    type Error = String;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(MethodPattern::Get),
            1 => Ok(MethodPattern::Connect),
            2 => Ok(MethodPattern::Post),
            3 => Ok(MethodPattern::Delete),
            4 => Ok(MethodPattern::Put),
            5 => Ok(MethodPattern::Patch),
            6 => Ok(MethodPattern::Options),
            7 => Ok(MethodPattern::Trace),
            8 => Ok(MethodPattern::Head),
            _ => Err(format!("Failed to parse numeric MethodPattern '{value}'")),
        }
    }
}

impl From<MethodPattern> for hyper::http::Method {
    fn from(method: MethodPattern) -> Self {
        match method {
            MethodPattern::Get => hyper::http::Method::GET,
            MethodPattern::Connect => hyper::http::Method::CONNECT,
            MethodPattern::Post => hyper::http::Method::POST,
            MethodPattern::Delete => hyper::http::Method::DELETE,
            MethodPattern::Put => hyper::http::Method::PUT,
            MethodPattern::Patch => hyper::http::Method::PATCH,
            MethodPattern::Options => hyper::http::Method::OPTIONS,
            MethodPattern::Trace => hyper::http::Method::TRACE,
            MethodPattern::Head => hyper::http::Method::HEAD,
        }
    }
}

impl Serialize for MethodPattern {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.to_string().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for MethodPattern {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        MethodPattern::from_str(&String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Encode, Decode)]
pub struct LiteralInfo(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Encode, Decode)]
pub struct VarInfo {
    pub key_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Encode, Decode)]
pub struct QueryInfo {
    pub key_name: String,
}

impl Display for QueryInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{{{}}}", self.key_name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Encode, Decode)]
pub struct AllPathPatterns {
    pub path_patterns: Vec<PathPattern>,
    pub query_params: Vec<QueryInfo>,
}

impl AllPathPatterns {
    pub fn parse(input: &str) -> Result<AllPathPatterns, String> {
        input.parse()
    }
}

impl Display for AllPathPatterns {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for pattern in self.path_patterns.iter() {
            write!(f, "/")?;
            write!(f, "{pattern}")?;
        }

        if !self.query_params.is_empty() {
            write!(f, "?")?;
            for (index, query) in self.query_params.iter().enumerate() {
                if index > 0 {
                    write!(f, "&")?;
                }
                write!(f, "{query}")?;
            }
        }

        Ok(())
    }
}

impl FromStr for AllPathPatterns {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        parse_path_pattern(s)
            .map_err(|err| err.to_string())
            .and_then(|(leftover, result)| {
                if !leftover.is_empty() {
                    Err("Failed to parse path".to_string())
                } else {
                    Ok(result)
                }
            })
    }
}

impl<'de> Deserialize<'de> for AllPathPatterns {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;

        match value {
            Value::String(value) => match AllPathPatterns::parse(value.as_str()) {
                Ok(path_pattern) => Ok(path_pattern),
                Err(message) => Err(serde::de::Error::custom(message.to_string())),
            },

            _ => Err(serde::de::Error::custom("Failed to parse path from yaml")),
        }
    }
}

impl Serialize for AllPathPatterns {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let value = Value::String(self.to_string());
        Value::serialize(&value, serializer)
    }
}

/// Invariant: PathPattern::CatchAllVar is only allowed at the end of the path
#[derive(Debug, Clone, PartialEq, Eq, Hash, Encode, Decode)]
pub enum PathPattern {
    Literal(LiteralInfo),
    Var(VarInfo),
    CatchAllVar(VarInfo),
}

impl PathPattern {
    pub fn literal(value: impl Into<String>) -> PathPattern {
        PathPattern::Literal(LiteralInfo(value.into()))
    }

    pub fn var(value: impl Into<String>) -> PathPattern {
        PathPattern::Var(VarInfo {
            key_name: value.into(),
        })
    }

    pub fn catch_all_var(value: impl Into<String>) -> PathPattern {
        PathPattern::CatchAllVar(VarInfo {
            key_name: value.into(),
        })
    }
}

impl Display for PathPattern {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PathPattern::Literal(info) => write!(f, "{}", info.0),
            PathPattern::Var(info) => write!(f, "{{{}}}", info.key_name),
            PathPattern::CatchAllVar(info) => write!(f, "{{+{}}}", info.key_name),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Route {
    pub method: MethodPattern,
    pub path: AllPathPatterns,
    pub middlewares: Option<HttpMiddlewares>,
    pub binding: GatewayBinding,
}

impl Route {
    pub fn cors_preflight_binding(&self) -> Option<HttpCors> {
        match &self.binding {
            GatewayBinding::Static(static_binding) => static_binding.get_cors_preflight(),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompiledRoute {
    pub method: MethodPattern,
    pub path: AllPathPatterns,
    pub binding: GatewayBindingCompiled,
    pub middlewares: Option<HttpMiddlewares>,
}

impl CompiledRoute {
    pub fn as_auth_callback_route(&self) -> Option<CompiledAuthCallBackRoute> {
        match &self.binding {
            GatewayBindingCompiled::Static(StaticBinding::HttpAuthCallBack(auth_callback)) => {
                Some(CompiledAuthCallBackRoute {
                    method: self.method.clone(),
                    path: self.path.clone(),
                    http_auth_middleware: auth_callback.deref().clone(),
                })
            }

            _ => None,
        }
    }
}

impl TryFrom<CompiledRoute> for golem_api_grpc::proto::golem::apidefinition::CompiledHttpRoute {
    type Error = String;

    fn try_from(value: CompiledRoute) -> Result<Self, Self::Error> {
        let method = value.method as i32;
        let path = value.path.to_string();
        let binding =
            golem_api_grpc::proto::golem::apidefinition::CompiledGatewayBinding::try_from(
                value.binding,
            )?;

        let middleware_proto = value
            .middlewares
            .map(golem_api_grpc::proto::golem::apidefinition::Middleware::try_from)
            .transpose()?;

        Ok(Self {
            method,
            path,
            binding: Some(binding),
            middleware: middleware_proto,
        })
    }
}

impl TryFrom<golem_api_grpc::proto::golem::apidefinition::CompiledHttpRoute> for CompiledRoute {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::apidefinition::CompiledHttpRoute,
    ) -> Result<Self, Self::Error> {
        let method = MethodPattern::try_from(value.method)?;
        let path = AllPathPatterns::parse(value.path.as_str()).map_err(|e| e.to_string())?;
        let binding_proto = value.binding.ok_or("binding is missing")?;
        let binding = GatewayBindingCompiled::try_from(binding_proto)?;
        let middlewares = value
            .middleware
            .map(HttpMiddlewares::try_from)
            .transpose()?;

        Ok(CompiledRoute {
            method,
            path,
            binding,
            middlewares,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompiledAuthCallBackRoute {
    pub method: MethodPattern,
    pub path: AllPathPatterns,
    pub http_auth_middleware: HttpAuthenticationMiddleware,
}

#[derive(Debug, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum RouteCompilationErrors {
    MetadataNotFoundError(VersionedComponentId),
    RibError(RibCompilationError),
    ValidationError(ValidationErrors),
}

#[derive(Clone, Debug)]
pub struct ComponentMetadataDictionary {
    pub metadata: HashMap<VersionedComponentId, ComponentDetails>,
}

#[derive(Clone, Debug)]
pub struct ComponentDetails {
    pub component_info: ComponentDependencyKey,
    pub metadata: Vec<AnalysedExport>,
}

impl ComponentMetadataDictionary {
    pub fn from_components(components: &Vec<Component>) -> ComponentMetadataDictionary {
        let mut metadata = HashMap::new();

        for component in components {
            let component_info = ComponentDependencyKey {
                component_name: component.component_name.0.clone(),
                component_id: component.versioned_component_id.component_id.0,
                root_package_name: component.metadata.root_package_name().clone(),
                root_package_version: component.metadata.root_package_version().clone(),
            };

            let component_details = ComponentDetails {
                component_info,
                metadata: component.metadata.exports().to_vec(),
            };

            metadata.insert(component.versioned_component_id.clone(), component_details);
        }

        ComponentMetadataDictionary { metadata }
    }
}

impl CompiledRoute {
    pub fn get_security_middleware(&self) -> Option<HttpAuthenticationMiddleware> {
        match &self.middlewares {
            Some(middlewares) => middlewares.get_http_authentication_middleware(),
            None => None,
        }
    }
    pub fn from_route(
        route: &Route,
        metadata_dictionary: &ComponentMetadataDictionary,
    ) -> Result<CompiledRoute, RouteCompilationErrors> {
        let query_params = route.path.query_params.as_ref();
        let path_params = route
            .path
            .path_patterns
            .iter()
            .filter_map(|pattern| match pattern {
                PathPattern::Var(var) => Some(var.key_name.as_str()),
                PathPattern::CatchAllVar(var) => Some(var.key_name.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();

        match &route.binding {
            GatewayBinding::Default(worker_binding) => {
                let component_details = metadata_dictionary
                    .metadata
                    .get(&worker_binding.component_id)
                    .ok_or(RouteCompilationErrors::MetadataNotFoundError(
                        worker_binding.component_id.clone(),
                    ))?;

                let component_dependency = vec![ComponentDependency::new(
                    component_details.component_info.clone(),
                    component_details.metadata.clone(),
                )];

                let binding = WorkerBindingCompiled::from_raw_worker_binding(
                    worker_binding,
                    &component_dependency,
                )
                .map_err(RouteCompilationErrors::RibError)?;

                Self::validate_rib_scripts(
                    query_params,
                    &path_params,
                    None,
                    binding.invocation_context_compiled.as_ref(),
                    binding.idempotency_key_compiled.as_ref(),
                    Some(&binding.response_compiled),
                )?;

                Ok(CompiledRoute {
                    method: route.method.clone(),
                    path: route.path.clone(),
                    binding: GatewayBindingCompiled::Worker(Box::new(binding)),
                    middlewares: route.middlewares.clone(),
                })
            }

            GatewayBinding::FileServer(worker_binding) => {
                let component_details = metadata_dictionary
                    .metadata
                    .get(&worker_binding.component_id)
                    .ok_or(RouteCompilationErrors::MetadataNotFoundError(
                        worker_binding.component_id.clone(),
                    ))?;

                let component_dependency = vec![ComponentDependency::new(
                    component_details.component_info.clone(),
                    component_details.metadata.clone(),
                )];

                let binding = FileServerBindingCompiled::from_raw_file_server_worker_binding(
                    worker_binding,
                    &component_dependency,
                )
                .map_err(RouteCompilationErrors::RibError)?;

                Self::validate_rib_scripts(
                    query_params,
                    &path_params,
                    binding.worker_name_compiled.as_ref(),
                    binding.invocation_context_compiled.as_ref(),
                    binding.idempotency_key_compiled.as_ref(),
                    Some(&binding.response_compiled),
                )?;

                Ok(CompiledRoute {
                    method: route.method.clone(),
                    path: route.path.clone(),
                    binding: GatewayBindingCompiled::FileServer(Box::new(binding)),
                    middlewares: route.middlewares.clone(),
                })
            }

            GatewayBinding::HttpHandler(http_handler_binding) => {
                let binding =
                    HttpHandlerBindingCompiled::from_raw_http_handler_binding(http_handler_binding)
                        .map_err(RouteCompilationErrors::RibError)?;

                Self::validate_rib_scripts(
                    query_params,
                    &path_params,
                    binding.worker_name_compiled.as_ref(),
                    None,
                    binding.idempotency_key_compiled.as_ref(),
                    None,
                )?;

                Ok(CompiledRoute {
                    method: route.method.clone(),
                    path: route.path.clone(),
                    binding: GatewayBindingCompiled::HttpHandler(Box::new(binding)),
                    middlewares: route.middlewares.clone(),
                })
            }

            GatewayBinding::Static(static_binding) => Ok(CompiledRoute {
                method: route.method.clone(),
                path: route.path.clone(),
                binding: GatewayBindingCompiled::Static(static_binding.clone()),
                middlewares: route.middlewares.clone(),
            }),

            GatewayBinding::SwaggerUi(_) => {
                // SwaggerUI binding starts with None - will be populated after full compilation
                Ok(CompiledRoute {
                    method: route.method.clone(),
                    path: route.path.clone(),
                    binding: GatewayBindingCompiled::SwaggerUi(SwaggerUiBinding::new()),
                    middlewares: route.middlewares.clone(),
                })
            }
        }
    }

    // Validate the Rib script that can exist
    // in worker name, invocation context, idempotency key and response mapping
    // to check if the query and path params lookups are actually in the API route
    fn validate_rib_scripts(
        api_query_params: &[QueryInfo],
        path_params: &[&str],
        worker_name_compiled: Option<&WorkerNameCompiled>,
        invocation_context_compiled: Option<&InvocationContextCompiled>,
        idempotency_key_compiled: Option<&IdempotencyKeyCompiled>,
        response_mapping: Option<&ResponseMappingCompiled>,
    ) -> Result<(), RouteCompilationErrors> {
        let mut validation_errors = vec![];
        if let Some(worker_name_compiled) = worker_name_compiled {
            let input_type_info = &worker_name_compiled.rib_input_type_info;
            let invalid_query_params =
                Self::find_invalid_query_keys_in_rib(api_query_params, input_type_info);

            if !invalid_query_params.is_empty() {
                validation_errors.push(
                    format!(
                        "Following request.query lookups in worker name rib script is not present in API route: {}",
                        invalid_query_params.join(", ")
                    )
                );
            }

            let invalid_path_params =
                Self::find_invalid_path_keys_in_rib(path_params, input_type_info);

            if !invalid_path_params.is_empty() {
                validation_errors.push(
                    format!(
                        "Following request.path lookups in worker name rib script is not present in API route: {}",
                        invalid_path_params.join(", ")
                    )
                );
            }
        }

        if let Some(invocation_context_compiled) = invocation_context_compiled {
            let input_type_info = &invocation_context_compiled.rib_input;
            let invalid_query_params =
                Self::find_invalid_query_keys_in_rib(api_query_params, input_type_info);

            if !invalid_query_params.is_empty() {
                validation_errors.push(
                    format!(
                        "Following request.query lookups in invocation context rib script is not present in API route: {}",
                        invalid_query_params.join(", ")
                    )
                );
            }

            let invalid_path_params =
                Self::find_invalid_path_keys_in_rib(path_params, input_type_info);

            if !invalid_path_params.is_empty() {
                validation_errors.push(
                    format!(
                        "Following request.path lookups in invocation context rib script is not present in API route: {}",
                        invalid_path_params.join(", ")
                    )
                );
            }
        }

        if let Some(idempotency_key_compiled) = idempotency_key_compiled {
            let input_type_info = &idempotency_key_compiled.rib_input;
            let invalid_query_params =
                Self::find_invalid_query_keys_in_rib(api_query_params, input_type_info);

            if !invalid_query_params.is_empty() {
                validation_errors.push(
                    format!(
                        "Following request.query lookups in idempotency key rib script is not present in API route: {}",
                        invalid_query_params.join(", ")
                    )
                );
            }

            let invalid_path_params =
                Self::find_invalid_path_keys_in_rib(path_params, input_type_info);

            if !invalid_path_params.is_empty() {
                validation_errors.push(
                    format!(
                        "Following request.path lookups in idempotency key rib script is not present in API route: {}",
                        invalid_path_params.join(", ")
                    )
                );
            }
        }

        if let Some(response_mapping) = response_mapping {
            let input_type_info = &response_mapping.rib_input;
            let invalid_query_params =
                Self::find_invalid_query_keys_in_rib(api_query_params, input_type_info);

            if !invalid_query_params.is_empty() {
                validation_errors.push(
                    format!(
                        "Following request.query lookups in response mapping rib script is not present in API route: {}",
                        invalid_query_params.join(", ")
                    )
                );
            }

            let invalid_path_params =
                Self::find_invalid_path_keys_in_rib(path_params, input_type_info);

            if !invalid_path_params.is_empty() {
                validation_errors.push(
                    format!(
                        "Following request.path lookups in response mapping rib script is not present in API route: {}",
                        invalid_path_params.join(", ")
                    )
                );
            }
        }

        if !validation_errors.is_empty() {
            Err(RouteCompilationErrors::ValidationError(ValidationErrors {
                errors: validation_errors,
            }))
        } else {
            Ok(())
        }
    }

    // Find all query param lookups in rib script that are not defined in the path pattern
    fn find_invalid_query_keys_in_rib<'a>(
        input_query_params: &[QueryInfo],
        rib_input_type_info: &'a RibInputTypeInfo,
    ) -> Vec<&'a str> {
        let api_query_keys = input_query_params
            .iter()
            .map(|query| query.key_name.as_str())
            .collect::<Vec<_>>();

        let rib_query_keys = Self::get_request_lookups_in_rib(rib_input_type_info, "query");

        // find request.query lookups in Rib that are not in actual API
        rib_query_keys
            .into_iter()
            .filter(|&rib_query_key| !api_query_keys.contains(&rib_query_key))
            .collect()
    }

    fn find_invalid_path_keys_in_rib<'a>(
        path_params: &[&'a str],
        rib_input_type_info: &'a RibInputTypeInfo,
    ) -> Vec<&'a str> {
        let rib_path_keys = Self::get_request_lookups_in_rib(rib_input_type_info, "path");

        // find request.path lookups in Rib that are not in actual API
        rib_path_keys
            .into_iter()
            .filter(|rib_path_key| !path_params.contains(rib_path_key))
            .collect()
    }

    // Find all keys under `request.x` where x can be `path` or `query`
    // which is part of the rib script
    fn get_request_lookups_in_rib<'a>(
        rib_input_type_info: &'a RibInputTypeInfo,
        key_name: &'a str,
    ) -> Vec<&'a str> {
        // get path params from rib_input_type info
        let rib_query_params = rib_input_type_info.get("request");

        if let Some(rib_path_params) = rib_query_params {
            match rib_path_params {
                AnalysedType::Record(type_record) => type_record
                    .fields
                    .iter()
                    .flat_map(|field| {
                        if field.name == key_name {
                            let typ = &field.typ;
                            match typ {
                                AnalysedType::Record(type_record) => {
                                    type_record.fields.iter().map(|x| x.name.as_str()).collect()
                                }
                                _ => vec![],
                            }
                        } else {
                            vec![]
                        }
                    })
                    .collect::<Vec<_>>(),

                _ => vec![],
            }
        } else {
            vec![]
        }
    }
}

impl From<CompiledRoute> for Route {
    fn from(compiled_route: CompiledRoute) -> Self {
        Route {
            method: compiled_route.method,
            path: compiled_route.path,
            binding: GatewayBinding::from(compiled_route.binding),
            middlewares: compiled_route.middlewares,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use test_r::test;

    #[test]
    fn split_path_works_with_single_value() {
        let path_pattern = "/foo";
        let result = AllPathPatterns::parse(path_pattern);

        let expected = AllPathPatterns {
            path_patterns: vec![PathPattern::literal("foo")],
            query_params: vec![],
        };

        assert_eq!(result, Ok(expected));
    }

    #[test]
    fn split_path_works_with_multiple_values() {
        let path_pattern = "/foo/bar";
        let result = AllPathPatterns::parse(path_pattern);

        let expected = AllPathPatterns {
            path_patterns: vec![PathPattern::literal("foo"), PathPattern::literal("bar")],
            query_params: vec![],
        };

        assert_eq!(result, Ok(expected));
    }

    #[test]
    fn split_path_works_with_variables() {
        let path_pattern = "/foo/bar/{var}";
        let result = AllPathPatterns::parse(path_pattern);

        let expected = AllPathPatterns {
            path_patterns: vec![
                PathPattern::literal("foo"),
                PathPattern::literal("bar"),
                PathPattern::var("var"),
            ],
            query_params: vec![],
        };

        assert_eq!(result, Ok(expected));
    }

    #[test]
    fn split_path_works_with_variables_and_queries() {
        let path_pattern = "/foo/bar/{var}?{userid1}&{userid2}";
        let result = AllPathPatterns::parse(path_pattern);

        let expected = AllPathPatterns {
            path_patterns: vec![
                PathPattern::literal("foo"),
                PathPattern::literal("bar"),
                PathPattern::var("var"),
            ],
            query_params: vec![
                QueryInfo {
                    key_name: "userid1".to_string(),
                },
                QueryInfo {
                    key_name: "userid2".to_string(),
                },
            ],
        };

        assert_eq!(result, Ok(expected));
    }

    #[track_caller]
    fn test_path_pattern_to_string(path_pattern_str: &str) {
        let path_pattern = AllPathPatterns::parse(path_pattern_str).unwrap();
        let path_pattern_str_result = path_pattern.to_string();
        assert_eq!(
            path_pattern_str_result,
            path_pattern_str,
            "Assertion failed for test case at {}",
            std::panic::Location::caller()
        );
    }

    #[test]
    fn test_path_patterns_to_string() {
        test_path_pattern_to_string("/foo/bar/{var1}/{var2}?{userid1}&{userid2}");
        test_path_pattern_to_string("/foo/bar/{var1}/{var2}?{userid1}");
        test_path_pattern_to_string("/foo/bar/{var1}/{var2}");
        test_path_pattern_to_string("/foo/bar");
    }

    #[track_caller]
    fn test_string_expr_parse_and_encode(input: &str) {
        let parsed_expr1 = rib::from_string(input).unwrap();
        let encoded_expr = parsed_expr1.to_string();
        let parsed_expr2 = rib::from_string(encoded_expr.as_str()).unwrap();

        assert_eq!(
            parsed_expr1,
            parsed_expr2,
            "Assertion failed for test case at {}",
            std::panic::Location::caller()
        );
    }

    #[test]
    fn expr_parser_without_vars() {
        test_string_expr_parse_and_encode("foo");
    }

    #[test]
    fn expr_parser_with_vars() {
        test_string_expr_parse_and_encode("\"worker-id-${request.path.user_id}\"");
    }

    #[test]
    fn expression_with_predicate0() {
        test_string_expr_parse_and_encode("1<2");
    }

    #[test]
    fn expression_with_predicate1() {
        test_string_expr_parse_and_encode("request.path.user-id>request.path.id");
    }

    #[test]
    fn expression_with_predicate2() {
        test_string_expr_parse_and_encode("request.path.user-id>2");
    }

    #[test]
    fn expression_with_predicate3() {
        test_string_expr_parse_and_encode("request.path.user-id==2");
    }

    #[test]
    fn expression_with_predicate4() {
        test_string_expr_parse_and_encode("request.path.user-id<2");
    }

    #[test]
    fn expr_with_if_condition() {
        test_string_expr_parse_and_encode("if request.path.user_id>1 then 1 else 0");
    }

    #[test]
    fn expr_with_if_condition_with_expr_left() {
        test_string_expr_parse_and_encode(
            "if request.path.user_id>1 then request.path.user_id else 0",
        );
    }

    #[test]
    fn expr_with_if_condition_with_expr_left_right() {
        test_string_expr_parse_and_encode(
            "if request.path.user_id>1 then request.path.user_id else request.path.id",
        );
    }

    #[test]
    fn expr_with_if_condition_with_expr_right() {
        test_string_expr_parse_and_encode("if request.path.user_id>1 then 0 else request.path.id");
    }

    #[test]
    fn expr_with_if_condition_with_with_literals() {
        test_string_expr_parse_and_encode(
            "\"foo-${if request.path.user_id>1 then request.path.user_id else 0}\"",
        );
    }

    #[test]
    fn expr_request() {
        test_string_expr_parse_and_encode("request");
    }

    #[test]
    fn expr_worker_response() {
        test_string_expr_parse_and_encode("worker.response");
    }
}
