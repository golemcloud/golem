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

use super::component::ComponentName;
use super::environment::EnvironmentId;
use super::security_scheme::SecuritySchemeName;
use crate::model::Empty;
use crate::model::{diff, validate_kebab_case_identifier};
use crate::{
    declare_enums, declare_revision, declare_structs, declare_transparent_newtypes, declare_unions,
    newtype_uuid,
};
use derive_more::Display;
use desert_rust::BinaryCodec;
use std::fmt::{Display, Formatter};
use std::str::FromStr;

newtype_uuid!(
    HttpApiDefinitionId,
    golem_api_grpc::proto::golem::apidefinition::HttpApiDefinitionId
);

declare_revision!(HttpApiDefinitionRevision);

declare_transparent_newtypes! {
    #[derive(Display, PartialOrd, Eq, Ord, Hash, BinaryCodec)]
    #[desert(transparent)]
    pub struct HttpApiDefinitionName(pub String);

    // User provided version. See HttpApiDefinitionRevision for the "proper" golem version.
    #[derive(Display, PartialOrd, Eq, Ord, BinaryCodec)]
    #[desert(transparent)]
    pub struct HttpApiDefinitionVersion(pub String);
}

impl TryFrom<&str> for HttpApiDefinitionName {
    type Error = String;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        value.to_string().try_into()
    }
}

impl TryFrom<String> for HttpApiDefinitionName {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        validate_kebab_case_identifier("HTTP API definition", &value)?;
        Ok(HttpApiDefinitionName(value.to_string()))
    }
}

impl FromStr for HttpApiDefinitionName {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::try_from(s)
    }
}

impl HttpApiDefinitionName {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

declare_enums! {
    #[derive(BinaryCodec, Hash)]
    pub enum RouteMethod {
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
}

impl Display for RouteMethod {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            RouteMethod::Get => write!(f, "GET"),
            RouteMethod::Connect => write!(f, "CONNECT"),
            RouteMethod::Post => write!(f, "POST"),
            RouteMethod::Delete => write!(f, "DELETE"),
            RouteMethod::Put => write!(f, "PUT"),
            RouteMethod::Patch => write!(f, "PATCH"),
            RouteMethod::Options => write!(f, "OPTIONS"),
            RouteMethod::Trace => write!(f, "TRACE"),
            RouteMethod::Head => write!(f, "HEAD"),
        }
    }
}

