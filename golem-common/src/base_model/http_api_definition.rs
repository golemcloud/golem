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

use crate::base_model::component::ComponentName;
use crate::base_model::environment::EnvironmentId;
use crate::base_model::security_scheme::SecuritySchemeName;
use crate::base_model::{diff, validate_lower_kebab_case_identifier, Empty};
use crate::{
    declare_enums, declare_revision, declare_structs, declare_transparent_newtypes, declare_unions,
    newtype_uuid,
};
use derive_more::Display;
use std::fmt::{Display, Formatter};
use std::str::FromStr;

newtype_uuid!(
    HttpApiDefinitionId,
    golem_api_grpc::proto::golem::apidefinition::HttpApiDefinitionId
);

declare_revision!(HttpApiDefinitionRevision);

declare_transparent_newtypes! {
    #[derive(Display, PartialOrd, Eq, Ord, Hash)]
    #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
    #[cfg_attr(feature = "full", desert(transparent))]
    pub struct HttpApiDefinitionName(pub String);

    // User provided version. See HttpApiDefinitionRevision for the "proper" golem version.
    #[derive(Display, PartialOrd, Eq, Ord)]
    #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
    #[cfg_attr(feature = "full", desert(transparent))]
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
        validate_lower_kebab_case_identifier("HTTP API definition", &value)?;
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
    #[derive(Hash)]
    #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
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

    #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
    #[cfg_attr(feature = "full", desert(evolution()))]
    pub struct HttpApiRoute {
        pub method: RouteMethod,
        pub path: String,
        pub binding: GatewayBinding,
        pub security: Option<SecuritySchemeName>
    }

    #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
    #[cfg_attr(feature = "full", desert(evolution()))]
    pub struct WorkerGatewayBinding {
        pub component_name: ComponentName,
        pub idempotency_key: Option<String>,
        pub invocation_context: Option<String>,
        pub response: String,
    }

    #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
    #[cfg_attr(feature = "full", desert(evolution()))]
    pub struct FileServerBinding {
        pub component_name: ComponentName,
        pub worker_name: String,
        pub response: String,
    }

    #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
    #[cfg_attr(feature = "full", desert(evolution()))]
    pub struct HttpHandlerBinding {
        pub component_name: ComponentName,
        pub worker_name: String,
        pub idempotency_key: Option<String>,
        pub invocation_context: Option<String>,
        pub response: String,
    }

    #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
    #[cfg_attr(feature = "full", desert(evolution()))]
    pub struct CorsPreflightBinding {
        pub response: Option<String>,
    }
}

declare_unions! {
    #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
    pub enum GatewayBinding {
        Worker(WorkerGatewayBinding),
        FileServer(FileServerBinding),
        HttpHandler(HttpHandlerBinding),
        CorsPreflight(CorsPreflightBinding),
        SwaggerUi(Empty)
    }
}
