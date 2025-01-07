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

use crate::config::S3BlobStorageConfig;
use crate::storage::blob::{BlobMetadata, BlobStorage, BlobStorageNamespace, ExistsResult};
use async_trait::async_trait;
use aws_sdk_s3::config::{BehaviorVersion, Credentials, Region};
use aws_sdk_s3::error::SdkError;
use aws_sdk_s3::operation::copy_object::CopyObjectError;
use aws_sdk_s3::operation::get_object::GetObjectError::NoSuchKey;
use aws_sdk_s3::operation::head_object::HeadObjectError;
use aws_sdk_s3::operation::put_object::PutObjectError;
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::types::{Delete, Object, ObjectIdentifier};
use aws_sdk_s3::Client;
use bytes::Bytes;
use futures::TryFutureExt;
use golem_common::model::Timestamp;
use golem_common::retries::with_retries_customized;
use std::error::Error;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use tracing::info;

use super::ReplayableStream;

#[derive(Debug)]
pub struct S3BlobStorage {
    client: aws_sdk_s3::Client,
    config: S3BlobStorageConfig,
}

impl S3BlobStorage {
    pub async fn new(config: S3BlobStorageConfig) -> Self {
        let region = config.region.clone();

        let mut config_builder =
            aws_config::defaults(BehaviorVersion::v2024_03_28()).region(Region::new(region));

        if let Some(endpoint_url) = &config.aws_endpoint_url {
            info!(
                "The AWS endpoint urls for blob storage is {}",
                &endpoint_url
            );
            config_builder = config_builder.endpoint_url(endpoint_url);
        }

        if config.use_minio_credentials {
            let creds = Credentials::new("minioadmin", "minioadmin", None, None, "test");
            config_builder = config_builder.credentials_provider(creds);
        }

        let sdk_config = config_builder.load().await;

        Self {
            client: aws_sdk_s3::Client::new(&sdk_config),
            config,
        }
    }

    fn bucket_of(&self, namespace: &BlobStorageNamespace) -> &String {
        match namespace {
            BlobStorageNamespace::CompilationCache => &self.config.compilation_cache_bucket,
            BlobStorageNamespace::CustomStorage(_account_id) => &self.config.custom_data_bucket,
            BlobStorageNamespace::OplogPayload { .. } => &self.config.oplog_payload_bucket,
            BlobStorageNamespace::CompressedOplog { level, .. } => {
                &self.config.compressed_oplog_buckets[*level]
            }
            BlobStorageNamespace::InitialComponentFiles { .. } => {
                &self.config.initial_component_files_bucket
            }
        }
    }

    fn prefix_of(&self, namespace: &BlobStorageNamespace) -> PathBuf {
        match namespace {
            BlobStorageNamespace::CompilationCache => {
                Path::new(&self.config.object_prefix).to_path_buf()
            }
            BlobStorageNamespace::CustomStorage(account_id) => {
                let account_id_string = account_id.to_string();
                if self.config.object_prefix.is_empty() {
                    Path::new(&account_id_string).to_path_buf()
                } else {
                    Path::new(&self.config.object_prefix)
                        .join(account_id_string)
                        .to_path_buf()
                }
            }
            BlobStorageNamespace::OplogPayload {
                account_id,
                worker_id,
            } => {
                let account_id_string = account_id.to_string();
                let worker_id_string = worker_id.to_string();
                if self.config.object_prefix.is_empty() {
                    Path::new(&account_id_string)
                        .join(worker_id_string)
                        .to_path_buf()
                } else {
                    Path::new(&self.config.object_prefix)
                        .join(account_id_string)
                        .join(worker_id_string)
                        .to_path_buf()
                }
            }
            BlobStorageNamespace::CompressedOplog {
                account_id,
                component_id,
                ..
            } => {
                let account_id_string = account_id.to_string();
                let component_id_string = component_id.to_string();
                if self.config.object_prefix.is_empty() {
                    Path::new(&account_id_string)
                        .join(component_id_string)
                        .to_path_buf()
                } else {
                    Path::new(&self.config.object_prefix)
                        .join(account_id_string)
                        .join(component_id_string)
                        .to_path_buf()
                }
            }
            BlobStorageNamespace::InitialComponentFiles { account_id } => {
                let account_id_string = account_id.to_string();
                if self.config.object_prefix.is_empty() {
                    PathBuf::from(&account_id_string)
                } else {
                    Path::new(&self.config.object_prefix)
                        .join(account_id_string)
                        .to_path_buf()
                }
            }
        }
    }

