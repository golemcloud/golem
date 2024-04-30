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

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use aws_config::BehaviorVersion;
use aws_sdk_s3 as s3;
use aws_sdk_s3::error::SdkError;
use golem_common::model::ComponentId;
use golem_common::retries::with_retries;
use s3::config::Region;
use s3::error::DisplayErrorContext;
use s3::operation::get_object::GetObjectError::NoSuchKey;
use s3::primitives::ByteStream;
use tracing::info;
use wasmtime::component::Component;

use crate::error::GolemError;
use crate::services::golem_config::{
    CompiledComponentServiceConfig, CompiledComponentServiceS3Config,
};
use crate::Engine;

/// Service for storing compiled native binaries of WebAssembly components
#[async_trait]
pub trait CompiledComponentService {
    async fn get(
        &self,
        component_id: &ComponentId,
        component_version: u64,
        engine: &Engine,
    ) -> Result<Option<Component>, GolemError>;
    async fn put(
        &self,
        component_id: &ComponentId,
        component_version: u64,
        component: &Component,
    ) -> Result<(), GolemError>;
}

pub async fn configured(
    config: &CompiledComponentServiceConfig,
) -> Arc<dyn CompiledComponentService + Send + Sync> {
    match config {
        CompiledComponentServiceConfig::S3(config) => {
            let region = config.region.clone();

            let sdk_config_base =
                aws_config::defaults(BehaviorVersion::v2023_11_09()).region(Region::new(region));

            let sdk_config = if let Some(endpoint_url) = &config.aws_endpoint_url {
                info!(
                    "The AWS endpoint urls for compiled component service is {}",
                    &endpoint_url
                );
                sdk_config_base.endpoint_url(endpoint_url).load().await
            } else {
                sdk_config_base.load().await
            };

            Arc::new(CompiledComponentServiceS3 {
                config: config.clone(),
                client: s3::Client::new(&sdk_config),
            })
        }
        CompiledComponentServiceConfig::Local(config) => {
            Arc::new(CompiledComponentServiceLocalFileSystem::new(&config.root))
        }
        CompiledComponentServiceConfig::Disabled(_) => {
            Arc::new(CompiledComponentServiceDisabled::new())
        }
    }
}

pub struct CompiledComponentServiceS3 {
    config: CompiledComponentServiceS3Config,
    client: s3::Client,
}

#[async_trait]
impl CompiledComponentService for CompiledComponentServiceS3 {
    async fn get(
        &self,
        component_id: &ComponentId,
        component_version: u64,
        engine: &Engine,
    ) -> Result<Option<Component>, GolemError> {
        let prefix = if self.config.object_prefix.is_empty() {
            "".to_string()
        } else {
            format!("{}/", self.config.object_prefix)
        };
        let key = format!("{}{}#{}:compiled", prefix, component_id, component_version);
        let description = format!("Downloading compiled component from {}", key);
        let result = with_retries(
            &description,
            "golem-compiled-components",
            "get-object",
            &self.config.retries,
            &(self.client.clone(), self.config.bucket.clone(), key.clone()),
            |(client, bucket, key)| {
                Box::pin(async move { client.get_object().bucket(bucket).key(key).send().await })
            },
            Self::get_object_error_is_retriable,
        )
        .await;
        match result {
            Ok(response) => {
                let body = response.body;
                let aggregated_bytes = body.collect().await.map_err(|err| {
                    GolemError::component_download_failed(
                        component_id.clone(),
                        component_version,
                        format!(
                            "Could not download compiled component: {}",
                            DisplayErrorContext(&err)
                        ),
                    )
                })?;
                let bytes = aggregated_bytes.into_bytes();
                let component = unsafe {
                    Component::deserialize(engine, &bytes).map_err(|err| {
                        GolemError::component_download_failed(
                            component_id.clone(),
                            component_version,
                            format!("Could not deserialize compiled component: {}", err),
                        )
                    })?
                };
                Ok(Some(component))
            }
            Err(SdkError::ServiceError(service_error)) => match service_error.err() {
                NoSuchKey(_) => Ok(None),
                err => Err(GolemError::component_download_failed(
                    component_id.clone(),
                    component_version,
                    format!(
                        "Could not download compiled component: {}",
                        DisplayErrorContext(&err)
                    ),
                )),
            },
            Err(err) => Err(GolemError::component_download_failed(
                component_id.clone(),
                component_version,
                format!(
                    "Could not download compiled component: {}",
                    DisplayErrorContext(&err)
                ),
            )),
        }
    }

