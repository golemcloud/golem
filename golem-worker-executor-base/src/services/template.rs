use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures_util::TryStreamExt;
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode, SimpleCache};
use golem_common::config::RetryConfig;
use golem_common::metrics::external_calls::record_external_call_response_size_bytes;
use golem_common::model::TemplateId;
use golem_api_grpc::proto::golem::cloudservices::templateservice::template_service_client::TemplateServiceClient;
use golem_api_grpc::proto::golem::cloudservices::templateservice::{
    download_template_response, get_latest_template_version_response, DownloadTemplateRequest,
    GetLatestTemplateVersionRequest,
};
use golem_api_grpc::proto::golem::{TemplateError, TokenSecret};
use golem_common::retries::with_retries;
use http::Uri;
use prost::Message;
use tracing::{debug, info, warn};
use uuid::Uuid;
use wasmtime::component::Component;
use wasmtime::Engine;

use crate::error::GolemError;
use crate::grpc::{is_grpc_retriable, GrpcError};
use crate::metrics::component::record_compilation_time;
use crate::services::compiled_template;
use crate::services::compiled_template::CompiledTemplateService;
use crate::services::golem_config::{
    CompiledTemplateServiceConfig, TemplateCacheConfig, TemplateServiceConfig,
};

/// Service for downloading a specific Golem template (WASM component) from the Golem Cloud API
#[async_trait]
pub trait TemplateService {
    async fn get(
        &self,
        engine: &Engine,
        template_id: &TemplateId,
        template_version: i32,
    ) -> Result<Component, GolemError>;

    async fn get_latest(
        &self,
        engine: &Engine,
        template_id: &TemplateId,
    ) -> Result<(i32, Component), GolemError>;
}