impl TryFrom<&str> for RouteMethod {
    type Error = String;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl FromStr for RouteMethod {
    type Err = String;

    fn from_str(method: &str) -> Result<Self, Self::Err> {
        match method.to_uppercase().as_str() {
            "GET" => Ok(RouteMethod::Get),
            "CONNECT" => Ok(RouteMethod::Connect),
            "POST" => Ok(RouteMethod::Post),
            "DELETE" => Ok(RouteMethod::Delete),
            "PUT" => Ok(RouteMethod::Put),
            "PATCH" => Ok(RouteMethod::Patch),
            "OPTIONS" => Ok(RouteMethod::Options),
            "TRACE" => Ok(RouteMethod::Trace),
            "HEAD" => Ok(RouteMethod::Head),
            _ => Err(format!("Illegal route method: {}", method)),
        }
    }
}

impl From<RouteMethod> for http::Method {
    fn from(value: RouteMethod) -> Self {
        match value {
            RouteMethod::Get => Self::GET,
            RouteMethod::Connect => Self::CONNECT,
            RouteMethod::Post => Self::POST,
            RouteMethod::Delete => Self::DELETE,
            RouteMethod::Put => Self::PUT,
            RouteMethod::Patch => Self::PATCH,
            RouteMethod::Options => Self::OPTIONS,
            RouteMethod::Trace => Self::TRACE,
            RouteMethod::Head => Self::HEAD,
        }
    }
}

declare_enums! {
    pub enum GatewayBindingType {
        Worker,
        FileServer,
        HttpHandler,
        CorsPreflight,
        SwaggerUi,
    }
}

declare_structs! {
    pub struct HttpApiDefinitionCreation {
        pub name: HttpApiDefinitionName,
        pub version: HttpApiDefinitionVersion,
        pub routes: Vec<HttpApiRoute>,
    }

    pub struct HttpApiDefinitionUpdate {
        pub current_revision: HttpApiDefinitionRevision,
        pub version: Option<HttpApiDefinitionVersion>,
        pub routes: Option<Vec<HttpApiRoute>>,
    }

    pub struct HttpApiDefinition {
        pub id: HttpApiDefinitionId,
        pub revision: HttpApiDefinitionRevision,
        pub environment_id: EnvironmentId,
        pub name: HttpApiDefinitionName,
        pub hash: diff::Hash,
        pub version: HttpApiDefinitionVersion,
        pub routes: Vec<HttpApiRoute>,
        pub created_at: chrono::DateTime<chrono::Utc>,
        pub updated_at: chrono::DateTime<chrono::Utc>
    }

    #[derive(BinaryCodec)]
    #[desert(evolution())]
    pub struct HttpApiRoute {
        pub method: RouteMethod,
        pub path: String,
        pub binding: GatewayBinding,
        pub security: Option<SecuritySchemeName>
    }

    #[derive(BinaryCodec)]
    #[desert(evolution())]
    pub struct WorkerGatewayBinding {
        pub component_name: ComponentName,
        pub idempotency_key: Option<String>,
        pub invocation_context: Option<String>,
        pub response: String,
    }

    #[derive(BinaryCodec)]
    #[desert(evolution())]
    pub struct FileServerBinding {
        pub component_name: ComponentName,
        pub worker_name: String,
        pub response: String,
    }

    #[derive(BinaryCodec)]
    #[desert(evolution())]
    pub struct HttpHandlerBinding {
        pub component_name: ComponentName,
        pub worker_name: String,
        pub idempotency_key: Option<String>,
        pub invocation_context: Option<String>,
        pub response: String,
    }

    #[derive(BinaryCodec)]
    #[desert(evolution())]
    pub struct CorsPreflightBinding {
        pub response: Option<String>,
    }
}

impl HttpApiDefinition {
    pub fn to_diffable(&self) -> diff::HttpApiDefinition {
        diff::HttpApiDefinition {
            routes: self
                .routes
                .iter()
                .map(|route| route.to_diffable())
                .collect(),
            version: self.version.0.clone(),
        }
    }
}

impl HttpApiRoute {
    pub fn to_diffable(&self) -> (diff::HttpApiMethodAndPath, diff::HttpApiRoute) {
        (
            diff::HttpApiMethodAndPath {
                method: self.method.to_string(),
                path: self.path.clone(),
            },
            diff::HttpApiRoute {
                binding: self.binding.to_diffable(),
                security: self.security.as_ref().map(|sec| sec.0.clone()),
            },
        )
    }
}

declare_unions! {
    #[derive(BinaryCodec)]
    pub enum GatewayBinding {
        Worker(WorkerGatewayBinding),
        FileServer(FileServerBinding),
        HttpHandler(HttpHandlerBinding),
        CorsPreflight(CorsPreflightBinding),
        SwaggerUi(Empty)
    }
}

impl GatewayBinding {
    pub fn binding_type(&self) -> GatewayBindingType {
        match self {
            GatewayBinding::Worker(_) => GatewayBindingType::Worker,
            GatewayBinding::FileServer(_) => GatewayBindingType::FileServer,
            GatewayBinding::HttpHandler(_) => GatewayBindingType::HttpHandler,
            GatewayBinding::CorsPreflight(_) => GatewayBindingType::CorsPreflight,
            GatewayBinding::SwaggerUi(_) => GatewayBindingType::SwaggerUi,
        }
    }

    pub fn component_name(&self) -> Option<&ComponentName> {
        match self {
            GatewayBinding::Worker(binding) => Some(&binding.component_name),
            GatewayBinding::FileServer(binding) => Some(&binding.component_name),
            GatewayBinding::HttpHandler(binding) => Some(&binding.component_name),
            GatewayBinding::CorsPreflight(_) => None,
            GatewayBinding::SwaggerUi(_) => None,
        }
    }

