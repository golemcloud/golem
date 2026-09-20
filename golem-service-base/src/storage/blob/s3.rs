// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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
use crate::storage::blob::{
    BlobMetadata, BlobRangeError, BlobStorage, BlobStorageNamespace, ExistsResult, ListedBlob,
    blob_path_is_root, blob_path_to_string, blob_range, validate_relative_blob_path,
};
use anyhow::{Error, anyhow};
use async_trait::async_trait;
use aws_sdk_s3::Client;
use aws_sdk_s3::config::http::HttpResponse;
use aws_sdk_s3::config::interceptors::BeforeDeserializationInterceptorContextRef;
use aws_sdk_s3::config::{
    BehaviorVersion, ConfigBag, Credentials, Intercept, Region, RequestChecksumCalculation,
    RuntimeComponents,
};
use aws_sdk_s3::error::{BoxError, SdkError};
use aws_sdk_s3::operation::copy_object::CopyObjectError;
use aws_sdk_s3::operation::delete_objects::DeleteObjectsOutput;
use aws_sdk_s3::operation::get_object::GetObjectError;
use aws_sdk_s3::operation::get_object::GetObjectError::NoSuchKey;
use aws_sdk_s3::operation::head_object::HeadObjectError;
use aws_sdk_s3::operation::put_object::PutObjectError;
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::types::{Delete, Object, ObjectIdentifier};
use bytes::{Buf, Bytes};
use futures::stream::BoxStream;
use futures::{TryFutureExt, TryStreamExt};
use golem_common::model::Timestamp;
use golem_common::retries::with_retries_customized;
use http_body::SizeHint;
use http_body_util::BodyExt;
use http_body_util::combinators::BoxBody;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use tracing::info;

/// The largest number of keys that S3 accepts in one `DeleteObjects` request.
const MAX_KEYS_PER_DELETE_OBJECTS: usize = 1_000;

/// The backend uses the body of a response with this HTTP status, and without a
/// `Content-Range`, as the full object, unless its `Content-Length` shows that `end` is not in
/// the object (`response_body`). RFC 9110 lets a server ignore the range and give this
/// response (sections 14.2 and 15.5.17).
const HTTP_OK: u16 = 200;

/// A response with this HTTP status tells the backend that the range has no byte in the
/// object (RFC 9110, section 15.5.17). `get_raw_slice` gives a `BlobRangeError` for it.
const RANGE_NOT_SATISFIABLE: u16 = 416;

/// The name of the object that records a directory, because S3 has no directories.
const DIR_MARKER: &str = "__dir_marker";

#[derive(Debug)]
pub struct S3BlobStorage {
    client: aws_sdk_s3::Client,
    config: S3BlobStorageConfig,
}

/// Records the HTTP status of the response that the SDK makes the output of a request from.
///
/// The output of a request does not hold the status of its response. The SDK runs
/// `read_before_deserialization` with the response of an attempt, and then makes the output
/// of that attempt from that response (`try_attempt` in
/// `aws_smithy_runtime::client::orchestrator`). The interceptor stores the status of each
/// response that it gets, and `get` gives the status that it stored last. The output comes
/// from the last attempt, so `get` gives the status of the response of that attempt.
#[derive(Debug, Clone, Default)]
struct ResponseStatus(Arc<Mutex<Option<u16>>>);

impl ResponseStatus {
    /// Gives the status that the interceptor stored last, or `None` when it got no response.
    fn get(&self) -> Option<u16> {
        *self.0.lock().unwrap()
    }
}

impl Intercept for ResponseStatus {
    fn name(&self) -> &'static str {
        "ResponseStatus"
    }

    fn read_before_deserialization(
        &self,
        context: &BeforeDeserializationInterceptorContextRef<'_>,
        _runtime_components: &RuntimeComponents,
        _cfg: &mut ConfigBag,
    ) -> Result<(), BoxError> {
        *self.0.lock().unwrap() = Some(context.response().status().as_u16());
        Ok(())
    }
}

