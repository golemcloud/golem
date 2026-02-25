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

use crate::config::S3BlobStorageConfig;
use crate::replayable_stream::ErasedReplayableStream;
use crate::storage::blob::{BlobMetadata, BlobStorage, BlobStorageNamespace, ExistsResult};
use anyhow::Error;
use async_trait::async_trait;
use aws_sdk_s3::Client;
use aws_sdk_s3::config::{BehaviorVersion, Credentials, Region, RequestChecksumCalculation};
use aws_sdk_s3::error::SdkError;
use aws_sdk_s3::operation::copy_object::CopyObjectError;
use aws_sdk_s3::operation::get_object::GetObjectError::NoSuchKey;
use aws_sdk_s3::operation::head_object::HeadObjectError;
use aws_sdk_s3::operation::put_object::PutObjectError;
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::types::{Delete, Object, ObjectIdentifier};
use bytes::{Buf, Bytes};
use futures::TryFutureExt;
use futures::stream::BoxStream;
use golem_common::model::Timestamp;
use golem_common::retries::with_retries_customized;
use http_body::SizeHint;
use http_body_util::BodyExt;
use http_body_util::combinators::BoxBody;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use tracing::info;

#[derive(Debug)]
pub struct S3BlobStorage {
    client: aws_sdk_s3::Client,
    config: S3BlobStorageConfig,
}

impl S3BlobStorage {
    #[allow(deprecated)]
    pub async fn new(config: S3BlobStorageConfig) -> Self {
        let region = config.region.clone();

        let mut config_builder =
            aws_config::defaults(BehaviorVersion::v2024_03_28()).region(Region::new(region));

        if let Some(endpoint_url) = &config.aws_endpoint_url {
            info!("The AWS endpoint url for blob storage is {}", &endpoint_url);
            config_builder = config_builder.endpoint_url(endpoint_url);
        }

        if let Some(credentials) = config.aws_credentials.clone() {
            let creds = Credentials::new(
                credentials.access_key_id,
                credentials.secret_access_key,
                None,
                None,
                credentials.provider_name.leak(),
            );
            config_builder = config_builder.credentials_provider(creds);
        }

        let sdk_config = config_builder.load().await;

        let s3_config: aws_sdk_s3::config::Config = (&sdk_config).into();

        let mut s3_config_builder = s3_config
            .to_builder()
            .request_checksum_calculation(RequestChecksumCalculation::WhenRequired);

        if let Some(path_style) = &config.aws_path_style {
            s3_config_builder = s3_config_builder.force_path_style(*path_style);
        }

        let s3_config = s3_config_builder.build();

        Self {
            client: aws_sdk_s3::Client::from_conf(s3_config),
            config,
        }
    }

    fn bucket_of(&self, namespace: &BlobStorageNamespace) -> &String {
        match namespace {
            BlobStorageNamespace::CompilationCache { .. } => &self.config.compilation_cache_bucket,
            BlobStorageNamespace::CustomStorage { .. } => &self.config.custom_data_bucket,
            BlobStorageNamespace::OplogPayload { .. } => &self.config.oplog_payload_bucket,
            BlobStorageNamespace::CompressedOplog { level, .. } => {
                &self.config.compressed_oplog_buckets[*level]
            }
            BlobStorageNamespace::InitialComponentFiles { .. } => {
                &self.config.initial_component_files_bucket
            }
            BlobStorageNamespace::Components { .. } => &self.config.components_bucket,
        }
    }

