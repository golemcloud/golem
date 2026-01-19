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
use crate::model::diff;
use crate::model::Empty;

pub use crate::base_model::http_api_definition::*;

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
