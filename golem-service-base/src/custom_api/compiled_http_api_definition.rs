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

use super::compiled_gateway_binding::GatewayBindingCompiled;
use super::http_middlewares::HttpMiddlewares;
use super::path_pattern::AllPathPatterns;
use desert_rust::BinaryCodec;
use golem_common::model::account::AccountId;
use golem_common::model::api_definition::{ApiDefinitionId, ApiDefinitionRevision};
use golem_common::model::environment::EnvironmentId;
use poem_openapi::Enum;
use serde::de::Error;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt::{Debug, Display, Formatter};
use std::str::FromStr;

// The Rib Expressions that exists in various parts of HttpApiDefinition (mainly in Routes)
// are compiled to form CompiledHttpApiDefinition.
// The Compilation happens during API definition registration,
// and is persisted, so that custom http requests are served by looking up
// CompiledHttpApiDefinition
#[derive(Debug, Clone, PartialEq)]
pub struct CompiledHttpApiDefinition {
    pub id: ApiDefinitionId,
    pub revision: ApiDefinitionRevision,
    pub routes: Vec<CompiledRoute>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub account_id: AccountId,
    pub environment_id: EnvironmentId,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, BinaryCodec, Enum)]
#[desert(evolution())]
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

impl From<MethodPattern> for http::Method {
    fn from(value: MethodPattern) -> Self {
        match value {
            MethodPattern::Get => Self::GET,
            MethodPattern::Connect => Self::CONNECT,
            MethodPattern::Post => Self::POST,
            MethodPattern::Delete => Self::DELETE,
            MethodPattern::Put => Self::PUT,
            MethodPattern::Patch => Self::PATCH,
            MethodPattern::Options => Self::OPTIONS,
            MethodPattern::Trace => Self::TRACE,
            MethodPattern::Head => Self::HEAD,
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

#[derive(Debug, Clone, PartialEq)]
pub struct CompiledRoute {
    pub method: MethodPattern,
    pub path: AllPathPatterns,
    pub binding: GatewayBindingCompiled,
    pub middlewares: Option<HttpMiddlewares>,
}
