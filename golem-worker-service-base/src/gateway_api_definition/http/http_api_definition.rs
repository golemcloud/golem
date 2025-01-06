// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::gateway_api_definition::http::path_pattern_parser::parse_path_pattern;
use crate::gateway_api_definition::http::{HttpApiDefinitionRequest, RouteRequest};
use crate::gateway_api_definition::{ApiDefinitionId, ApiVersion, HasGolemBindings};
use crate::gateway_api_definition_transformer::transform_http_api_definition;
use crate::gateway_binding::WorkerBindingCompiled;
use crate::gateway_binding::{GatewayBinding, GatewayBindingCompiled};
use crate::gateway_middleware::{
    HttpAuthenticationMiddleware, HttpCors, HttpMiddleware, HttpMiddlewares,
};
use crate::gateway_security::SecuritySchemeReference;
use crate::service::gateway::api_definition::ApiDefinitionError;
use crate::service::gateway::api_definition_validator::ValidationErrors;
use crate::service::gateway::security_scheme::SecuritySchemeService;
use bincode::{Decode, Encode};
use golem_api_grpc::proto::golem::apidefinition as grpc_apidefinition;
use golem_api_grpc::proto::golem::apidefinition::HttpRoute;
use golem_service_base::model::{Component, VersionedComponentId};
use golem_wasm_ast::analysis::AnalysedExport;
use poem_openapi::Enum;
use serde::{Deserialize, Serialize, Serializer};
use serde_json::Value;
use std::collections::HashMap;
use std::fmt::{Debug, Display};
use std::str::FromStr;
use std::sync::Arc;
use std::time::SystemTime;
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

    pub async fn from_http_api_definition_request<Namespace>(
        namespace: &Namespace,
        request: HttpApiDefinitionRequest,
        created_at: chrono::DateTime<chrono::Utc>,
        security_scheme_service: &Arc<dyn SecuritySchemeService<Namespace> + Send + Sync>,
    ) -> Result<Self, ApiDefinitionError> {
        let mut registry = HashMap::new();

        if let Some(security_schemes) = request.security {
            for security_scheme_reference in security_schemes {
                let security_scheme = security_scheme_service
                    .get(
                        &security_scheme_reference.security_scheme_identifier,
                        namespace,
                    )
                    .await
                    .map_err(ApiDefinitionError::SecuritySchemeError)?;

                registry.insert(
                    security_scheme_reference.security_scheme_identifier.clone(),
                    security_scheme.clone(),
                );
            }
        }

        let mut routes = vec![];

        for route in request.routes {
            let mut http_middlewares = vec![];

            if let Some(security) = route.security {
                let security_scheme = security_scheme_service
                    .get(&security.security_scheme_identifier, namespace)
                    .await
                    .map_err(ApiDefinitionError::SecuritySchemeError)?;

                http_middlewares.push(HttpMiddleware::authenticate_request(security_scheme));
            }

            if let Some(cors) = route.cors {
                http_middlewares.push(HttpMiddleware::cors(cors));
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
        let global_security = value.security_schemes();
        let security = if global_security.is_empty() {
            None
        } else {
            Some(global_security)
        };

        Self {
            id: value.id(),
            version: value.version(),
            security,
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

impl<Namespace> From<CompiledHttpApiDefinition<Namespace>> for HttpApiDefinition {
    fn from(compiled_http_api_definition: CompiledHttpApiDefinition<Namespace>) -> Self {
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

impl TryFrom<grpc_apidefinition::ApiDefinition>
    for crate::gateway_api_definition::http::HttpApiDefinition
{
    type Error = String;
    fn try_from(value: grpc_apidefinition::ApiDefinition) -> Result<Self, Self::Error> {
        let routes = match value.definition.ok_or("definition is missing")? {
            grpc_apidefinition::api_definition::Definition::Http(http) => http
                .routes
                .into_iter()
                .map(crate::gateway_api_definition::http::Route::try_from)
                .collect::<Result<Vec<crate::gateway_api_definition::http::Route>, String>>()?,
        };
        let id = value.id.ok_or("Api Definition ID is missing")?;
        let created_at = value
            .created_at
            .ok_or("Created At is missing")
            .and_then(|t| SystemTime::try_from(t).map_err(|_| "Failed to convert timestamp"))?;
        let result = crate::gateway_api_definition::http::HttpApiDefinition {
            id: ApiDefinitionId(id.value),
            version: ApiVersion(value.version),
            routes,
            draft: value.draft,
            created_at: created_at.into(),
        };
        Ok(result)
    }
}

// The Rib Expressions that exists in various parts of HttpApiDefinition (mainly in Routes)
// are compiled to form CompiledHttpApiDefinition.
// The Compilation happens during API definition registration,
// and is persisted, so that custom http requests are served by looking up
// CompiledHttpApiDefinition
#[derive(Debug, Clone, PartialEq)]
pub struct CompiledHttpApiDefinition<Namespace> {
    pub id: ApiDefinitionId,
    pub version: ApiVersion,
    pub routes: Vec<CompiledRoute>,
    pub draft: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub namespace: Namespace,
}

impl<Namespace: Clone> CompiledHttpApiDefinition<Namespace> {
    pub fn from_http_api_definition(
        http_api_definition: &HttpApiDefinition,
        metadata_dictionary: &ComponentMetadataDictionary,
        namespace: &Namespace,
    ) -> Result<Self, RouteCompilationErrors> {
        let mut compiled_routes = vec![];

        for route in &http_api_definition.routes {
            let compiled_route = CompiledRoute::from_route(route, metadata_dictionary)?;
            compiled_routes.push(compiled_route);
        }

        Ok(CompiledHttpApiDefinition {
            id: http_api_definition.id.clone(),
            version: http_api_definition.version.clone(),
            routes: compiled_routes,
            draft: http_api_definition.draft,
            created_at: http_api_definition.created_at,
            namespace: namespace.clone(),
        })
    }
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    derive_more::Display,
    Encode,
    Decode,
    Enum,
)]
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

impl FromStr for MethodPattern {
    type Err = &'static str;

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
            _ => Err("Failed to parse method"),
        }
    }
}

impl TryFrom<i32> for MethodPattern {
    type Error = &'static str;

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
            _ => Err("Failed to parse MethodPattern"),
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

impl TryFrom<HttpRoute> for Route {
    type Error = String;

    fn try_from(http_route: HttpRoute) -> Result<Self, Self::Error> {
        let binding = http_route.binding.ok_or("Missing binding")?;
        let middlewares = http_route
            .middleware
            .map(HttpMiddlewares::try_from)
            .transpose()?;

        Ok(Route {
            method: MethodPattern::try_from(http_route.method)?,
            path: AllPathPatterns::from_str(http_route.path.as_str())?,
            binding: GatewayBinding::try_from(binding)?,
            middlewares,
        })
    }
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

#[derive(Debug)]
pub enum RouteCompilationErrors {
    MetadataNotFoundError(VersionedComponentId),
    RibCompilationError(String),
}

#[derive(Clone, Debug)]
pub struct ComponentMetadataDictionary {
    pub metadata: HashMap<VersionedComponentId, Vec<AnalysedExport>>,
}

impl ComponentMetadataDictionary {
    pub fn from_components(components: &Vec<Component>) -> ComponentMetadataDictionary {
        let mut metadata = HashMap::new();
        for component in components {
            metadata.insert(
                component.versioned_component_id.clone(),
                component.metadata.exports.clone(),
            );
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
        match &route.binding {
            GatewayBinding::Default(worker_binding) => {
                let metadata = metadata_dictionary
                    .metadata
                    .get(&worker_binding.component_id)
                    .ok_or(RouteCompilationErrors::MetadataNotFoundError(
                        worker_binding.component_id.clone(),
                    ))?;

                let binding =
                    WorkerBindingCompiled::from_raw_worker_binding(worker_binding, metadata)
                        .map_err(RouteCompilationErrors::RibCompilationError)?;

                Ok(CompiledRoute {
                    method: route.method.clone(),
                    path: route.path.clone(),
                    binding: GatewayBindingCompiled::Worker(binding),
                    middlewares: route.middlewares.clone(),
                })
            }

            GatewayBinding::FileServer(worker_binding) => {
                let metadata = metadata_dictionary
                    .metadata
                    .get(&worker_binding.component_id)
                    .ok_or(RouteCompilationErrors::MetadataNotFoundError(
                        worker_binding.component_id.clone(),
                    ))?;

                let binding =
                    WorkerBindingCompiled::from_raw_worker_binding(worker_binding, metadata)
                        .map_err(RouteCompilationErrors::RibCompilationError)?;

                Ok(CompiledRoute {
                    method: route.method.clone(),
                    path: route.path.clone(),
                    binding: GatewayBindingCompiled::FileServer(binding),
                    middlewares: route.middlewares.clone(),
                })
            }

            GatewayBinding::Static(static_binding) => Ok(CompiledRoute {
                method: route.method.clone(),
                path: route.path.clone(),
                binding: GatewayBindingCompiled::Static(static_binding.clone()),
                middlewares: route.middlewares.clone(),
            }),
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
    use crate::api;
    use async_trait::async_trait;

    use crate::gateway_security::{
        SecurityScheme, SecuritySchemeIdentifier, SecuritySchemeWithProviderMetadata,
    };
    use crate::service::gateway::security_scheme::SecuritySchemeServiceError;
    use chrono::{DateTime, Utc};
    use golem_service_base::auth::DefaultNamespace;
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

    // TODO; Avoid having to pass null to fix tests
    fn get_api_spec(
        path_pattern: &str,
        worker_id: &str,
        response_mapping: &str,
    ) -> serde_yaml::Value {
        let yaml_string = format!(
            r#"
          id: users-api
          version: 0.0.1
          projectId: '15d70aa5-2e23-4ee3-b65c-4e1d702836a3'
          createdAt: 2024-08-21T07:42:15.696Z
          routes:
          - method: Get
            path: {}
            binding:
              componentId:
                version: 0
                componentId: '15d70aa5-2e23-4ee3-b65c-4e1d702836a3'
              workerName: '{}'
              response: '{}'

        "#,
            path_pattern, worker_id, response_mapping
        );

        let de = serde_yaml::Deserializer::from_str(yaml_string.as_str());
        serde_yaml::Value::deserialize(de).unwrap()
    }

    struct TestSecuritySchemeService;

    #[async_trait]
    impl<Namespace> SecuritySchemeService<Namespace> for TestSecuritySchemeService {
        async fn get(
            &self,
            _security_scheme_name: &SecuritySchemeIdentifier,
            _namespace: &Namespace,
        ) -> Result<SecuritySchemeWithProviderMetadata, SecuritySchemeServiceError> {
            Err(SecuritySchemeServiceError::InternalError(
                "Not implemented".to_string(),
            ))
        }

        async fn create(
            &self,
            _namespace: &Namespace,
            _security_scheme: &SecurityScheme,
        ) -> Result<SecuritySchemeWithProviderMetadata, SecuritySchemeServiceError> {
            Err(SecuritySchemeServiceError::InternalError(
                "Not implemented".to_string(),
            ))
        }
    }

    #[test]
    async fn test_api_spec_proto_conversion() {
        async fn test_encode_decode(path_pattern: &str, worker_id: &str, response_mapping: &str) {
            let security_scheme_service: Arc<
                dyn SecuritySchemeService<DefaultNamespace> + Send + Sync,
            > = Arc::new(TestSecuritySchemeService);

            let yaml = get_api_spec(path_pattern, worker_id, response_mapping);
            let api_http_definition_request: api::HttpApiDefinitionRequest =
                serde_yaml::from_value(yaml.clone()).unwrap();
            let core_http_definition_request: HttpApiDefinitionRequest =
                api_http_definition_request.try_into().unwrap();
            let timestamp: DateTime<Utc> = "2024-08-21T07:42:15.696Z".parse().unwrap();
            let core_http_definition = HttpApiDefinition::from_http_api_definition_request(
                &DefaultNamespace(),
                core_http_definition_request,
                timestamp,
                &security_scheme_service,
            )
            .await
            .unwrap();
            let proto: grpc_apidefinition::ApiDefinition =
                core_http_definition.clone().try_into().unwrap();
            let decoded: HttpApiDefinition = proto.try_into().unwrap();
            assert_eq!(core_http_definition, decoded);
        }
        test_encode_decode(
            "/foo/{user-id}",
            "let x: string = request.path.user-id; \"shopping-cart-${if x>100 then 0 else 1}\"",
            "${ let result = golem:it/api.{do-something}(request.body); {status: if result.user == \"admin\" then 401 else 200 } }",
        ).await;
        test_encode_decode(
            "/foo/{user-id}",
            "let x: string = request.path.user-id; \"shopping-cart-${if x>100 then 0 else 1}\"",
            "${ let result = golem:it/api.{do-something}(request.body.foo); {status: if result.user == \"admin\" then 401 else 200 } }",
        ).await;
        test_encode_decode(
            "/foo/{user-id}",
            "let x: string = request.path.user-id; \"shopping-cart-${if x>100 then 0 else 1}\"",
            "${ let result = golem:it/api.{do-something}(request.path.user-id); {status: if result.user == \"admin\" then 401 else 200 } }",
        ).await;
        test_encode_decode(
            "/foo",
            "let x: string = request.body.user-id; \"shopping-cart-${if x>100 then 0 else 1}\"",
            "${ let result = golem:it/api.{do-something}(\"foo\"); {status: if result.user == \"admin\" then 401 else 200 } }",
        ).await;
    }
}