/// How the backend reads the body of a response to a ranged read. `response_body` selects the
/// variant from the status and the headers of the response, before the body is read.
enum ResponseBody {
    /// The backend uses the body as the bytes of the range. The range has `length` bytes. This
    /// is the variant of a response whose `Content-Range` gives the range.
    Range { length: u64 },
    /// The backend uses the body as the full object, and cuts the range out of it. This is
    /// the variant of a 200 response without a `Content-Range`, unless its `Content-Length`
    /// shows that `end` is not in the object. The backend does not check that the body is the
    /// object, so an error page with the status 200 gets this variant too.
    WholeObject,
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

        Self::with_sdk_config(config, s3_config_builder.build())
    }

    /// Makes a blob storage that sends its requests through a client with the given S3 settings.
    fn with_sdk_config(config: S3BlobStorageConfig, sdk_config: aws_sdk_s3::Config) -> Self {
        Self {
            client: aws_sdk_s3::Client::from_conf(sdk_config),
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
            BlobStorageNamespace::InitialAgentFiles { .. } => {
                &self.config.initial_agent_files_bucket
            }
            BlobStorageNamespace::Components { .. } => &self.config.components_bucket,
        }
    }

    fn prefix_of(&self, namespace: &BlobStorageNamespace) -> PathBuf {
        match namespace {
            BlobStorageNamespace::CompilationCache { environment_id }
            | BlobStorageNamespace::CustomStorage { environment_id }
            | BlobStorageNamespace::InitialAgentFiles { environment_id }
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
                agent_id,
                agent_mode,
            } => {
                let environment_id_string = environment_id.to_string();
                let agent_id_string = agent_id.to_string();
                let mode = super::agent_mode_prefix(*agent_mode);
                if self.config.object_prefix.is_empty() {
                    Path::new(mode)
                        .join(environment_id_string)
                        .join(agent_id_string)
                        .to_path_buf()
                } else {
                    Path::new(&self.config.object_prefix)
                        .join(mode)
                        .join(environment_id_string)
                        .join(agent_id_string)
                        .to_path_buf()
                }
            }
            BlobStorageNamespace::CompressedOplog {
                environment_id,
                component_id,
                agent_mode,
                ..
            } => {
                let environment_id_string = environment_id.to_string();
                let component_id_string = component_id.to_string();
                let mode = super::agent_mode_prefix(*agent_mode);
                if self.config.object_prefix.is_empty() {
                    Path::new(mode)
                        .join(environment_id_string)
                        .join(component_id_string)
                        .to_path_buf()
                } else {
                    Path::new(&self.config.object_prefix)
                        .join(mode)
                        .join(environment_id_string)
                        .join(component_id_string)
                        .to_path_buf()
                }
            }
        }
    }

    fn encode_copy_source_key(key: &str) -> String {
        let mut encoded = String::with_capacity(key.len());
        for b in key.bytes() {
            let ch = b as char;
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '~' | '/') {
                encoded.push(ch);
            } else {
                encoded.push_str(&format!("%{:02X}", b));
            }
        }
        encoded
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
        let prefix_str = blob_path_to_string(prefix)?;
        let prefix_with_slash = if prefix_str.ends_with('/') {
            prefix_str.clone()
        } else {
            format!("{prefix_str}/")
        };

        loop {
            let response = with_retries_customized(
                target_label,
                op_label,
                Some(format!("{bucket} - {prefix_str}")),
                &self.config.retries,
                &(self.client.clone(), bucket, prefix_with_slash.clone(), cont),
                |(client, bucket, prefix, cont)| {
                    Box::pin(async move {
                        client
                            .list_objects_v2()
                            .bucket(*bucket)
                            .prefix(prefix.clone())
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

    /// Returns whether any object exists under the given prefix (treated as a
    /// directory, i.e. with a trailing `/`). Used to detect implicit directories
    /// that have children but no explicit `__dir_marker`.
    async fn prefix_has_objects(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        bucket: &str,
        prefix: &Path,
    ) -> Result<bool, Error> {
        let prefix_str = blob_path_to_string(prefix)?;
        let prefix_with_slash = if prefix_str.ends_with('/') {
            prefix_str.clone()
        } else {
            format!("{prefix_str}/")
        };

        let response = with_retries_customized(
            target_label,
            op_label,
            Some(format!("{bucket} - {prefix_str}")),
            &self.config.retries,
            &(self.client.clone(), bucket, prefix_with_slash),
            |(client, bucket, prefix)| {
                Box::pin(async move {
                    client
                        .list_objects_v2()
                        .bucket(*bucket)
                        .prefix(prefix.clone())
                        // Only count objects strictly under the prefix, never an
                        // object whose key is exactly `<prefix>/` (a degenerate
                        // self-placeholder), so it is not mistaken for a child.
                        .start_after(prefix.clone())
                        .max_keys(1)
                        .send()
                        .await
                })
            },
            Self::is_list_objects_v2_error_retriable,
            Self::sdk_error_as_loggable_string,
            false,
        )
        .await?;

        Ok(!response.contents().is_empty())
    }

    /// Tells how the backend reads the body of a response to a ranged read, before the body is
    /// read.
    ///
    /// `status`, `content_range` and `content_length` are the status, the `Content-Range` and
    /// the `Content-Length` of the response.
    ///
    /// A `Content-Range` that gives the range `start` to `end` selects `Range`. A
    /// `Content-Range` that starts at `start` and ends before `end` gives a `BlobRangeError`.
    /// Any other `Content-Range` gives a different error. So does a range with more bytes than
    /// a `u64` counts, which is only the range 0 to `u64::MAX`.
    ///
    /// The backend uses the body of a 200 response without a `Content-Range` as the full object
    /// (RFC 9110, section 15.3.1). RFC 9110 lets a server ignore the range and give this
    /// response (sections 14.2 and 15.5.17). The backend uses the `Content-Length` of this
    /// response as the size of the object. A `Content-Length` that shows that `end` is not in
    /// the object gives a `BlobRangeError` before the body is read. Without a `Content-Length`,
    /// the backend uses the length of the body as the size. The backend does not check that the
    /// body is the object. A 200 response with a different body, for example an error page,
    /// gives the range of that body.
    ///
    /// The check of the `Content-Length` trusts the server. A server can send a
    /// `Content-Length` that is shorter than its body. Then a range with `end` at or after that
    /// `Content-Length` gets a `BlobRangeError`, even if the range is in the object. The guest
    /// gets that as invalid input. `DefaultBlobStoreService::get_data` in
    /// `golem_worker_executor::services::blob_store` maps a `BlobRangeError` to
    /// `BlobStoreError::InvalidInput`. `classify_blob_store_error` in
    /// `golem_worker_executor::durable_host::blobstore` makes that permanent. Without the
    /// check, the SDK rejects such a body (`ContentLengthEnforcingBody` in
    /// `aws_smithy_runtime`). `get_data` maps that error to `BlobStoreError::TransientBackend`,
    /// which `classify_blob_store_error` makes transient. The executor retries a transient
    /// error (`try_trigger_retry` in `golem_worker_executor::durable_host::durability`).
    ///
    /// Any other response without a `Content-Range` gives a different error. A 206 response
    /// holds a part of the object (section 15.3.7). Without a `Content-Range`, the backend does
    /// not know which part. Its `Content-Length` does not give the size of the object, and its
    /// body does not give the range.
    fn response_body(
        status: u16,
        content_range: Option<&str>,
        content_length: Option<i64>,
        start: u64,
        end: u64,
    ) -> Result<ResponseBody, Error> {
        let Some(content_range) = content_range else {
            let size = content_length.and_then(|length| u64::try_from(length).ok());
            return match (status, size) {
                (HTTP_OK, Some(size)) if end >= size => Err(BlobRangeError { start, end }.into()),
                (HTTP_OK, _) => Ok(ResponseBody::WholeObject),
                _ => Err(anyhow!(
                    "S3 returned the status {status} with no content range for the byte range {start}-{end}"
                )),
            };
        };
        let returned = content_range
            .strip_prefix("bytes ")
            .and_then(|value| value.split_once('/'))
            .and_then(|(range, _)| range.split_once('-'))
            .and_then(|(first, last)| first.parse::<u64>().ok().zip(last.parse::<u64>().ok()));
        let length = end.checked_sub(start).and_then(|last| last.checked_add(1));
        match (returned, length) {
            (Some((first, last)), Some(length)) if first == start && last == end => {
                Ok(ResponseBody::Range { length })
            }
            (Some((first, last)), _) if first == start && last < end => {
                Err(BlobRangeError { start, end }.into())
            }
            _ => Err(anyhow!(
                "S3 returned the content range {content_range:?} for the byte range {start}-{end}"
            )),
        }
    }

    /// Deletes objects in requests of at most [`MAX_KEYS_PER_DELETE_OBJECTS`] keys, one request
    /// at a time.
    ///
    /// An empty list sends no request. When the last attempt of a request has an error, the
    /// deletion stops and gives that error.
    async fn delete_keys(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        bucket: &str,
        keys: &[ObjectIdentifier],
    ) -> Result<(), Error> {
        futures::stream::iter(keys.chunks(MAX_KEYS_PER_DELETE_OBJECTS).map(Ok))
            .try_for_each(|chunk| {
                self.delete_objects_request(target_label, op_label, bucket, chunk)
            })
            .await
    }

    /// Sends one `DeleteObjects` request in quiet mode, with retries.
    ///
    /// A response that reports an error for a key is an error of that attempt, so the request
    /// goes again within the retry budget. The new attempt sends the keys that the attempt before
    /// deleted too. In quiet mode, S3 gives an error only for a key that it did not delete. A
    /// delete of a key that is not there is not an error, so a key that the attempt before
    /// deleted gives no error.
    async fn delete_objects_request(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        bucket: &str,
        keys: &[ObjectIdentifier],
    ) -> Result<(), Error> {
        let delete = Delete::builder()
            .set_objects(Some(keys.to_vec()))
            .quiet(true)
            .build()?;

        with_retries_customized(
            target_label,
            op_label,
            Some(format!("{bucket} - {} keys", keys.len())),
            &self.config.retries,
            &(self.client.clone(), bucket, delete),
            |(client, bucket, delete)| {
                Box::pin(async move {
                    let output = client
                        .delete_objects()
                        .bucket(*bucket)
                        .delete(delete.clone())
                        .send()
                        .await
                        .map_err(SdkErrorOrCustomError::sdk_error)?;
                    Self::key_error(&output, bucket, delete.objects().len())
                        .map_or(Ok(()), |err| Err(SdkErrorOrCustomError::custom_error(err)))
                })
            },
            |err| err.is_retriable(Self::is_delete_objects_error_retriable),
            SdkErrorOrCustomError::as_loggable,
            false,
        )
        .await
        .map_err(|err| match err {
            SdkErrorOrCustomError::SdkError(err) => Error::new(err),
            SdkErrorOrCustomError::CustomError(err) => err,
        })
    }

    /// Gives an error when a `DeleteObjects` response reports an error for a key.
    ///
    /// The message gives the number of keys that S3 did not delete, and the details of the first.
    fn key_error(output: &DeleteObjectsOutput, bucket: &str, requested: usize) -> Option<Error> {
        output.errors().first().map(|first| {
            anyhow!(
                "S3 did not delete {} of {requested} keys in bucket {bucket}; first: {}: {}: {}",
                output.errors().len(),
                first.key().unwrap_or_default(),
                first.code().unwrap_or_default(),
                first.message().unwrap_or_default(),
            )
        })
    }

    /// Tells whether a `GetObject` error is a missing key or a 416. The retry loop stops at
    /// such an error: the backend does not send the request again, does not write the error to
    /// the error log, and does not count it as a failure.
    ///
    /// What the backend then gives is set at each `get_object` call. `get_raw`, `get_stream`
    /// and `get_raw_slice` give `Ok(None)` for a missing key. `get_raw_slice` gives an
    /// `Err` that holds a `BlobRangeError` for a 416. `get_raw` and `get_stream` send no
    /// range, and give a 416 as an `Err` that holds the SDK error.
    fn is_final_get_object_error(error: &GetObjectError, response: &HttpResponse) -> bool {
        matches!(error, NoSuchKey(_)) || Self::is_range_not_satisfiable(response)
    }

    fn is_get_object_error_retriable(error: &SdkError<GetObjectError>) -> bool {
        match error {
            SdkError::ServiceError(service_error) => {
                !Self::is_final_get_object_error(service_error.err(), service_error.raw())
            }
            _ => true,
        }
    }

    /// Tells whether the status of a response is 416 (`RANGE_NOT_SATISFIABLE`).
    fn is_range_not_satisfiable(response: &HttpResponse) -> bool {
        response.status().as_u16() == RANGE_NOT_SATISFIABLE
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

    /// Gives the text that the retry loop (`with_retries_customized` in `golem_common::retries`)
    /// records for a `GetObject` error, or `None` for an error that the loop does not record and
    /// does not count as a failure.
    ///
    /// A missing key and a 416 (`is_final_get_object_error`) stay out of the error log and out
    /// of the failure counter. Every other error gets its text.
    fn get_object_error_as_loggable(error: &SdkError<GetObjectError>) -> Option<String> {
        match error {
            SdkError::ServiceError(service_error)
                if Self::is_final_get_object_error(service_error.err(), service_error.raw()) =>
            {
                None
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
        validate_relative_blob_path(path)?;
        let bucket = self.bucket_of(&namespace);
        let key = self.prefix_of(&namespace).join(path);
        let key_str = blob_path_to_string(&key)?;

        let result = with_retries_customized(
            target_label,
            op_label,
            Some(format!("{bucket} - {key:?}")),
            &self.config.retries,
            &(self.client.clone(), bucket, key_str),
            |(client, bucket, key)| {
                Box::pin(async move {
                    client
                        .get_object()
                        .bucket(*bucket)
                        .key(key.clone())
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
        validate_relative_blob_path(path)?;
        let bucket = self.bucket_of(&namespace);
        let key = self.prefix_of(&namespace).join(path);
        let key_str = blob_path_to_string(&key)?;

        let result = with_retries_customized(
            target_label,
            op_label,
            Some(format!("{bucket} - {key:?}")),
            &self.config.retries,
            &(self.client.clone(), bucket, key_str),
            |(client, bucket, key)| {
                Box::pin(async move {
                    client
                        .get_object()
                        .bucket(*bucket)
                        .key(key.clone())
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
        // A `start` after `end` is an invalid range (RFC 9110, section 14.1.1). RFC 9110 lets a
        // server ignore or reject it (section 14.2), so the backend sends no request for it.
        if start > end {
            return Err(BlobRangeError { start, end }.into());
        }
        validate_relative_blob_path(path)?;
        let bucket = self.bucket_of(&namespace);
        let key = self.prefix_of(&namespace).join(path);
        let key_str = blob_path_to_string(&key)?;

        let result = with_retries_customized(
            target_label,
            op_label,
            Some(format!("{bucket} - {key:?}")),
            &self.config.retries,
            &(self.client.clone(), bucket, key_str),
            |(client, bucket, key)| {
                Box::pin(async move {
                    let status = ResponseStatus::default();
                    client
                        .get_object()
                        .bucket(*bucket)
                        .key(key.clone())
                        .range(format!("bytes={start}-{end}"))
                        .customize()
                        .interceptor(status.clone())
                        .send()
                        .await
                        .map(|response| (response, status.get()))
                })
            },
            Self::is_get_object_error_retriable,
            Self::get_object_error_as_loggable,
            false,
        )
        .await;

        match result {
            Ok((response, status)) => {
                // The SDK makes an output only after the interceptor stored the status of a
                // response (see `ResponseStatus`).
                let status = status.ok_or_else(|| {
                    anyhow!("S3 gave an output without a response for the byte range {start}-{end}")
                })?;
                let response_body = Self::response_body(
                    status,
                    response.content_range.as_deref(),
                    response.content_length,
                    start,
                    end,
                )?;
                let body = response.body.collect().await?.to_vec();
                let bytes = match response_body {
                    // A body with another number of bytes than the range gives an error.
                    ResponseBody::Range { length } => {
                        let returned = body.len();
                        (u64::try_from(returned).ok() == Some(length))
                            .then_some(body)
                            .ok_or_else(|| {
                                anyhow!(
                                    "S3 returned {returned} bytes for the byte range {start}-{end}"
                                )
                            })?
                    }
                    // The rule of the default `get_raw_slice`, so every backend gives the same
                    // error for a range that is not in the object.
                    ResponseBody::WholeObject => blob_range(&body, start, end)?.to_vec(),
                };

                Ok(Some(bytes))
            }
            Err(SdkError::ServiceError(service_error))
                if Self::is_range_not_satisfiable(service_error.raw()) =>
            {
                Err(BlobRangeError { start, end }.into())
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
        validate_relative_blob_path(path)?;
        let bucket = self.bucket_of(&namespace);
        let key = self.prefix_of(&namespace).join(path);
        let key_str = blob_path_to_string(&key)?;
        let op_id = format!("{bucket} - {key:?}");

        let file_head_result = with_retries_customized(
            target_label,
            op_label,
            Some(op_id.clone()),
            &self.config.retries,
            &(self.client.clone(), bucket, key_str.clone()),
            |(client, bucket, key)| {
                Box::pin(async move {
                    client
                        .head_object()
                        .bucket(*bucket)
                        .key(key.clone())
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
                    let marker = key.join(DIR_MARKER);
                    let marker_str = blob_path_to_string(&marker)?;
                    let dir_marker_head_result = with_retries_customized(
                        target_label,
                        op_label,
                        Some(op_id),
                        &self.config.retries,
                        &(self.client.clone(), bucket, marker_str),
                        |(client, bucket, marker)| {
                            Box::pin(async move {
                                client
                                    .head_object()
                                    .bucket(*bucket)
                                    .key(marker.clone())
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
        validate_relative_blob_path(path)?;
        let bucket = self.bucket_of(&namespace);
        let key = self.prefix_of(&namespace).join(path);
        let key_str = blob_path_to_string(&key)?;
        let bytes = Bytes::copy_from_slice(data);

        with_retries_customized(
            target_label,
            op_label,
            Some(format!("{bucket} - {key:?}")),
            &self.config.retries,
            &(self.client.clone(), bucket, key_str, bytes),
            |(client, bucket, key, bytes)| {
                Box::pin(async move {
                    client
                        .put_object()
                        .bucket(*bucket)
                        .key(key.clone())
                        .body(ByteStream::from(bytes.clone()))
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
        validate_relative_blob_path(path)?;
        let bucket = self.bucket_of(&namespace);
        let key = self.prefix_of(&namespace).join(path);
        let key_str = blob_path_to_string(&key)?;

        fn go<'a>(
            args: &'a (
                Client,
                &String,
                String,
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
                    .key(key.clone())
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
            &(self.client.clone(), bucket, key_str, stream),
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
        validate_relative_blob_path(path)?;
        let bucket = self.bucket_of(&namespace);
        let key = self.prefix_of(&namespace).join(path);
        let key_str = blob_path_to_string(&key)?;

        with_retries_customized(
            target_label,
            op_label,
            Some(format!("{bucket} - {key:?}")),
            &self.config.retries,
            &(self.client.clone(), bucket, key_str),
            |(client, bucket, key)| {
                Box::pin(async move {
                    client
                        .delete_object()
                        .bucket(*bucket)
                        .key(key.clone())
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
        for path in paths {
            validate_relative_blob_path(path)?;
        }
        let bucket = self.bucket_of(&namespace);
        let prefix = self.prefix_of(&namespace);

        let to_delete = paths
            .iter()
            .map(|path| {
                let key = prefix.join(path);
                let key = blob_path_to_string(&key)?;
                ObjectIdentifier::builder()
                    .key(key)
                    .build()
                    .map_err(|e| e.into())
            })
            .collect::<Result<Vec<_>, Error>>()?;

        self.delete_keys(target_label, op_label, bucket, &to_delete)
            .await
    }

    async fn create_dir(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<(), Error> {
        validate_relative_blob_path(path)?;
        let bucket = self.bucket_of(&namespace);
        let key = self.prefix_of(&namespace).join(path);
        let marker = key.join(DIR_MARKER);
        let marker_str = blob_path_to_string(&marker)?;

        with_retries_customized(
            target_label,
            op_label,
            Some(format!("{bucket} - {key:?}")),
            &self.config.retries,
            &(self.client.clone(), bucket, marker_str),
            |(client, bucket, marker)| {
                Box::pin(async move {
                    client
                        .put_object()
                        .bucket(*bucket)
                        .key(marker.clone())
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
        validate_relative_blob_path(path)?;
        let bucket = self.bucket_of(&namespace);
        let namespace_root = self.prefix_of(&namespace);
        let key = namespace_root.join(path);

        Ok(self
            .list_objects(target_label, op_label, bucket, &key)
            .await?
            .iter()
            .flat_map(|obj| obj.key.as_ref().map(|k| Path::new(k).to_path_buf()))
            .filter_map(|path| {
                let is_dir_marker = path.file_name().and_then(|s| s.to_str()) == Some(DIR_MARKER);
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

    async fn list_blobs_below(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Box<[ListedBlob]>, Error> {
        validate_relative_blob_path(path)?;
        let bucket = self.bucket_of(&namespace);
        let namespace_root = self.prefix_of(&namespace);
        let key = namespace_root.join(path);

        self.list_objects(target_label, op_label, bucket, &key)
            .await?
            .iter()
            .filter_map(|object| object.key().map(|key| (key, object.size())))
            // S3 has no directories, so it records one as an object: a key that ends with `/`,
            // which other S3 tools write, or the marker that `create_dir` writes.
            .filter(|(key, _)| {
                !key.ends_with('/')
                    && Path::new(key).file_name().and_then(|name| name.to_str()) != Some(DIR_MARKER)
            })
            .map(|(key, size)| {
                let size = size.ok_or_else(|| anyhow!("S3 gave no size for the key {key}"))?;
                Ok::<_, Error>(ListedBlob {
                    path: Path::new(key).strip_prefix(&namespace_root)?.into(),
                    size: u64::try_from(size)?,
                })
            })
            .collect()
    }

    async fn delete_dir(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<bool, Error> {
        validate_relative_blob_path(path)?;

        if blob_path_is_root(path) {
            return Ok(false);
        }

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

        self.delete_keys(target_label, op_label, bucket, &to_delete)
            .await?;

        Ok(has_entries)
    }

    async fn exists(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<ExistsResult, Error> {
        validate_relative_blob_path(path)?;
        let bucket = self.bucket_of(&namespace);
        let key = self.prefix_of(&namespace).join(path);
        let key_str = blob_path_to_string(&key)?;
        let op_id = format!("{bucket} - {key:?}");

        let file_head_result = with_retries_customized(
            target_label,
            op_label,
            Some(op_id.clone()),
            &self.config.retries,
            &(self.client.clone(), bucket, key_str.clone()),
            |(client, bucket, key)| {
                Box::pin(async move {
                    client
                        .head_object()
                        .bucket(*bucket)
                        .key(key.clone())
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
                    let marker = key.join(DIR_MARKER);
                    let marker_str = blob_path_to_string(&marker)?;
                    let dir_marker_head_result = with_retries_customized(
                        target_label,
                        op_label,
                        Some(op_id),
                        &self.config.retries,
                        &(self.client.clone(), bucket, marker_str),
                        |(client, bucket, marker)| {
                            Box::pin(async move {
                                client
                                    .head_object()
                                    .bucket(*bucket)
                                    .key(marker.clone())
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
                                HeadObjectError::NotFound(_) => {
                                    // S3 has no real directories: an implicit
                                    // directory can exist (because of nested
                                    // objects or markers) without an explicit
                                    // `__dir_marker` of its own. Match the
                                    // filesystem and in-memory backends by
                                    // reporting a directory whenever any object
                                    // exists under this path's prefix.
                                    if self
                                        .prefix_has_objects(target_label, op_label, bucket, &key)
                                        .await?
                                    {
                                        Ok(ExistsResult::Directory)
                                    } else {
                                        Ok(ExistsResult::DoesNotExist)
                                    }
                                }
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
        validate_relative_blob_path(from)?;
        validate_relative_blob_path(to)?;
        let bucket = self.bucket_of(&namespace);
        let from_key = self.prefix_of(&namespace).join(from);
        let to_key = self.prefix_of(&namespace).join(to);
        let from_key_str = blob_path_to_string(&from_key)?;
        let to_key_str = blob_path_to_string(&to_key)?;
        let encoded_from_key = Self::encode_copy_source_key(&from_key_str);

        with_retries_customized(
            target_label,
            op_label,
            Some(format!("{bucket} - {from_key:?} -> {to_key:?}")),
            &self.config.retries,
            &(self.client.clone(), bucket, encoded_from_key, to_key_str),
            |(client, bucket, encoded_from_key, to_key)| {
                Box::pin(async move {
                    client
                        .copy_object()
                        .bucket(*bucket)
                        .copy_source(format!("/{}/{}", *bucket, encoded_from_key))
                        .key(to_key.clone())
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
        self.hint
    }
}

#[cfg(test)]
mod tests;