    fn prefix_of(&self, namespace: &BlobStorageNamespace) -> PathBuf {
        match namespace {
            BlobStorageNamespace::CompilationCache { environment_id }
            | BlobStorageNamespace::CustomStorage { environment_id }
            | BlobStorageNamespace::InitialComponentFiles { environment_id }
            | BlobStorageNamespace::Components { environment_id } => {
                let environment_id_string = environment_id.to_string();
                if self.config.object_prefix.is_empty() {
                    Path::new(&environment_id_string).to_path_buf()
                } else {
                    Path::new(&self.config.object_prefix)
                        .join(environment_id_string)
                        .to_path_buf()
                }
            }
            BlobStorageNamespace::OplogPayload {
                environment_id,
                worker_id,
            } => {
                let environment_id_string = environment_id.to_string();
                let worker_id_string = worker_id.to_string();
                if self.config.object_prefix.is_empty() {
                    Path::new(&environment_id_string)
                        .join(worker_id_string)
                        .to_path_buf()
                } else {
                    Path::new(&self.config.object_prefix)
                        .join(environment_id_string)
                        .join(worker_id_string)
                        .to_path_buf()
                }
            }
            BlobStorageNamespace::CompressedOplog {
                environment_id,
                component_id,
                ..
            } => {
                let environment_id_string = environment_id.to_string();
                let component_id_string = component_id.to_string();
                if self.config.object_prefix.is_empty() {
                    Path::new(&environment_id_string)
                        .join(component_id_string)
                        .to_path_buf()
                } else {
                    Path::new(&self.config.object_prefix)
                        .join(environment_id_string)
                        .join(component_id_string)
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
    ) -> Result<Vec<Object>, Error> {
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
                Self::sdk_error_as_loggable_string,
                false,
            )
            .await?;

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

    fn error_string<T: std::error::Error>(error: &SdkError<T>) -> String {
        match error {
            SdkError::ConstructionFailure(inner) => format!("Construction failure: {inner:?}"),
            SdkError::TimeoutError(inner) => format!("Timeout: {inner:?}"),
            SdkError::DispatchFailure(inner) => {
                // normal display of the error does not expose enough useful information
                format!(
                    "Dispatch failure: {:?}",
                    inner.as_connector_error().unwrap()
                )
            }
            SdkError::ResponseError(inner) => format!("Response error: {inner:?}"),
            SdkError::ServiceError(inner) => inner.err().to_string(),
            _ => error.to_string(),
        }
    }

    fn sdk_error_as_loggable_string<T: std::error::Error>(error: &SdkError<T>) -> Option<String> {
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
    ) -> Result<Option<Vec<u8>>, Error> {
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
            false,
        )
        .await;

        match result {
            Ok(response) => {
                let body = response.body;
                let aggregated_bytes = body.collect().await?;
                let bytes = aggregated_bytes.to_vec();

                Ok(Some(bytes))
            }
            Err(SdkError::ServiceError(service_error)) => match service_error.into_err() {
                NoSuchKey(_) => Ok(None),
                err => Err(err.into()),
            },
            Err(err) => Err(err.into()),
        }
    }

    async fn get_stream(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Option<BoxStream<'static, Result<Bytes, Error>>>, Error> {
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
            false,
        )
        .await;

        match result {
            Ok(response) => {
                let stream = futures::stream::unfold(response.body, |mut body| async {
                    body.next().await.map(|x| (x.map_err(|e| e.into()), body))
                });
                Ok(Some(Box::pin(stream)))
            }
            Err(SdkError::ServiceError(service_error)) => match service_error.into_err() {
                NoSuchKey(_) => Ok(None),
                err => Err(err.into()),
            },
            Err(err) => Err(err.into()),
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
    ) -> Result<Option<Vec<u8>>, Error> {
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
                        .range(format!("bytes={start}-{end}"))
                        .send()
                        .await
                })
            },
            Self::is_get_object_error_retriable,
            Self::sdk_error_as_loggable_string,
            false,
        )
        .await;

        match result {
            Ok(response) => {
                let body = response.body;
                let aggregated_bytes = body.collect().await?;
                let bytes = aggregated_bytes.to_vec();

                Ok(Some(bytes))
            }
            Err(SdkError::ServiceError(service_error)) => match service_error.into_err() {
                NoSuchKey(_) => Ok(None),
                err => Err(err.into()),
            },
            Err(err) => Err(err.into()),
        }
    }

    async fn get_metadata(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Option<BlobMetadata>, Error> {
        let bucket = self.bucket_of(&namespace);
        let key = self.prefix_of(&namespace).join(path);
        let op_id = format!("{bucket} - {key:?}");

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
            false,
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
            Err(SdkError::ServiceError(service_error)) => match service_error.into_err() {
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
                        false,
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
                        Err(SdkError::ServiceError(service_error)) => {
                            match service_error.into_err() {
                                HeadObjectError::NotFound(_) => Ok(None),
                                err => Err(err.into()),
                            }
                        }
                        Err(err) => Err(err.into()),
                    }
                }
                err => Err(err.into()),
            },
            Err(err) => Err(err.into()),
        }
    }

    async fn put_raw(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        data: &[u8],
    ) -> Result<(), Error> {
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
            Self::sdk_error_as_loggable_string,
            false,
        )
        .await?;

        Ok(())
    }

    async fn put_stream(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        stream: &dyn ErasedReplayableStream<Item = Result<Vec<u8>, Error>, Error = Error>,
    ) -> Result<(), Error> {
        let bucket = self.bucket_of(&namespace);
        let key = self.prefix_of(&namespace).join(path);

        fn go<'a>(
            args: &'a (
                Client,
                &String,
                PathBuf,
                &dyn ErasedReplayableStream<Item = Result<Vec<u8>, Error>, Error = Error>,
            ),
        ) -> Pin<
            Box<dyn Future<Output = Result<(), SdkErrorOrCustomError<PutObjectError>>> + 'a + Send>,
        > {
            let (client, bucket, key, stream) = args;
            Box::pin(async move {
                let stream_length = stream
                    .length_erased()
                    .await
                    .map_err(SdkErrorOrCustomError::custom_error)?;

                let stream = stream
                    .make_stream_erased()
                    .await
                    .map_err(SdkErrorOrCustomError::custom_error)?;

                // Checksum calculation requires body length to be known.
                let body = SizedBody::new(reqwest::Body::wrap_stream(stream), stream_length);

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
            false,
        )
        .await
        .map_err(|e| e.erase())?;

        Ok(())
    }

    async fn delete(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<(), Error> {
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
            Self::sdk_error_as_loggable_string,
            false,
        )
        .await?;

        Ok(())
    }

    async fn delete_many(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        paths: &[PathBuf],
    ) -> Result<(), Error> {
        let bucket = self.bucket_of(&namespace);
        let prefix = self.prefix_of(&namespace);

        let to_delete = paths
            .iter()
            .map(|path| {
                let key = prefix.join(path);
                ObjectIdentifier::builder()
                    .key(key.to_string_lossy())
                    .build()
                    .map_err(|e| e.into())
            })
            .collect::<Result<Vec<_>, Error>>()?;

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
            Self::sdk_error_as_loggable_string,
            false,
        )
        .await?;

        Ok(())
    }

    async fn create_dir(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<(), Error> {
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
            Self::sdk_error_as_loggable_string,
            false,
        )
        .await?;

        Ok(())
    }

    async fn list_dir(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Vec<PathBuf>, Error> {
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
    ) -> Result<bool, Error> {
        let bucket = self.bucket_of(&namespace);
        let key = self.prefix_of(&namespace).join(path);

        let to_delete = self
            .list_objects(target_label, op_label, bucket, &key)
            .await?
            .iter()
            .flat_map(|obj| {
                obj.key
                    .as_ref()
                    .map(|k| ObjectIdentifier::builder().key(k).build())
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
                Self::sdk_error_as_loggable_string,
                false,
            )
            .await?;
        }

        Ok(has_entries)
    }

    async fn exists(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<ExistsResult, Error> {
        let bucket = self.bucket_of(&namespace);
        let key = self.prefix_of(&namespace).join(path);
        let op_id = format!("{bucket} - {key:?}");

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
            false,
        )
        .await;
        match file_head_result {
            Ok(_) => Ok(ExistsResult::File),
            Err(SdkError::ServiceError(service_error)) => match service_error.into_err() {
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
                        false,
                    )
                    .await;
                    match dir_marker_head_result {
                        Ok(_) => Ok(ExistsResult::Directory),
                        Err(SdkError::ServiceError(service_error)) => {
                            match service_error.into_err() {
                                HeadObjectError::NotFound(_) => Ok(ExistsResult::DoesNotExist),
                                err => Err(err.into()),
                            }
                        }
                        Err(err) => Err(err.into()),
                    }
                }
                err => Err(err.into()),
            },
            Err(err) => Err(err.into()),
        }
    }

    async fn copy(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        from: &Path,
        to: &Path,
    ) -> Result<(), Error> {
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
            Self::sdk_error_as_loggable_string,
            false,
        )
        .await?;

        Ok(())
    }
}

