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

use crate::config::ComponentTransformerPluginCallerConfig;
use crate::model::component::Component;
use async_trait::async_trait;
use golem_common::SafeDisplay;
use golem_common::model::base64::Base64;
use golem_common::model::component::{
    ComponentFilePath, ComponentFilePermissions, ComponentType, InitialComponentFile,
};
use golem_common::model::component::{ComponentName, VersionedComponentId};
use golem_common::model::component_metadata::ComponentMetadata;
use golem_common::retries::with_retries;
use http::StatusCode;
use reqwest::multipart::{Form, Part};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::fmt::{Debug, Display, Formatter};
use tracing::debug;

#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitialComponentFileWithData {
    pub path: ComponentFilePath,
    pub permissions: ComponentFilePermissions,
    pub content: Base64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComponentTransformerResponse {
    pub data: Option<Base64>,
    pub additional_files: Option<Vec<InitialComponentFileWithData>>,
    pub env: Option<HashMap<String, String>>,
}

#[async_trait]
pub trait ComponentTransformerPluginCaller: Send + Sync {
    async fn call_remote_transformer_plugin(
        &self,
        component: &Component,
        data: &[u8],
        url: String,
        parameters: &HashMap<String, String>,
    ) -> Result<ComponentTransformerResponse, TransformationFailedReason>;
}

#[derive(Debug, thiserror::Error)]
pub enum TransformationFailedReason {
    Failure(String),
    Request(reqwest::Error),
    HttpStatus(StatusCode),
}

impl Display for TransformationFailedReason {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            TransformationFailedReason::Failure(message) => write!(f, "{message}"),
            TransformationFailedReason::Request(error) => write!(f, "Request error: {error}"),
            TransformationFailedReason::HttpStatus(status) => write!(f, "HTTP status: {status}"),
        }
    }
}

impl SafeDisplay for TransformationFailedReason {
    fn to_safe_string(&self) -> String {
        self.to_string()
    }
}

#[derive(Debug)]
pub struct ComponentTransformerPluginCallerDefault {
    client: reqwest::Client,
    config: ComponentTransformerPluginCallerConfig,
}

impl ComponentTransformerPluginCallerDefault {
    pub fn new(config: ComponentTransformerPluginCallerConfig) -> Self {
        Self {
            client: reqwest::Client::new(),
            config,
        }
    }
}

#[async_trait]
impl ComponentTransformerPluginCaller for ComponentTransformerPluginCallerDefault {
    async fn call_remote_transformer_plugin(
        &self,
        component: &Component,
        data: &[u8],
        url: String,
        parameters: &HashMap<String, String>,
    ) -> Result<ComponentTransformerResponse, TransformationFailedReason> {
        let serializable_component: SerializableComponent = component.clone().into();
        let response = with_retries(
            "component_transformer_plugin",
            "transform",
            None,
            &self.config.retries,
            &(
                self.client.clone(),
                serializable_component,
                url,
                data,
                parameters,
            ),
            |(client, serializable_component, url, data, parameters)| {
                Box::pin(async move {
                    let mut form = Form::new();
                    form = form.part("component", Part::bytes(data.to_vec()));
                    form = form.part(
                        "metadata",
                        Part::text(serde_json::to_string(&serializable_component).map_err(
                            |err| {
                                TransformationFailedReason::Failure(format!(
                                    "Failed serializing component: {err}"
                                ))
                            },
                        )?)
                        .mime_str("application/json")
                        .unwrap(),
                    );
                    for (key, value) in *parameters {
                        if key == "component" {
                            return Err(TransformationFailedReason::Failure(
                                "Parameter key 'component' is reserved".to_string(),
                            ));
                        }
                        if key == "metadata" {
                            return Err(TransformationFailedReason::Failure(
                                "Parameter key 'metadata' is reserved".to_string(),
                            ));
                        }
                        form = form.part(key.clone(), Part::text(value.clone()));
                    }

                    let request = client.post(url).multipart(form);

                    let response = request
                        .send()
                        .await
                        .map_err(TransformationFailedReason::Request)?;

                    if response.status().is_server_error() {
                        return Err(TransformationFailedReason::HttpStatus(response.status()));
                    }

                    Ok(response)
                })
            },
            |err| {
                matches!(
                    err,
                    TransformationFailedReason::HttpStatus(_)
                        | TransformationFailedReason::Request(_)
                )
            },
        )
        .await?;

        if !response.status().is_success() {
            Err(TransformationFailedReason::HttpStatus(response.status()))?
        }

        let parsed = response
            .json::<ComponentTransformerResponse>()
            .await
            .map_err(|err| {
                TransformationFailedReason::Failure(format!(
                    "Failed to read response from transformation plugin: {err}"
                ))
            })?;

        debug!(
            "Received response from component transformer plugin: {:?}",
            parsed
        );

        Ok(parsed)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct SerializableComponent {
    pub versioned_component_id: VersionedComponentId,
    pub component_name: ComponentName,
    pub component_size: u64,
    pub metadata: ComponentMetadata,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub component_type: ComponentType,
    pub files: Vec<InitialComponentFile>,
    pub env: BTreeMap<String, String>,
}

impl From<Component> for SerializableComponent {
    fn from(value: Component) -> Self {
        Self {
            versioned_component_id: value.versioned_component_id,
            component_name: value.component_name,
            component_size: value.component_size,
            metadata: value.metadata,
            created_at: value.created_at,
            component_type: value.component_type,
            files: value.files,
            env: value.env,
        }
    }
}