    async fn list_objects(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        bucket: &str,
        prefix: &Path,
    ) -> Result<Vec<Object>, String> {
        let mut result = Vec::new();
        let mut cont: Option<String> = None;

        loop {
            let response = with_retries_customized(
                target_label,
                op_label,
                Some(format!("{bucket} - {}", prefix.to_string_lossy())),
                &self.config.retries,
                &(self.client.clone(), bucket, prefix, cont),
                |(client, bucket, prefix, cont)| {
                    Box::pin(async move {
                        let prefix = if prefix.to_string_lossy().ends_with('/') {
                            prefix.to_string_lossy().to_string()
                        } else {
                            format!("{}/", prefix.to_string_lossy())
                        };
                        client
                            .list_objects_v2()
                            .bucket(*bucket)
                            .prefix(prefix)
                            .set_continuation_token(cont.clone())
                            .send()
                            .await
                    })
                },
                Self::is_list_objects_v2_error_retriable,
                Self::as_loggable_generic,
            )
            .await
            .map_err(|err| err.to_string())?;

            result.extend(response.contents().iter().cloned());
            if let Some(cont_token) = response.next_continuation_token() {
                cont = Some(cont_token.to_string());
            } else {
                break;
            }
        }

        Ok(result)
    }

    fn is_get_object_error_retriable(
        error: &SdkError<aws_sdk_s3::operation::get_object::GetObjectError>,
    ) -> bool {
        match error {
            SdkError::ServiceError(service_error) => !matches!(service_error.err(), NoSuchKey(_)),
            _ => true,
        }
    }

    fn is_head_object_error_retriable(error: &SdkError<HeadObjectError>) -> bool {
        match error {
            SdkError::ServiceError(service_error) => {
                !matches!(service_error.err(), HeadObjectError::NotFound(_))
            }
            _ => true,
        }
    }

    fn is_put_object_error_retriable(
        _error: &SdkError<aws_sdk_s3::operation::put_object::PutObjectError>,
    ) -> bool {
        true
    }

    fn is_list_objects_v2_error_retriable(
        _error: &SdkError<aws_sdk_s3::operation::list_objects_v2::ListObjectsV2Error>,
    ) -> bool {
        true
    }

    fn is_delete_object_error_retriable(
        _error: &SdkError<aws_sdk_s3::operation::delete_object::DeleteObjectError>,
    ) -> bool {
        true
    }

    fn is_delete_objects_error_retriable(
        _error: &SdkError<aws_sdk_s3::operation::delete_objects::DeleteObjectsError>,
    ) -> bool {
        true
    }

    fn is_copy_object_error_retriable(error: &SdkError<CopyObjectError>) -> bool {
        match error {
            SdkError::ServiceError(service_error) => !matches!(
                service_error.err(),
                CopyObjectError::ObjectNotInActiveTierError(_)
            ),
            _ => true,
        }
    }

    fn error_string<T: Error>(error: &SdkError<T>) -> String {
        match error {
            SdkError::ConstructionFailure(_) => "Construction failure".to_string(),
            SdkError::TimeoutError(_) => "Timeout".to_string(),
            SdkError::DispatchFailure(inner) => {
                format!("Dispatch failure: {}", inner.as_connector_error().unwrap())
            }
            SdkError::ResponseError(_) => "Response error".to_string(),
            SdkError::ServiceError(inner) => inner.err().to_string(),
            _ => error.to_string(),
        }
    }