#[allow(clippy::large_enum_variant)]
enum SdkErrorOrCustomError<T> {
    SdkError(aws_sdk_s3::error::SdkError<T>),
    CustomError(anyhow::Error),
}

impl<T> SdkErrorOrCustomError<T> {
    fn erase(self) -> anyhow::Error
    where
        T: std::error::Error + Send + Sync + 'static,
    {
        match self {
            Self::CustomError(inner) => inner.context("from CustomError"),
            Self::SdkError(inner) => anyhow::Error::new(inner).context("from SdkError"),
        }
    }

    fn sdk_error(err: aws_sdk_s3::error::SdkError<T>) -> Self {
        SdkErrorOrCustomError::SdkError(err)
    }

    fn custom_error(err: anyhow::Error) -> Self {
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
        T: std::error::Error,
    {
        match self {
            SdkErrorOrCustomError::SdkError(err) => {
                S3BlobStorage::sdk_error_as_loggable_string(err)
            }
            SdkErrorOrCustomError::CustomError(err) => Some(format!("{err:#}")),
        }
    }
}

// body with explicitly overridden size hint. Needed because size hints are not settable for streams, see: https://github.com/seanmonstar/reqwest/issues/1293
pub struct SizedBody<D, E> {
    inner: BoxBody<D, E>,
    hint: SizeHint,
}

impl<D, E> SizedBody<D, E> {
    pub fn new<B>(body: B, size: u64) -> Self
    where
        B: http_body::Body<Data = D, Error = E> + Send + Sync + 'static,
    {
        Self {
            inner: body.boxed(),
            hint: SizeHint::with_exact(size),
        }
    }
}

impl<D: Buf, E> http_body::Body for SizedBody<D, E> {
    type Data = D;
    type Error = E;

    #[inline]
    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<Option<Result<http_body::Frame<D>, E>>> {
        Pin::new(&mut self.inner).poll_frame(cx)
    }

    #[inline]
    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    #[inline]
    fn size_hint(&self) -> SizeHint {
        self.hint.clone()
    }
}