pub async fn configured(
    config: &TemplateServiceConfig,
    cache_config: &TemplateCacheConfig,
    compiled_config: &CompiledTemplateServiceConfig,
) -> Arc<dyn TemplateService + Send + Sync> {
    let compiled_component_service = compiled_template::configured(compiled_config).await;
    match config {
        TemplateServiceConfig::Grpc(config) => {
            info!("Using template API at {}", config.url());
            Arc::new(TemplateServiceGrpc::new(
                config.uri(),
                config
                    .access_token
                    .parse::<Uuid>()
                    .expect("Access token must be an UUID"),
                cache_config.max_capacity,
                cache_config.time_to_idle,
                config.retries.clone(),
                compiled_component_service,
            ))
        }
        TemplateServiceConfig::Local(config) => Arc::new(TemplateServiceLocalFileSystem::new(
            &config.root,
            cache_config.max_capacity,
            cache_config.time_to_idle,
            compiled_component_service,
        )),
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct TemplateKey {
    template_id: TemplateId,
    template_version: i32,
}

pub struct TemplateServiceGrpc {
    endpoint: Uri,
    template_cache: Cache<TemplateKey, (), Component, GolemError>,
    access_token: Uuid,
    retry_config: RetryConfig,
    compiled_template_service: Arc<dyn CompiledTemplateService + Send + Sync>,
}

impl TemplateServiceGrpc {
    pub fn new(
        endpoint: Uri,
        access_token: Uuid,
        max_capacity: usize,
        time_to_idle: Duration,
        retry_config: RetryConfig,
        compiled_component_service: Arc<dyn CompiledTemplateService + Send + Sync>,
    ) -> Self {
        Self {
            endpoint,
            template_cache: create_template_cache(max_capacity, time_to_idle),
            access_token,
            retry_config,
            compiled_template_service: compiled_component_service,
        }
    }
}

#[async_trait]
impl TemplateService for TemplateServiceGrpc {
    async fn get(
        &self,
        engine: &Engine,
        template_id: &TemplateId,
        template_version: i32,
    ) -> Result<Component, GolemError> {
        let key = TemplateKey {
            template_id: template_id.clone(),
            template_version,
        };
        let template_id = template_id.clone();
        let engine = engine.clone();
        let endpoint = self.endpoint.clone();
        let access_token = self.access_token;
        let retry_config = self.retry_config.clone();
        let compiled_template_service = self.compiled_template_service.clone();
        self.template_cache
            .get_or_insert_simple(&key.clone(), || {
                Box::pin(async move {
                    let result = compiled_template_service
                        .get(&template_id, template_version, &engine)
                        .await;

                    let component = match result {
                        Ok(component) => component,
                        Err(err) => {
                            warn!("Failed to download compiled component {:?}: {}", key, err);
                            None
                        }
                    };

                    match component {
                        Some(component) => Ok(component),
                        None => {
                            let bytes = download_via_grpc(
                                &endpoint,
                                &access_token,
                                &retry_config,
                                &template_id,
                                template_version,
                            )
                            .await?;

                            let start = Instant::now();
                            let component =
                                Component::from_binary(&engine, &bytes).map_err(|e| {
                                    GolemError::TemplateParseFailed {
                                        template_id: template_id.clone(),
                                        template_version,
                                        reason: format!("{}", e),
                                    }
                                })?;
                            let end = Instant::now();

                            let compilation_time = end.duration_since(start);
                            record_compilation_time(compilation_time);
                            debug!(
                                "Compiled {} in {}ms",
                                template_id,
                                compilation_time.as_millis(),
                            );

                            let result = compiled_template_service
                                .put(&template_id, template_version, &component)
                                .await;

                            match result {
                                Ok(_) => Ok(component),
                                Err(err) => {
                                    warn!("Failed to upload compiled template {:?}: {}", key, err);
                                    Ok(component)
                                }
                            }
                        }
                    }
                })
            })
            .await
    }

    async fn get_latest(
        &self,
        engine: &Engine,
        template_id: &TemplateId,
    ) -> Result<(i32, Component), GolemError> {
        let latest_version = get_latest_version_via_grpc(
            &self.endpoint,
            &self.access_token,
            &self.retry_config,
            template_id,
        )
        .await?;
        let component = self.get(engine, template_id, latest_version).await?;
        Ok((latest_version, component))
    }
}

async fn download_via_grpc(
    endpoint: &Uri,
    access_token: &Uuid,
    retry_config: &RetryConfig,
    template_id: &TemplateId,
    template_version: i32,
) -> Result<Vec<u8>, GolemError> {
    let desc = format!("Downloading template {template_id}");
    debug!("{}", &desc);
    with_retries(
        &desc,
        "components",
        "download",
        retry_config,
        &(
            endpoint.clone(),
            template_id.clone(),
            access_token.to_owned(),
        ),
        |(endpoint, template_id, access_token)| {
            Box::pin(async move {
                let mut client = TemplateServiceClient::connect(endpoint.clone())
                    .await?
                    .max_decoding_message_size(50 * 1024 * 1024);

                let response = client
                    .download_template(DownloadTemplateRequest {
                        template_id: Some(template_id.clone().into()),
                        version: Some(template_version),
                        token_secret: Some(TokenSecret {
                            value: Some((*access_token).into()),
                        }),
                    })
                    .await?
                    .into_inner();

                let chunks = response.into_stream().try_collect::<Vec<_>>().await?;
                let bytes = chunks
                    .into_iter()
                    .map(|chunk| match chunk.result {
                        None => Err("Empty response".to_string().into()),
                        Some(download_template_response::Result::SuccessChunk(chunk)) => Ok(chunk),
                        Some(download_template_response::Result::Error(error)) => {
                            Err(GrpcError::Domain(error))
                        }
                    })
                    .collect::<Result<Vec<Vec<u8>>, GrpcError<TemplateError>>>()?;

                let bytes: Vec<u8> = bytes.into_iter().flatten().collect();

                record_external_call_response_size_bytes("components", "download", bytes.len());

                Ok(bytes)
            })
        },
        is_grpc_retriable::<TemplateError>,
    )
    .await
    .map_err(|error| grpc_template_download_error(error, template_id, template_version))
}

async fn get_latest_version_via_grpc(
    endpoint: &Uri,
    access_token: &Uuid,
    retry_config: &RetryConfig,
    template_id: &TemplateId,
) -> Result<i32, GolemError> {
    let desc = format!("Getting latest version of {template_id}");
    debug!("{}", &desc);
    with_retries(
        &desc,
        "components",
        "download",
        retry_config,
        &(
            endpoint.clone(),
            template_id.clone(),
            access_token.to_owned(),
        ),
        |(endpoint, template_id, access_token)| {
            Box::pin(async move {
                let mut client = TemplateServiceClient::connect(endpoint.clone()).await?;
                let response = client
                    .get_latest_template_version(GetLatestTemplateVersionRequest {
                        template_id: Some(template_id.clone().into()),
                        token_secret: Some(TokenSecret {
                            value: Some((*access_token).into()),
                        }),
                    })
                    .await?
                    .into_inner();

                let len = response.encoded_len();
                let version = match response.result {
                    None => Err("Empty response".to_string().into()),
                    Some(get_latest_template_version_response::Result::Success(version)) => {
                        Ok(version)
                    }
                    Some(get_latest_template_version_response::Result::Error(error)) => {
                        Err(GrpcError::Domain(error))
                    }
                }?;

                record_external_call_response_size_bytes("components", "get_latest_version", len);

                Ok(version)
            })
        },
        is_grpc_retriable::<TemplateError>,
    )
    .await
    .map_err(|error| grpc_get_latest_version_error(error, template_id))
}

fn grpc_template_download_error(
    error: GrpcError<TemplateError>,
    template_id: &TemplateId,
    template_version: i32,
) -> GolemError {
    GolemError::TemplateDownloadFailed {
        template_id: template_id.clone(),
        template_version,
        reason: format!("{}", error),
    }
}

fn grpc_get_latest_version_error(
    error: GrpcError<TemplateError>,
    template_id: &TemplateId,
) -> GolemError {
    GolemError::GetLatestVersionOfTemplateFailed {
        template_id: template_id.clone(),
        reason: format!("{}", error),
    }
}

fn create_template_cache(
    max_capacity: usize,
    time_to_idle: Duration,
) -> Cache<TemplateKey, (), Component, GolemError> {
    Cache::new(
        Some(max_capacity),
        FullCacheEvictionMode::LeastRecentlyUsed(1),
        BackgroundEvictionMode::OlderThan {
            ttl: time_to_idle,
            period: Duration::from_secs(60),
        },
        "component",
    )
}

impl From<std::io::Error> for GolemError {
    fn from(value: std::io::Error) -> Self {
        GolemError::Unknown {
            details: format!("{}", value),
        }
    }
}

pub struct TemplateServiceLocalFileSystem {
    root: PathBuf,
    template_cache: Cache<TemplateKey, (), Component, GolemError>,
    compiled_template_service: Arc<dyn CompiledTemplateService + Send + Sync>,
}

impl TemplateServiceLocalFileSystem {
    pub fn new(
        root: &Path,
        max_capacity: usize,
        time_to_idle: Duration,
        compiled_template_service: Arc<dyn CompiledTemplateService + Send + Sync>,
    ) -> Self {
        if !root.exists() {
            std::fs::create_dir_all(root).expect("Failed to create local template store");
        }
        Self {
            root: root.to_path_buf(),
            template_cache: create_template_cache(max_capacity, time_to_idle),
            compiled_template_service,
        }
    }

    async fn get_from_path(
        &self,
        path: &Path,
        engine: &Engine,
        template_id: &TemplateId,
        template_version: i32,
    ) -> Result<Component, GolemError> {
        let key = TemplateKey {
            template_id: template_id.clone(),
            template_version,
        };
        let template_id = template_id.clone();
        let engine = engine.clone();
        let compiled_template_service = self.compiled_template_service.clone();
        let path = path.to_path_buf();
        self.template_cache
            .get_or_insert_simple(&key.clone(), || {
                Box::pin(async move {
                    let result = compiled_template_service
                        .get(&template_id, template_version, &engine)
                        .await;

                    let component = match result {
                        Ok(component) => component,
                        Err(err) => {
                            warn!("Failed to download compiled component {:?}: {}", key, err);
                            None
                        }
                    };

                    match component {
                        Some(component) => Ok(component),
                        None => {
                            let bytes = tokio::fs::read(path).await?;

                            let start = Instant::now();
                            let component =
                                Component::from_binary(&engine, &bytes).map_err(|e| {
                                    GolemError::TemplateParseFailed {
                                        template_id: template_id.clone(),
                                        template_version,
                                        reason: format!("{}", e),
                                    }
                                })?;
                            let end = Instant::now();

                            let compilation_time = end.duration_since(start);
                            record_compilation_time(compilation_time);
                            debug!(
                                "Compiled {} in {}ms",
                                template_id,
                                compilation_time.as_millis(),
                            );

                            let result = compiled_template_service
                                .put(&template_id, template_version, &component)
                                .await;

                            match result {
                                Ok(_) => Ok(component),
                                Err(err) => {
                                    warn!("Failed to upload compiled template {:?}: {}", key, err);
                                    Ok(component)
                                }
                            }
                        }
                    }
                })
            })
            .await
    }
}

#[async_trait]
impl TemplateService for TemplateServiceLocalFileSystem {
    async fn get(
        &self,
        engine: &Engine,
        template_id: &TemplateId,
        template_version: i32,
    ) -> Result<Component, GolemError> {
        let path = self
            .root
            .join(format!("{}-{}.wasm", template_id, template_version));

        self.get_from_path(&path, engine, template_id, template_version)
            .await
    }

    async fn get_latest(
        &self,
        engine: &Engine,
        template_id: &TemplateId,
    ) -> Result<(i32, Component), GolemError> {
        let prefix = format!("{}-", template_id);
        let mut reader = tokio::fs::read_dir(&self.root).await?;
        let mut matching_files = Vec::new();
        while let Some(entry) = reader.next_entry().await? {
            if let Ok(file_name) = entry.file_name().into_string() {
                if file_name.starts_with(&prefix) && file_name.ends_with(".wasm") {
                    matching_files.push((
                        entry.path(),
                        file_name[prefix.len()..file_name.len() - 5].to_string(),
                    ));
                }
            }
        }

        let latest = matching_files
            .into_iter()
            .filter_map(|(path, s)| s.parse::<i32>().map(|version| (path, version)).ok())
            .max_by_key(|(_, version)| *version);

        match latest {
            Some((path, version)) => {
                let component = self
                    .get_from_path(&path, engine, template_id, version)
                    .await?;
                Ok((version, component))
            }
            None => Err(GolemError::GetLatestVersionOfTemplateFailed {
                template_id: template_id.clone(),
                reason: "Could not find any template with the given id".to_string(),
            }),
        }
    }
}

#[cfg(any(feature = "mocks", test))]
pub struct TemplateServiceMock {}

#[cfg(any(feature = "mocks", test))]
impl Default for TemplateServiceMock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(any(feature = "mocks", test))]
impl TemplateServiceMock {
    pub fn new() -> Self {
        Self {}
    }
}

#[cfg(any(feature = "mocks", test))]
#[async_trait]
impl TemplateService for TemplateServiceMock {
    async fn get(
        &self,
        _engine: &Engine,
        _template_id: &TemplateId,
        _template_version: i32,
    ) -> Result<Component, GolemError> {
        unimplemented!()
    }

    async fn get_latest(
        &self,
        _engine: &Engine,
        _template_id: &TemplateId,
    ) -> Result<(i32, Component), GolemError> {
        unimplemented!()
    }
}
