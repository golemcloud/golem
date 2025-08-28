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

pub mod cors;
pub mod oas;
pub mod security_scheme;

use self::cors::CorsConfiguration;
use super::component::ComponentRevision;
use super::ComponentId;
use crate::api::api_definition::GatewayBindingType;
use crate::{declare_enums, declare_structs, declare_transparent_newtypes, newtype_uuid};
use std::fmt::{Display, Formatter};
use std::str::FromStr;

newtype_uuid!(ApiDefinitionId);

declare_transparent_newtypes! {
    pub struct ApiDefinitionRevision(pub u64);
}

declare_enums! {
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

declare_structs! {
    pub struct GatewayBindingComponent {
        name: String,
        /// Version of the component. If not provided the latest version is used.
        /// Note that the version is only used to typecheck the various rib scripts and prevent component updates.
        /// During runtime, the actual version of the worker or the latest version (in case no worker was found) is used.
        version: Option<u64>,
    }

    pub struct ResolvedGatewayBindingComponent {
        name: String,
        id: ComponentId,
        revision: ComponentRevision,
    }

    pub struct GatewayBindingData {
        pub binding_type: Option<GatewayBindingType>, // descriminator to keep backward compatibility
        // For binding type - worker/default and file-server
        // Optional only to keep backward compatibility
        pub component: Option<GatewayBindingComponent>,
        // worker-name is optional to keep backward compatibility
        // this is not required anymore with first class worker support in rib
        // which is embedded in response field
        pub worker_name: Option<String>,
        // For binding type - worker/default
        pub idempotency_key: Option<String>,
        // For binding type - worker/default and fileserver, this is required
        // For binding type cors-preflight, this is optional otherwise default cors-preflight settings
        // is used
        pub response: Option<String>,
        // For binding type - worker/default
        pub invocation_context: Option<String>,
    }


    pub struct Middlewares {
        pub cors: Option<CorsConfiguration>,
        pub auth: Option<SecuritySchemeReferenceData>,
    }

    pub struct SecuritySchemeReferenceData {
        security_scheme: String,
        // Additional scope support can go in future
    }
}