    async fn put(
        &self,
        component_id: &ComponentId,
        component_version: u64,
        component: &Component,
    ) -> Result<(), GolemError> {
        let prefix = if self.config.object_prefix.is_empty() {
            "".to_string()
        } else {
            format!("{}/", self.config.object_prefix)
        };

        let key = format!("{}{}#{}:compiled", prefix, component_id, component_version);

        let bytes = component.serialize().unwrap();

        with_retries(
            "Uploading compiled component",
            "golem-compiled-components",
            "put-object",
            &self.config.retries,
            &(
                self.client.clone(),
                self.config.bucket.clone(),
                key.clone(),
                bytes.clone(),
            ),
            |(client, bucket, key, bytes)| {
                Box::pin(async move {
                    client
                        .put_object()
                        .bucket(bucket)
                        .key(key)
                        .body(ByteStream::from(bytes.clone()))
                        .send()
                        .await
                })
            },
            Self::put_object_error_is_retriable,
        )
        .await
        .map(|_| ())
        .map_err(|err| {
            GolemError::component_download_failed(
                component_id.clone(),
                component_version,
                format!(
                    "Could not upload compiled component: {}",
                    DisplayErrorContext(&err)
                ),
            )
        })
    }
}

impl CompiledComponentServiceS3 {
    fn get_object_error_is_retriable(
        error: &SdkError<s3::operation::get_object::GetObjectError>,
    ) -> bool {
        match error {
            SdkError::ServiceError(service_error) => !matches!(service_error.err(), NoSuchKey(_)),
            _ => true,
        }
    }

    fn put_object_error_is_retriable(
        _error: &SdkError<s3::operation::put_object::PutObjectError>,
    ) -> bool {
        true
    }
}

pub struct CompiledComponentServiceLocalFileSystem {
    root: PathBuf,
}

impl CompiledComponentServiceLocalFileSystem {
    pub fn new(root: &Path) -> Self {
        if !root.exists() {
            std::fs::create_dir_all(root).expect("Failed to create local compiled component store");
        }
        Self {
            root: root.to_path_buf(),
        }
    }

    fn path_of(&self, component_id: &ComponentId, component_version: u64) -> PathBuf {
        self.root
            .join(format!("{}-{}.cwasm", component_id, component_version))
    }
}

#[async_trait]
impl CompiledComponentService for CompiledComponentServiceLocalFileSystem {
    async fn get(
        &self,
        component_id: &ComponentId,
        component_version: u64,
        engine: &Engine,
    ) -> Result<Option<Component>, GolemError> {
        let path = self.path_of(component_id, component_version);
        if tokio::fs::try_exists(&path).await? {
            let bytes = tokio::fs::read(&path).await?;
            let component = unsafe {
                Component::deserialize(engine, bytes).map_err(|err| {
                    GolemError::component_download_failed(
                        component_id.clone(),
                        component_version,
                        format!("Could not deserialize compiled component: {}", err),
                    )
                })?
            };
            Ok(Some(component))
        } else {
            Ok(None)
        }
    }

    async fn put(
        &self,
        component_id: &ComponentId,
        component_version: u64,
        component: &Component,
    ) -> Result<(), GolemError> {
        let bytes = component.serialize().unwrap();
        let path = self.path_of(component_id, component_version);
        tokio::fs::write(&path, &bytes).await?;
        Ok(())
    }
}

pub struct CompiledComponentServiceDisabled {}

impl Default for CompiledComponentServiceDisabled {
    fn default() -> Self {
        Self::new()
    }
}

impl CompiledComponentServiceDisabled {
    pub fn new() -> Self {
        CompiledComponentServiceDisabled {}
    }
}

#[async_trait]
impl CompiledComponentService for CompiledComponentServiceDisabled {
    async fn get(
        &self,
        _component_id: &ComponentId,
        _component_version: u64,
        _engine: &Engine,
    ) -> Result<Option<Component>, GolemError> {
        Ok(None)
    }

    async fn put(
        &self,
        _component_id: &ComponentId,
        _component_version: u64,
        _component: &Component,
    ) -> Result<(), GolemError> {
        Ok(())
    }
}
