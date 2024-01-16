use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use aws_config::BehaviorVersion;
use aws_sdk_s3 as s3;
use aws_sdk_s3::error::SdkError;
use golem_common::model::TemplateId;
use golem_common::retries::with_retries;
use s3::config::Region;
use s3::error::DisplayErrorContext;
use s3::operation::get_object::GetObjectError::NoSuchKey;
use s3::primitives::ByteStream;
use wasmtime::component::Component;

use crate::error::GolemError;
use crate::services::golem_config::{
    CompiledTemplateServiceConfig, CompiledTemplateServiceS3Config,
};
use crate::Engine;

/// Service for storing compiled native binaries of WebAssembly components
#[async_trait]
pub trait CompiledTemplateService {
    async fn get(
        &self,
        template_id: &TemplateId,
        template_version: i32,
        engine: &Engine,
    ) -> Result<Option<Component>, GolemError>;
    async fn put(
        &self,
        template_id: &TemplateId,
        template_version: i32,
        component: &Component,
    ) -> Result<(), GolemError>;
}

pub async fn configured(
    config: &CompiledTemplateServiceConfig,
) -> Arc<dyn CompiledTemplateService + Send + Sync> {
    match config {
        CompiledTemplateServiceConfig::S3(config) => {
            let region = config.region.clone();

            dbg!("The endpoint url is {}", &config.aws_endpoint_url);
            dbg!("The env variable with sys env for aws access_key is", &std::env::var("AWS_ACCESS_KEY_ID"));
            dbg!("The env variable with sys env for aws secret is", &std::env::var("AWS_SECRET_ACCESS_KEY"));

            let sdk_config_base = aws_config::defaults(BehaviorVersion::v2023_11_09())
                .region(Region::new(region));

            let sdk_config = if let Some(endpoint_url) = &config.aws_endpoint_url {
                sdk_config_base.endpoint_url(endpoint_url).load().await
            } else {
                sdk_config_base.load().await
            };

            Arc::new(CompiledTemplateServiceS3 {
                config: config.clone(),
                client: s3::Client::new(&sdk_config),
            })
        }
        CompiledTemplateServiceConfig::Local(config) => {
            Arc::new(CompiledTemplateServiceLocalFileSystem::new(&config.root))
        }
        CompiledTemplateServiceConfig::Disabled(_) => {
            Arc::new(CompiledTemplateServiceDisabled::new())
        }
    }
}

pub struct CompiledTemplateServiceS3 {
    config: CompiledTemplateServiceS3Config,
    client: s3::Client,
}

#[async_trait]
impl CompiledTemplateService for CompiledTemplateServiceS3 {
    async fn get(
        &self,
        template_id: &TemplateId,
        template_version: i32,
        engine: &Engine,
    ) -> Result<Option<Component>, GolemError> {
        let prefix = if self.config.object_prefix.is_empty() {
            "".to_string()
        } else {
            format!("{}/", self.config.object_prefix)
        };
        let key = format!("{}{}#{}:compiled", prefix, template_id, template_version);
        let description = format!("Downloading compiled template from {}", key);
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
                    GolemError::template_download_failed(
                        template_id.clone(),
                        template_version,
                        format!(
                            "Could not download compiled template: {}",
                            DisplayErrorContext(&err)
                        ),
                    )
                })?;
                let bytes = aggregated_bytes.into_bytes();
                let component = unsafe {
                    Component::deserialize(engine, &bytes).map_err(|err| {
                        GolemError::template_download_failed(
                            template_id.clone(),
                            template_version,
                            format!("Could not deserialize compiled template: {}", err),
                        )
                    })?
                };
                Ok(Some(component))
            }
            Err(SdkError::ServiceError(service_error)) => match service_error.err() {
                NoSuchKey(_) => Ok(None),
                err => Err(GolemError::template_download_failed(
                    template_id.clone(),
                    template_version,
                    format!(
                        "Could not download compiled template: {}",
                        DisplayErrorContext(&err)
                    ),
                )),
            },
            Err(err) => Err(GolemError::template_download_failed(
                template_id.clone(),
                template_version,
                format!(
                    "Could not download compiled template: {}",
                    DisplayErrorContext(&err)
                ),
            )),
        }
    }

    async fn put(
        &self,
        template_id: &TemplateId,
        template_version: i32,
        component: &Component,
    ) -> Result<(), GolemError> {
        let prefix = if self.config.object_prefix.is_empty() {
            "".to_string()
        } else {
            format!("{}/", self.config.object_prefix)
        };

        let key = format!("{}{}#{}:compiled", prefix, template_id, template_version);
        let bytes = component.serialize().unwrap();

        with_retries(
            "Uploading compiled template",
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
            GolemError::template_download_failed(
                template_id.clone(),
                template_version,
                format!(
                    "Could not upload compiled template: {}",
                    DisplayErrorContext(&err)
                ),
            )
        })
    }
}

impl CompiledTemplateServiceS3 {
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

pub struct CompiledTemplateServiceLocalFileSystem {
    root: PathBuf,
}

impl CompiledTemplateServiceLocalFileSystem {
    pub fn new(root: &Path) -> Self {
        if !root.exists() {
            std::fs::create_dir_all(root).expect("Failed to create local compiled template store");
        }
        Self {
            root: root.to_path_buf(),
        }
    }

    fn path_of(&self, template_id: &TemplateId, template_version: i32) -> PathBuf {
        self.root
            .join(format!("{}-{}.cwasm", template_id, template_version))
    }
}

#[async_trait]
impl CompiledTemplateService for CompiledTemplateServiceLocalFileSystem {
    async fn get(
        &self,
        template_id: &TemplateId,
        template_version: i32,
        engine: &Engine,
    ) -> Result<Option<Component>, GolemError> {
        let path = self.path_of(template_id, template_version);
        if tokio::fs::try_exists(&path).await? {
            let bytes = tokio::fs::read(&path).await?;
            let component = unsafe {
                Component::deserialize(engine, bytes).map_err(|err| {
                    GolemError::template_download_failed(
                        template_id.clone(),
                        template_version,
                        format!("Could not deserialize compiled template: {}", err),
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
        template_id: &TemplateId,
        template_version: i32,
        component: &Component,
    ) -> Result<(), GolemError> {
        let bytes = component.serialize().unwrap();
        let path = self.path_of(template_id, template_version);
        tokio::fs::write(&path, &bytes).await?;
        Ok(())
    }
}

pub struct CompiledTemplateServiceDisabled {}

impl Default for CompiledTemplateServiceDisabled {
    fn default() -> Self {
        Self::new()
    }
}

impl CompiledTemplateServiceDisabled {
    pub fn new() -> Self {
        CompiledTemplateServiceDisabled {}
    }
}

#[async_trait]
impl CompiledTemplateService for CompiledTemplateServiceDisabled {
    async fn get(
        &self,
        _template_id: &TemplateId,
        _template_version: i32,
        _engine: &Engine,
    ) -> Result<Option<Component>, GolemError> {
        Ok(None)
    }

    async fn put(
        &self,
        _template_id: &TemplateId,
        _template_version: i32,
        _component: &Component,
    ) -> Result<(), GolemError> {
        Ok(())
    }
}