    pub fn to_diffable(&self) -> diff::HttpApiDefinitionBinding {
        match self {
            GatewayBinding::Worker(WorkerGatewayBinding {
                component_name,
                idempotency_key,
                invocation_context,
                response,
            }) => diff::HttpApiDefinitionBinding {
                binding_type: GatewayBindingType::Worker,
                component_name: Some(component_name.0.clone()),
                worker_name: None,
                idempotency_key: idempotency_key.clone(),
                invocation_context: invocation_context.clone(),
                response: Some(response.clone()),
            },
            GatewayBinding::FileServer(FileServerBinding {
                component_name,
                worker_name,
                response,
            }) => diff::HttpApiDefinitionBinding {
                binding_type: GatewayBindingType::FileServer,
                component_name: Some(component_name.0.clone()),
                worker_name: Some(worker_name.clone()),
                idempotency_key: None,
                invocation_context: None,
                response: Some(response.clone()),
            },
            GatewayBinding::HttpHandler(HttpHandlerBinding {
                component_name,
                worker_name,
                idempotency_key,
                invocation_context,
                response,
            }) => diff::HttpApiDefinitionBinding {
                binding_type: GatewayBindingType::HttpHandler,
                component_name: Some(component_name.0.clone()),
                worker_name: Some(worker_name.clone()),
                idempotency_key: idempotency_key.clone(),
                invocation_context: invocation_context.clone(),
                response: Some(response.clone()),
            },
            GatewayBinding::CorsPreflight(CorsPreflightBinding { response }) => {
                diff::HttpApiDefinitionBinding {
                    binding_type: GatewayBindingType::CorsPreflight,
                    component_name: None,
                    worker_name: None,
                    idempotency_key: None,
                    invocation_context: None,
                    response: response.clone(),
                }
            }
            GatewayBinding::SwaggerUi(Empty {}) => diff::HttpApiDefinitionBinding {
                binding_type: GatewayBindingType::SwaggerUi,
                component_name: None,
                worker_name: None,
                idempotency_key: None,
                invocation_context: None,
                response: None,
            },
        }
    }
}

mod protobuf {
    use super::RouteMethod;

    impl TryFrom<golem_api_grpc::proto::golem::apidefinition::HttpMethod> for RouteMethod {
        type Error = String;
        fn try_from(
            value: golem_api_grpc::proto::golem::apidefinition::HttpMethod,
        ) -> Result<Self, Self::Error> {
            use golem_api_grpc::proto::golem::apidefinition::HttpMethod as GrpcHttpMethod;

            match value {
                GrpcHttpMethod::Get => Ok(Self::Get),
                GrpcHttpMethod::Connect => Ok(Self::Connect),
                GrpcHttpMethod::Post => Ok(Self::Post),
                GrpcHttpMethod::Delete => Ok(Self::Delete),
                GrpcHttpMethod::Put => Ok(Self::Put),
                GrpcHttpMethod::Patch => Ok(Self::Patch),
                GrpcHttpMethod::Options => Ok(Self::Options),
                GrpcHttpMethod::Trace => Ok(Self::Trace),
                GrpcHttpMethod::Head => Ok(Self::Head),
                GrpcHttpMethod::Unspecified => Err("unkown http method".to_string()),
            }
        }
    }

    impl From<RouteMethod> for golem_api_grpc::proto::golem::apidefinition::HttpMethod {
        fn from(value: RouteMethod) -> Self {
            match value {
                RouteMethod::Get => Self::Get,
                RouteMethod::Connect => Self::Connect,
                RouteMethod::Post => Self::Post,
                RouteMethod::Delete => Self::Delete,
                RouteMethod::Put => Self::Put,
                RouteMethod::Patch => Self::Patch,
                RouteMethod::Options => Self::Options,
                RouteMethod::Trace => Self::Trace,
                RouteMethod::Head => Self::Head,
            }
        }
    }
}
