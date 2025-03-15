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

use crate::config::PluginTransformationsConfig;
use crate::model::Component;
use async_trait::async_trait;
use golem_common::model::component::ComponentOwner;
use golem_common::retries::with_retries;
use golem_common::SafeDisplay;
use http::StatusCode;
use reqwest::multipart::{Form, Part};
use std::collections::HashMap;
use std::fmt::{Debug, Display, Formatter};

#[async_trait]
pub trait TransformerPluginCaller<Owner: ComponentOwner>: Debug + Send + Sync {
    async fn call_remote_transformer_plugin(
        &self,
        component: &Component<Owner>,
        data: &[u8],
        url: String,
        parameters: &HashMap<String, String>,
    ) -> Result<Vec<u8>, TransformationFailedReason>;
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
pub struct TransformerPluginCallerDefault {
    client: reqwest::Client,
    config: PluginTransformationsConfig,
}

impl TransformerPluginCallerDefault {
    pub fn new(config: PluginTransformationsConfig) -> Self {
        Self {
            client: reqwest::Client::new(),
            config,
        }
    }
}

#[async_trait]
impl<Owner: ComponentOwner> TransformerPluginCaller<Owner> for TransformerPluginCallerDefault {
    async fn call_remote_transformer_plugin(
        &self,
        component: &Component<Owner>,
        data: &[u8],
        url: String,
        parameters: &HashMap<String, String>,
    ) -> Result<Vec<u8>, TransformationFailedReason> {
        let serializable_component: golem_service_base::model::Component = component.clone().into();
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

        let body = response.bytes().await.map_err(|err| {
            TransformationFailedReason::Failure(format!(
                "Failed to read response from transformation plugin: {}",
                err
            ))
        })?;

        Ok(body.to_vec())
    }
}
