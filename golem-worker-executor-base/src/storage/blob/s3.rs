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

use crate::services::golem_config::S3BlobStorageConfig;
use crate::storage::blob::{BlobMetadata, BlobStorage, BlobStorageNamespace, ExistsResult};
use async_trait::async_trait;
use aws_sdk_s3::config::{BehaviorVersion, Region};
use aws_sdk_s3::error::SdkError;
use aws_sdk_s3::operation::copy_object::CopyObjectError;
use aws_sdk_s3::operation::get_object::GetObjectError::NoSuchKey;
use aws_sdk_s3::operation::head_object::HeadObjectError;
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::types::{Delete, Object, ObjectIdentifier};
use bytes::Bytes;
use golem_common::model::Timestamp;
use golem_common::retries::with_retries;
use std::path::{Path, PathBuf};
use tracing::info;

#[derive(Debug)]
pub struct S3BlobStorage {
    client: aws_sdk_s3::Client,
    config: S3BlobStorageConfig,
}

impl S3BlobStorage {
    pub async fn new(config: S3BlobStorageConfig) -> Self {
        let region = config.region.clone();

        let sdk_config_base =
            aws_config::defaults(BehaviorVersion::v2023_11_09()).region(Region::new(region));

        let sdk_config = if let Some(endpoint_url) = &config.aws_endpoint_url {
            info!(
                "The AWS endpoint urls for blob storage is {}",
                &endpoint_url
            );
            sdk_config_base.endpoint_url(endpoint_url).load().await
        } else {
            sdk_config_base.load().await
        };

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
        }
    }

    async fn list_objects(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        bucket: &str,
        prefix: &str,
    ) -> Result<Vec<Object>, String> {
        let description = format!("Listing objects in {bucket} with prefix {prefix}");
        let mut result = Vec::new();
        let mut cont: Option<String> = None;

        loop {
            let response = with_retries(
                &description,
                target_label,
                op_label,
                &self.config.retries,
                &(self.client.clone(), bucket, prefix, cont),
                |(client, bucket, prefix, cont)| {
                    Box::pin(async move {
                        client
                            .list_objects_v2()
                            .bucket(*bucket)
                            .prefix(*prefix)
                            .set_continuation_token(cont.clone())
                            .send()
                            .await
                    })
                },
                Self::is_list_objects_v2_error_retriable,
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
        let description = format!("Downloading blob from {bucket}::{key:?}");
        let result = with_retries(
            &description,
            target_label,
            op_label,
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
            Err(err) => Err(err.to_string()),
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
        let description = format!("Downloading blob from {bucket}::{key:?}");
        let result = with_retries(
            &description,
            target_label,
            op_label,
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
            Err(err) => Err(err.to_string()),
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
        let description = format!("Getting metadata of blob storage entry {bucket}::{key:?}");
        let file_head_result = with_retries(
            &description,
            target_label,
            op_label,
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
                    let dir_marker_head_result = with_retries(
                        &description,
                        target_label,
                        op_label,
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
                        Err(err) => Err(err.to_string()),
                    }
                }
                err => Err(err.to_string()),
            },
            Err(err) => Err(err.to_string()),
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
        let description = format!("Uploading blob to {bucket}::{key:?}");

        with_retries(
            &description,
            target_label,
            op_label,
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
        )
        .await
        .map(|_| ())
        .map_err(|err| err.to_string())
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
        let description = format!("Deleting blob at {bucket}::{key:?}");

        with_retries(
            &description,
            target_label,
            op_label,
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

        let description = format!("Deleting blobs in {bucket}");

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

        with_retries(
            &description,
            target_label,
            op_label,
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
        let description = format!("Creating directory {bucket}::{key:?}");
        let marker = key.join("__dir_marker");

        with_retries(
            &description,
            target_label,
            op_label,
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
        let key = self.prefix_of(&namespace).join(path);

        Ok(self
            .list_objects(target_label, op_label, bucket, &key.to_string_lossy())
            .await?
            .iter()
            .flat_map(|obj| obj.key.as_ref().map(|k| Path::new(k).to_path_buf()))
            .collect::<Vec<_>>())
    }

    async fn delete_dir(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<(), String> {
        let bucket = self.bucket_of(&namespace);
        let key = self.prefix_of(&namespace).join(path);
        let description = format!("Deleting directory {bucket}::{key:?}");

        let to_delete = self
            .list_objects(target_label, op_label, bucket, &key.to_string_lossy())
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

        with_retries(
            &description,
            target_label,
            op_label,
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
        )
        .await
        .map_err(|err| err.to_string())?;

        Ok(())
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
        let description = format!("Checking existence of blob at {bucket}::{key:?}");
        let file_head_result = with_retries(
            &description,
            target_label,
            op_label,
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
        )
        .await;
        match file_head_result {
            Ok(_) => Ok(ExistsResult::File),
            Err(SdkError::ServiceError(service_error)) => match service_error.err() {
                HeadObjectError::NotFound(_) => {
                    let marker = key.join("__dir_marker");
                    let dir_marker_head_result = with_retries(
                        &description,
                        target_label,
                        op_label,
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
                    )
                    .await;
                    match dir_marker_head_result {
                        Ok(_) => Ok(ExistsResult::Directory),
                        Err(SdkError::ServiceError(service_error)) => match service_error.err() {
                            HeadObjectError::NotFound(_) => Ok(ExistsResult::DoesNotExist),
                            err => Err(err.to_string()),
                        },
                        Err(err) => Err(err.to_string()),
                    }
                }
                err => Err(err.to_string()),
            },
            Err(err) => Err(err.to_string()),
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
        let description =
            format!("Copying blob from {bucket}::{from_key:?} to {bucket}::{to_key:?}");

        with_retries(
            &description,
            target_label,
            op_label,
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
        )
        .await
        .map_err(|err| err.to_string())?;
        Ok(())
    }
}