    fn as_loggable_generic<T: Error>(error: &SdkError<T>) -> Option<String> {
        Some(Self::error_string(error))
    }

    fn get_object_error_as_loggable(
        error: &SdkError<aws_sdk_s3::operation::get_object::GetObjectError>,
    ) -> Option<String> {
        match error {
            SdkError::ServiceError(service_error) => {
                if matches!(service_error.err(), NoSuchKey(_)) {
                    None
                } else {
                    Some(Self::error_string(error))
                }
            }
            _ => Some(Self::error_string(error)),
        }
    }

    fn head_object_error_as_loggable(error: &SdkError<HeadObjectError>) -> Option<String> {
        match error {
            SdkError::ServiceError(service_error) => {
                if matches!(service_error.err(), HeadObjectError::NotFound(_)) {
                    None
                } else {
                    Some(Self::error_string(error))
                }
            }
            _ => Some(Self::error_string(error)),
        }
    }
}

#[async_trait]
impl BlobStorage for S3BlobStorage {
    async fn get_raw(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Option<Bytes>, String> {
        let bucket = self.bucket_of(&namespace);
        let key = self.prefix_of(&namespace).join(path);

        let result = with_retries_customized(
            target_label,
            op_label,
            Some(format!("{bucket} - {key:?}")),
            &self.config.retries,
            &(self.client.clone(), bucket, key),
            |(client, bucket, key)| {
                Box::pin(async move {
                    client
                        .get_object()
                        .bucket(*bucket)
                        .key(key.to_string_lossy())
                        .send()
                        .await
                })
            },
            Self::is_get_object_error_retriable,
            Self::get_object_error_as_loggable,
        )
        .await;

        match result {
            Ok(response) => {
                let body = response.body;
                let aggregated_bytes = body.collect().await.map_err(|err| err.to_string())?;
                let bytes = aggregated_bytes.into_bytes();

                Ok(Some(bytes))
            }
            Err(SdkError::ServiceError(service_error)) => match service_error.err() {
                NoSuchKey(_) => Ok(None),
                err => Err(err.to_string()),
            },
            Err(err) => Err(Self::error_string(&err)),
        }
    }

    async fn get_stream(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Option<Pin<Box<dyn futures::Stream<Item = Result<Bytes, String>> + Send>>>, String>
    {
        let bucket = self.bucket_of(&namespace);
        let key = self.prefix_of(&namespace).join(path);

        let result = with_retries_customized(
            target_label,
            op_label,
            Some(format!("{bucket} - {key:?}")),
            &self.config.retries,
            &(self.client.clone(), bucket, key),
            |(client, bucket, key)| {
                Box::pin(async move {
                    client
                        .get_object()
                        .bucket(*bucket)
                        .key(key.to_string_lossy())
                        .send()
                        .await
                })
            },
            Self::is_get_object_error_retriable,
            Self::get_object_error_as_loggable,
        )
        .await;

        match result {
            Ok(response) => {
                let stream = futures::stream::unfold(response.body, |mut body| async {
                    body.next()
                        .await
                        .map(|x| (x.map_err(|e| e.to_string()), body))
                });
                let pinned: Pin<Box<dyn futures::Stream<Item = Result<Bytes, String>> + Send>> =
                    Box::pin(stream);
                Ok(Some(pinned))
            }
            Err(SdkError::ServiceError(service_error)) => match service_error.err() {
                NoSuchKey(_) => Ok(None),
                err => Err(err.to_string()),
            },
            Err(err) => Err(Self::error_string(&err)),
        }
    }

    async fn get_raw_slice(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        start: u64,
        end: u64,
    ) -> Result<Option<Bytes>, String> {
        let bucket = self.bucket_of(&namespace);
        let key = self.prefix_of(&namespace).join(path);

        let result = with_retries_customized(
            target_label,
            op_label,
            Some(format!("{bucket} - {key:?}")),
            &self.config.retries,
            &(self.client.clone(), bucket, key),
            |(client, bucket, key)| {
                Box::pin(async move {
                    client
                        .get_object()
                        .bucket(*bucket)
                        .key(key.to_string_lossy())
                        .range(format!("bytes={}-{}", start, end))
                        .send()
                        .await
                })
            },
            Self::is_get_object_error_retriable,
            Self::as_loggable_generic,
        )
        .await;

        match result {
            Ok(response) => {
                let body = response.body;
                let aggregated_bytes = body.collect().await.map_err(|err| err.to_string())?;
                let bytes = aggregated_bytes.into_bytes();

                Ok(Some(bytes))
            }
            Err(SdkError::ServiceError(service_error)) => match service_error.err() {
                NoSuchKey(_) => Ok(None),
                err => Err(err.to_string()),
            },
            Err(err) => Err(Self::error_string(&err)),
        }
    }

    async fn get_metadata(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Option<BlobMetadata>, String> {
        let bucket = self.bucket_of(&namespace);
        let key = self.prefix_of(&namespace).join(path);
        let op_id = format!("{} - {:?}", bucket, key);

        let file_head_result = with_retries_customized(
            target_label,
            op_label,
            Some(op_id.clone()),
            &self.config.retries,
            &(self.client.clone(), bucket, key.clone()),
            |(client, bucket, key)| {
                Box::pin(async move {
                    client
                        .head_object()
                        .bucket(*bucket)
                        .key(key.to_string_lossy())
                        .send()
                        .await
                })
            },
            Self::is_head_object_error_retriable,
            Self::head_object_error_as_loggable,
        )
        .await;
        match file_head_result {
            Ok(result) => Ok(Some(BlobMetadata {
                size: result.content_length().unwrap_or_default() as u64,
                last_modified_at: Timestamp::from(
                    result
                        .last_modified
                        .unwrap()
                        .to_millis()
                        .expect("failed to convert date-time value to millis")
                        as u64,
                ),
            })),
            Err(SdkError::ServiceError(service_error)) => match service_error.err() {
                HeadObjectError::NotFound(_) => {
                    let marker = key.join("__dir_marker");
                    let dir_marker_head_result = with_retries_customized(
                        target_label,
                        op_label,
                        Some(op_id),
                        &self.config.retries,
                        &(self.client.clone(), bucket, marker),
                        |(client, bucket, marker)| {
                            Box::pin(async move {
                                client
                                    .head_object()
                                    .bucket(*bucket)
                                    .key(marker.to_string_lossy())
                                    .send()
                                    .await
                            })
                        },
                        Self::is_head_object_error_retriable,
                        Self::head_object_error_as_loggable,
                    )
                    .await;
                    match dir_marker_head_result {
                        Ok(result) => Ok(Some(BlobMetadata {
                            size: 0,
                            last_modified_at: Timestamp::from(
                                result
                                    .last_modified
                                    .unwrap()
                                    .to_millis()
                                    .expect("failed to convert date-time value to millis")
                                    as u64,
                            ),
                        })),
                        Err(SdkError::ServiceError(service_error)) => match service_error.err() {
                            HeadObjectError::NotFound(_) => Ok(None),
                            err => Err(err.to_string()),
                        },
                        Err(err) => Err(Self::error_string(&err)),
                    }
                }
                err => Err(err.to_string()),
            },
            Err(err) => Err(Self::error_string(&err)),
        }
    }

    async fn put_raw(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        data: &[u8],
    ) -> Result<(), String> {
        let bucket = self.bucket_of(&namespace);
        let key = self.prefix_of(&namespace).join(path);

        with_retries_customized(
            target_label,
            op_label,
            Some(format!("{bucket} - {key:?}")),
            &self.config.retries,
            &(self.client.clone(), bucket, key, data),
            |(client, bucket, key, bytes)| {
                Box::pin(async move {
                    client
                        .put_object()
                        .bucket(*bucket)
                        .key(key.to_string_lossy())
                        .body(ByteStream::from(bytes.to_vec()))
                        .send()
                        .await
                })
            },
            Self::is_put_object_error_retriable,
            Self::as_loggable_generic,
        )
        .await
        .map(|_| ())
        .map_err(|err| err.to_string())
    }

    async fn put_stream(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        stream: &dyn ReplayableStream<Item = Result<Bytes, String>>,
    ) -> Result<(), String> {
        let bucket = self.bucket_of(&namespace);
        let key = self.prefix_of(&namespace).join(path);

        fn go<'a>(
            args: &'a (
                Client,
                &String,
                PathBuf,
                &dyn ReplayableStream<Item = Result<Bytes, String>>,
            ),
        ) -> Pin<
            Box<dyn Future<Output = Result<(), SdkErrorOrCustomError<PutObjectError>>> + 'a + Send>,
        > {
            let (client, bucket, key, stream) = args;
            Box::pin(async move {
                let stream_length = stream
                    .length()
                    .await
                    .map_err(SdkErrorOrCustomError::custom_error)?;
                let stream = stream
                    .make_stream()
                    .await
                    .map_err(SdkErrorOrCustomError::custom_error)?;
                let body = reqwest::Body::wrap_stream(stream);
                let byte_stream = ByteStream::from_body_1_x(body);

                client
                    .put_object()
                    .bucket(*bucket)
                    .key(key.to_string_lossy())
                    .content_length(stream_length as i64)
                    .body(byte_stream)
                    .send()
                    .map_err(SdkErrorOrCustomError::sdk_error)
                    .map_ok(|_| ())
                    .await
            })
        }

        with_retries_customized(
            target_label,
            op_label,
            Some(format!("{bucket} - {key:?}")),
            &self.config.retries,
            &(self.client.clone(), bucket, key, stream),
            go,
            |err| err.is_retriable(Self::is_put_object_error_retriable),
            SdkErrorOrCustomError::as_loggable,
        )
        .await
        .map_err(SdkErrorOrCustomError::into_string)
    }

    async fn delete(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<(), String> {
        let bucket = self.bucket_of(&namespace);
        let key = self.prefix_of(&namespace).join(path);

        with_retries_customized(
            target_label,
            op_label,
            Some(format!("{bucket} - {key:?}")),
            &self.config.retries,
            &(self.client.clone(), bucket, key),
            |(client, bucket, key)| {
                Box::pin(async move {
                    client
                        .delete_object()
                        .bucket(*bucket)
                        .key(key.to_string_lossy())
                        .send()
                        .await
                })
            },
            Self::is_delete_object_error_retriable,
            Self::as_loggable_generic,
        )
        .await
        .map_err(|err| err.to_string())?;

        Ok(())
    }

    async fn delete_many(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        paths: &[PathBuf],
    ) -> Result<(), String> {
        let bucket = self.bucket_of(&namespace);
        let prefix = self.prefix_of(&namespace);

        let to_delete = paths
            .iter()
            .map(|path| {
                let key = prefix.join(path);
                ObjectIdentifier::builder()
                    .key(key.to_string_lossy())
                    .build()
                    .map_err(|e| e.to_string())
            })
            .collect::<Result<Vec<_>, String>>()?;

        with_retries_customized(
            target_label,
            op_label,
            Some(format!("{bucket} - {prefix:?}")),
            &self.config.retries,
            &(self.client.clone(), bucket, to_delete),
            |(client, bucket, to_delete)| {
                Box::pin(async move {
                    client
                        .delete_objects()
                        .bucket(*bucket)
                        .delete(
                            Delete::builder()
                                .set_objects(Some(to_delete.clone()))
                                .build()
                                .expect("Could not build delete object"),
                        )
                        .send()
                        .await
                })
            },
            Self::is_delete_objects_error_retriable,
            Self::as_loggable_generic,
        )
        .await
        .map_err(|err| err.to_string())?;

        Ok(())
    }

    async fn create_dir(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<(), String> {
        let bucket = self.bucket_of(&namespace);
        let key = self.prefix_of(&namespace).join(path);
        let marker = key.join("__dir_marker");

        with_retries_customized(
            target_label,
            op_label,
            Some(format!("{bucket} - {key:?}")),
            &self.config.retries,
            &(self.client.clone(), bucket, marker),
            |(client, bucket, marker)| {
                Box::pin(async move {
                    client
                        .put_object()
                        .bucket(*bucket)
                        .key(marker.to_string_lossy())
                        .body(ByteStream::from(Bytes::new()))
                        .send()
                        .await
                })
            },
            Self::is_put_object_error_retriable,
            Self::as_loggable_generic,
        )
        .await
        .map_err(|err| err.to_string())?;

        Ok(())
    }

    async fn list_dir(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Vec<PathBuf>, String> {
        let bucket = self.bucket_of(&namespace);
        let namespace_root = self.prefix_of(&namespace);
        let key = namespace_root.join(path);

        Ok(self
            .list_objects(target_label, op_label, bucket, &key)
            .await?
            .iter()
            .flat_map(|obj| obj.key.as_ref().map(|k| Path::new(k).to_path_buf()))
            .filter_map(|path| {
                let is_dir_marker =
                    path.file_name().and_then(|s| s.to_str()) == Some("__dir_marker");
                let is_nested = path.parent() != Some(&key);
                if is_nested {
                    if is_dir_marker {
                        path.parent().map(|p| p.to_path_buf())
                    } else {
                        None
                    }
                } else if is_dir_marker {
                    None
                } else {
                    Some(path)
                }
            })
            .filter_map(|path| {
                path.strip_prefix(&namespace_root)
                    .ok()
                    .map(|p| p.to_path_buf())
            })
            .collect::<Vec<_>>())
    }

    async fn delete_dir(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<bool, String> {
        let bucket = self.bucket_of(&namespace);
        let key = self.prefix_of(&namespace).join(path);

        let to_delete = self
            .list_objects(target_label, op_label, bucket, &key)
            .await?
            .iter()
            .flat_map(|obj| {
                obj.key.as_ref().map(|k| {
                    ObjectIdentifier::builder()
                        .key(k)
                        .build()
                        .map_err(|e| e.to_string())
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let has_entries = !to_delete.is_empty();

        if has_entries {
            with_retries_customized(
                target_label,
                op_label,
                Some(format!("{bucket} - {key:?}")),
                &self.config.retries,
                &(self.client.clone(), bucket, to_delete),
                |(client, bucket, to_delete)| {
                    Box::pin(async move {
                        client
                            .delete_objects()
                            .bucket(*bucket)
                            .delete(
                                Delete::builder()
                                    .set_objects(Some(to_delete.clone()))
                                    .build()
                                    .expect("Could not build delete object"),
                            )
                            .send()
                            .await
                    })
                },
                Self::is_delete_objects_error_retriable,
                Self::as_loggable_generic,
            )
            .await
            .map_err(|err| err.to_string())?;
        }

        Ok(has_entries)
    }

    async fn exists(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<ExistsResult, String> {
        let bucket = self.bucket_of(&namespace);
        let key = self.prefix_of(&namespace).join(path);
        let op_id = format!("{} - {:?}", bucket, key);

        let file_head_result = with_retries_customized(
            target_label,
            op_label,
            Some(op_id.clone()),
            &self.config.retries,
            &(self.client.clone(), bucket, key.clone()),
            |(client, bucket, key)| {
                Box::pin(async move {
                    client
                        .head_object()
                        .bucket(*bucket)
                        .key(key.to_string_lossy())
                        .send()
                        .await
                })
            },
            Self::is_head_object_error_retriable,
            Self::head_object_error_as_loggable,
        )
        .await;
        match file_head_result {
            Ok(_) => Ok(ExistsResult::File),
            Err(SdkError::ServiceError(service_error)) => match service_error.err() {
                HeadObjectError::NotFound(_) => {
                    let marker = key.join("__dir_marker");
                    let dir_marker_head_result = with_retries_customized(
                        target_label,
                        op_label,
                        Some(op_id),
                        &self.config.retries,
                        &(self.client.clone(), bucket, marker),
                        |(client, bucket, marker)| {
                            Box::pin(async move {
                                client
                                    .head_object()
                                    .bucket(*bucket)
                                    .key(marker.to_string_lossy())
                                    .send()
                                    .await
                            })
                        },
                        Self::is_head_object_error_retriable,
                        Self::head_object_error_as_loggable,
                    )
                    .await;
                    match dir_marker_head_result {
                        Ok(_) => Ok(ExistsResult::Directory),
                        Err(SdkError::ServiceError(service_error)) => match service_error.err() {
                            HeadObjectError::NotFound(_) => Ok(ExistsResult::DoesNotExist),
                            err => Err(err.to_string()),
                        },
                        Err(err) => Err(Self::error_string(&err)),
                    }
                }
                err => Err(err.to_string()),
            },
            Err(err) => Err(Self::error_string(&err)),
        }
    }

    async fn copy(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        from: &Path,
        to: &Path,
    ) -> Result<(), String> {
        let bucket = self.bucket_of(&namespace);
        let from_key = self.prefix_of(&namespace).join(from);
        let to_key = self.prefix_of(&namespace).join(to);

        with_retries_customized(
            target_label,
            op_label,
            Some(format!("{bucket} - {from_key:?} -> {to_key:?}")),
            &self.config.retries,
            &(self.client.clone(), bucket, from_key, to_key),
            |(client, bucket, from_key, to_key)| {
                Box::pin(async move {
                    client
                        .copy_object()
                        .bucket(*bucket)
                        .copy_source(format!("/{}/{}", *bucket, from_key.to_string_lossy()))
                        .key(to_key.to_string_lossy())
                        .send()
                        .await
                })
            },
            Self::is_copy_object_error_retriable,
            Self::as_loggable_generic,
        )
        .await
        .map_err(|err| err.to_string())?;
        Ok(())
    }
}

enum SdkErrorOrCustomError<T> {
    SdkError(aws_sdk_s3::error::SdkError<T>),
    CustomError(String),
}

impl<T> SdkErrorOrCustomError<T> {
    fn sdk_error(err: aws_sdk_s3::error::SdkError<T>) -> Self {
        SdkErrorOrCustomError::SdkError(err)
    }

    fn custom_error(err: String) -> Self {
        SdkErrorOrCustomError::CustomError(err)
    }

    fn is_retriable<F: FnOnce(&aws_sdk_s3::error::SdkError<T>) -> bool>(
        &self,
        is_sdk_error_retryable: F,
    ) -> bool {
        match self {
            SdkErrorOrCustomError::SdkError(err) => is_sdk_error_retryable(err),
            SdkErrorOrCustomError::CustomError(_) => true,
        }
    }

    fn as_loggable(&self) -> Option<String>
    where
        T: Error,
    {
        match self {
            SdkErrorOrCustomError::SdkError(err) => S3BlobStorage::as_loggable_generic(err),
            SdkErrorOrCustomError::CustomError(err) => Some(err.clone()),
        }
    }

    fn into_string(self) -> String
    where
        T: Error,
    {
        match self {
            SdkErrorOrCustomError::SdkError(err) => S3BlobStorage::error_string(&err),
            SdkErrorOrCustomError::CustomError(err) => err,
        }
    }
}
