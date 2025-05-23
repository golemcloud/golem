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

use crate::log::LogColorize;
use crate::model::api::to_method_pattern;
use crate::model::app::HttpApiDefinitionName;
use crate::model::app_raw::{
    HttpApiDefinition, HttpApiDefinitionBindingType, HttpApiDefinitionRoute,
};
use crate::model::component::Component;
use crate::model::deploy_diff::{DiffSerialize, ToYamlValueWithoutNulls};
use crate::model::text::fmt::format_rib_source_for_error;
use anyhow::anyhow;
use golem_client::model::{
    GatewayBindingComponent, GatewayBindingData, GatewayBindingType, HttpApiDefinitionRequest,
    HttpApiDefinitionResponseData, RouteRequestData,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiffableHttpApiDefinition(pub HttpApiDefinitionRequest);

impl DiffableHttpApiDefinition {
    pub fn from_server(api_definition: HttpApiDefinitionResponseData) -> anyhow::Result<Self> {
        Ok(Self(HttpApiDefinitionRequest {
            id: api_definition.id,
            version: api_definition.version,
            security: None, // TODO: check that this is not needed anymore
            routes: api_definition
                .routes
                .into_iter()
                .map(|route| RouteRequestData {
                    method: route.method,
                    path: route.path,
                    binding: GatewayBindingData {
                        binding_type: route.binding.binding_type,
                        component: route.binding.component.map(|component| {
                            GatewayBindingComponent {
                                name: component.name,
                                version: Some(component.version),
                            }
                        }),
                        worker_name: route.binding.worker_name,
                        idempotency_key: route.binding.idempotency_key,
                        response: route.binding.response,
                        invocation_context: route.binding.invocation_context,
                    },
                    security: route.security,
                })
                .collect(),
            draft: api_definition.draft,
        }))
    }

    pub fn from_manifest(
        server_api_def: Option<&DiffableHttpApiDefinition>,
        name: &HttpApiDefinitionName,
        api_definition: &HttpApiDefinition,
        latest_component_versions: &BTreeMap<String, Component>,
    ) -> anyhow::Result<Self> {
        let mut manifest_api_def = Self(HttpApiDefinitionRequest {
            id: name.to_string(),
            version: api_definition.version.clone(),
            security: None, // TODO: check that this is not needed anymore
            routes: api_definition
                .routes
                .iter()
                .map(|route| normalize_http_api_route(latest_component_versions, route))
                .collect::<Result<Vec<_>, _>>()?,
            draft: true,
        });

        // NOTE: if the only diff is being non-draft on serverside, we hide that
        if let Some(server_api_def) = server_api_def {
            if manifest_api_def.0.version == server_api_def.0.version
                && !server_api_def.0.draft
                && manifest_api_def.0.draft
            {
                manifest_api_def.0.draft = false;
            }
        }

        Ok(manifest_api_def)
    }
}

impl DiffSerialize for DiffableHttpApiDefinition {
    fn to_diffable_string(&self) -> anyhow::Result<String> {
        let yaml_value = self.0.clone().to_yaml_value_without_nulls()?;
        Ok(serde_yaml::to_string(&yaml_value)?)
    }
}

fn normalize_http_api_route(
    latest_component_versions: &BTreeMap<String, Component>,
    route: &HttpApiDefinitionRoute,
) -> anyhow::Result<RouteRequestData> {
    Ok(RouteRequestData {
        method: to_method_pattern(&route.method)?,
        path: normalize_http_api_binding_path(&route.path),
        binding: GatewayBindingData {
            binding_type: Some(
                route
                    .binding
                    .type_
                    .as_ref()
                    .map(|binding_type| match binding_type {
                        HttpApiDefinitionBindingType::Default => GatewayBindingType::Default,
                        HttpApiDefinitionBindingType::CorsPreflight => {
                            GatewayBindingType::CorsPreflight
                        }
                        HttpApiDefinitionBindingType::FileServer => GatewayBindingType::FileServer,
                        HttpApiDefinitionBindingType::HttpHandler => {
                            GatewayBindingType::HttpHandler
                        }
                    })
                    .unwrap_or_else(|| GatewayBindingType::Default),
            ),
            component: {
                route
                    .binding
                    .component_name
                    .as_ref()
                    .map(|name| GatewayBindingComponent {
                        name: name.clone(),
                        version: route.binding.component_version.or_else(|| {
                            latest_component_versions
                                .get(name)
                                .map(|component| component.versioned_component_id.version)
                        }),
                    })
            },
            worker_name: None,
            idempotency_key: normalize_rib_property(&route.binding.idempotency_key)?,
            invocation_context: normalize_rib_property(&route.binding.invocation_context)?,
            response: normalize_rib_property(&route.binding.response)?,
        },
        security: route.security.clone(),
    })
}

fn normalize_rib_property(rib: &Option<String>) -> anyhow::Result<Option<String>> {
    rib.as_ref()
        .map(|r| r.as_str())
        .map(normalize_rib_source_code)
        .transpose()
}

pub fn normalize_http_api_binding_path(path: &str) -> String {
    path.to_string()
        .strip_suffix("/")
        .unwrap_or(path)
        .to_string()
}

fn normalize_rib_source_code(rib: &str) -> anyhow::Result<String> {
    Ok(rib::from_string(rib)
        .map_err(|err| {
            anyhow!(
                "Failed to normalize Rib source code: {}\n{}\n{}",
                err,
                "Rib source:".log_color_highlight(),
                format_rib_source_for_error(&err, rib)
            )
        })?
        .to_string())
}
