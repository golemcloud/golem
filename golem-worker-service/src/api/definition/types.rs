// types.rs
// Copyright 2024 Golem Cloud
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

use serde::{Deserialize, Serialize};
use golem_wasm_ast::analysis::AnalysedType;

/// Base binding types for the API Gateway
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum BindingType {
    Default {
        input_type: AnalysedType,
        output_type: AnalysedType,
        function_name: String,
    },
    FileServer {
        root_dir: String,
    },
    SwaggerUI {
        doc_url: String,
    },
    Http, // Consider removing if not used
    Proxy, // Consider removing if not used
}

impl std::fmt::Display for BindingType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BindingType::Default {
                input_type,
                output_type,
                function_name,
            } => write!(f, "Default({:?}, {:?}, {})", input_type, output_type, function_name),
            BindingType::FileServer { root_dir } => write!(f, "FileServer({})", root_dir),
            BindingType::SwaggerUI { doc_url } => write!(f, "SwaggerUI({})", doc_url),
            _ => write!(f, "{:?}", self),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiDefinition {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub routes: Vec<Route>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Route {
    pub path: String,
    pub method: HttpMethod,
    pub description: String,
    pub component_name: String,
    pub binding: BindingType,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "UPPERCASE")] // In OpenAPI, methods use to be uppercase
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Delete,
    Patch,
    Head,
    Options,
}

impl std::fmt::Display for HttpMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HttpMethod::Get => write!(f, "GET"),
            HttpMethod::Post => write!(f, "POST"),
            HttpMethod::Put => write!(f, "PUT"),
            HttpMethod::Delete => write!(f, "DELETE"),
            HttpMethod::Patch => write!(f, "PATCH"),
            HttpMethod::Head => write!(f, "HEAD"),
            HttpMethod::Options => write!(f, "OPTIONS"),
        }
    }
}
