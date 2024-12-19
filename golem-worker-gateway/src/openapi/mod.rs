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

mod converter;
mod schema;
mod swagger_ui;

use golem_api_grpc::proto::golem::apidefinition::{
    ApiDefinition, CompiledGatewayBinding, CompiledHttpApiDefinition, CompiledHttpRoute, CorsPreflight,
    GatewayBindingType, HttpMethod, Middleware, SecurityWithProviderMetadata,
};
use openapiv3::{OpenAPI, PathItem, Operation, Parameter, ReferenceOr, SecurityScheme};
use std::collections::HashMap;

pub use converter::ApiDefinitionConverter;
pub use swagger_ui::SwaggerUiHandler;

/// Error type for OpenAPI conversion operations
#[derive(Debug, thiserror::Error)]
pub enum OpenApiError {
    #[error("Failed to convert API Definition to OpenAPI: {0}")]
    ConversionError(String),
    #[error("Invalid type: {0}")]
    InvalidType(String),
    #[error("Serialization error: {0}")]
    SerializationError(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, OpenApiError>;
