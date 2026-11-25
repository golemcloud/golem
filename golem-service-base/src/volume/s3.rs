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

use super::connection::ConnectionPolicy;
use super::keys::{
    check_not_marker, full_key, relative_key, strong_etag, validate_delete, validate_parts,
    validate_read, write_key,
};
use super::signing::Signing;
use super::transport::{SystemResolver, Transport};
use super::{
    BucketVersioning, LifecycleRule, LifecycleRuleStatus, ListedObject, NoncurrentExpiration,
    ObjectBody, ObjectMetadata, ObjectPage, ObjectRead, ObjectVersion, ResponseValue, UploadedPart,
    VolumeClient, VolumeError, WriteCondition, WriteTarget,
};
use crate::aws::AwsCredentials;
use async_trait::async_trait;
use aws_sdk_s3::config::{BehaviorVersion, Region, RequestChecksumCalculation};
use aws_sdk_s3::error::{ProvideErrorMetadata, SdkError};
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::types::{
    BucketVersioningStatus, CompletedMultipartUpload, CompletedPart, EncodingType,
    ExpirationStatus, ServerSideEncryption,
};
use aws_smithy_runtime_api::box_error::BoxError;
use aws_smithy_runtime_api::client::http::{SharedHttpConnector, http_client_fn};
use aws_smithy_runtime_api::client::identity::SharedIdentityResolver;
use aws_smithy_runtime_api::client::interceptors::{
    Intercept, context::BeforeDeserializationInterceptorContextRef,
};
use aws_smithy_runtime_api::client::runtime_components::RuntimeComponents;
use aws_smithy_types::{config_bag::ConfigBag, retry::RetryConfig, timeout::TimeoutConfig};
use bytes::Bytes;
use std::fmt::{Debug, Formatter};
use std::ops::RangeInclusive;
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tracing::instrument::WithSubscriber;
use tracing::subscriber::NoSubscriber;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Encryption {
    S3,
    Kms { key_id: Box<str> },
}

#[derive(Debug)]
pub struct S3VolumeConfig {
    pub endpoint: Option<Box<str>>,
    pub region: Arc<str>,
    pub bucket: Box<str>,
    /// An empty prefix selects the bucket root. A nonempty prefix ends in `/`.
    pub prefix: Box<str>,
    pub encryption: Option<Encryption>,
}

/// S3 operations with static credentials, conditional writes and checked connections.
/// No operation retries a request or follows a redirect.
/// SDK tracing is disabled because a response can reflect credentials in an error.
pub struct S3Volume {
    client: aws_sdk_s3::Client,
    config: S3VolumeConfig,
}

impl Debug for S3Volume {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("S3Volume")
    }
}

#[derive(Clone, Debug, Default)]
struct StatusObserver(Arc<OnceLock<u16>>);

impl Intercept for StatusObserver {
    fn name(&self) -> &'static str {
        "VolumeResponseStatus"
    }

    fn read_before_deserialization(
        &self,
        context: &BeforeDeserializationInterceptorContextRef<'_>,
        _: &RuntimeComponents,
        _: &mut ConfigBag,
    ) -> Result<(), BoxError> {
        let _ = self.0.set(context.response().status().as_u16());
        Ok(())
    }
}

fn readonly_result<T, E: ProvideErrorMetadata>(
    result: Result<T, SdkError<E>>,
    status: Option<u16>,
    expected: u16,
) -> Result<T, VolumeError> {
    match result {
        Ok(result) if status == Some(expected) => Ok(result),
        Ok(_) => Err(VolumeError::InvalidResponse),
        Err(error)
            if matches!(error, SdkError::ServiceError(_))
                && status.is_some_and(|status| (200..300).contains(&status)) =>
        {
            Err(VolumeError::InvalidResponse)
        }
        Err(error) => Err(failure(error, None)),
    }
}

fn normalized_version_id(id: Option<&str>) -> Result<Option<&str>, VolumeError> {
    match id {
        Some("") => Err(VolumeError::InvalidResponse),
        Some("null") | None => Ok(None),
        Some(id) => Ok(Some(id)),
    }
}

fn decoded_list_key(key: &str) -> Result<Box<str>, VolumeError> {
    let mut bytes = key.bytes();
    let decoded = std::iter::from_fn(|| {
        bytes.next().map(|byte| {
            if byte == b'%' {
                let high = bytes.next().and_then(|byte| char::from(byte).to_digit(16));
                let low = bytes.next().and_then(|byte| char::from(byte).to_digit(16));
                high.zip(low)
                    .map(|(high, low)| (high * 16 + low) as u8)
                    .ok_or(VolumeError::InvalidResponse)
            } else {
                Ok(byte)
            }
        })
    })
    .collect::<Result<Vec<_>, _>>()?;
    String::from_utf8(decoded)
        .map(String::into_boxed_str)
        .map_err(|_| VolumeError::InvalidResponse)
}

fn version(etag: Option<&str>, version_id: Option<&str>) -> Result<ObjectVersion, VolumeError> {
    Ok(ObjectVersion {
        etag: etag
            .filter(|etag| strong_etag(etag))
            .ok_or(VolumeError::InvalidResponse)?
            .into(),
        version_id: normalized_version_id(version_id)?.map(Into::into),
    })
}

fn lifecycle_rule(rule: aws_sdk_s3::types::LifecycleRule) -> Result<LifecycleRule, VolumeError> {
    let status = match rule.status() {
        ExpirationStatus::Enabled => LifecycleRuleStatus::Enabled,
        ExpirationStatus::Disabled => LifecycleRuleStatus::Disabled,
        other => LifecycleRuleStatus::Unknown(ResponseValue::new(other.as_str().into())),
    };
    #[expect(
        deprecated,
        reason = "S3 responses can contain a top-level rule prefix"
    )]
    let rule_prefix = rule.prefix();
    let prefix = rule
        .filter()
        .and_then(|filter| {
            filter
                .prefix()
                .or_else(|| filter.and().and_then(|and| and.prefix()))
        })
        .or(rule_prefix)
        .map(|prefix| ResponseValue::new(prefix.into()));
    let noncurrent_expiration = rule
        .noncurrent_version_expiration()
        .map(|expiration| {
            let days = expiration
                .noncurrent_days()
                .filter(|days| *days > 0)
                .ok_or(VolumeError::InvalidResponse)?;
            let newer_versions = expiration.newer_noncurrent_versions();
            if newer_versions.is_some_and(|count| count <= 0) {
                return Err(VolumeError::InvalidResponse);
            }
            Ok(NoncurrentExpiration {
                days,
                newer_versions,
            })
        })
        .transpose()?;
    Ok(LifecycleRule {
        status,
        prefix,
        noncurrent_expiration,
    })
}

fn check_reference(
    requested_version: Option<&str>,
    requested_etag: Option<&str>,
    actual: &ObjectVersion,
) -> Result<(), VolumeError> {
    if requested_version
        .is_some_and(|id| actual.version_id.as_deref() != (id != "null").then_some(id))
        || requested_etag.is_some_and(|etag| actual.etag.as_ref() != etag)
    {
        Err(VolumeError::InvalidResponse)
    } else {
        Ok(())
    }
}

fn check_range(
    requested: Option<&RangeInclusive<u64>>,
    content_range: Option<&str>,
    length: u64,
) -> Result<(), VolumeError> {
    let Some(requested) = requested else {
        return if content_range.is_none() {
            Ok(())
        } else {
            Err(VolumeError::InvalidResponse)
        };
    };
    let parse = || {
        let (range, total) = content_range?.strip_prefix("bytes ")?.split_once('/')?;
        let (start, end) = range.split_once('-')?;
        Some((
            start.parse::<u64>().ok()?,
            end.parse::<u64>().ok()?,
            total.parse::<u64>().ok()?,
        ))
    };
    let (start, end, total) = parse().ok_or(VolumeError::InvalidResponse)?;
    if start != *requested.start()
        || total == 0
        || end != (*requested.end()).min(total - 1)
        || end.checked_sub(start).and_then(|n| n.checked_add(1)) != Some(length)
    {
        Err(VolumeError::InvalidResponse)
    } else {
        Ok(())
    }
}

fn encryption_headers(
    encryption: Option<&Encryption>,
) -> (Option<ServerSideEncryption>, Option<String>) {
    match encryption {
        Some(Encryption::S3) => (Some(ServerSideEncryption::Aes256), None),
        Some(Encryption::Kms { key_id }) => {
            (Some(ServerSideEncryption::AwsKms), Some(key_id.to_string()))
        }
        None => (None, None),
    }
}

fn condition_headers(
    condition: WriteCondition<'_>,
) -> Result<(Option<String>, Option<String>), VolumeError> {
    match condition {
        WriteCondition::Absent => Ok((None, Some("*".to_owned()))),
        WriteCondition::Matches(etag) if strong_etag(etag) => Ok((Some(etag.to_owned()), None)),
        _ => Err(VolumeError::InvalidRequest),
    }
}

fn failure<E: ProvideErrorMetadata>(
    error: SdkError<E>,
    condition: Option<WriteCondition<'_>>,
) -> VolumeError {
    if matches!(error, SdkError::ConstructionFailure(_)) {
        return VolumeError::InvalidRequest;
    }
    let status = error
        .raw_response()
        .map(|response| response.status().as_u16());
    classify_failure(
        status,
        error
            .as_service_error()
            .and_then(ProvideErrorMetadata::code),
        condition,
    )
}

fn classify_failure(
    status: Option<u16>,
    code: Option<&str>,
    condition: Option<WriteCondition<'_>>,
) -> VolumeError {
    match code {
        Some("PreconditionFailed") if matches!(condition, Some(WriteCondition::Absent)) => {
            VolumeError::Exists
        }
        Some("PreconditionFailed" | "ConditionalRequestConflict") => VolumeError::Conflict,
        Some("NoSuchKey" | "NotFound") if matches!(condition, Some(WriteCondition::Matches(_))) => {
            VolumeError::Conflict
        }
        Some("NoSuchKey" | "NotFound" | "NoSuchUpload" | "NoSuchBucket") => VolumeError::NotFound,
        Some("AccessDenied" | "ExpiredToken" | "InvalidToken" | "TokenRefreshRequired") => {
            VolumeError::Denied
        }
        Some("NotImplemented") => VolumeError::Unsupported,
        _ => match status {
            Some(412) if matches!(condition, Some(WriteCondition::Absent)) => VolumeError::Exists,
            Some(409 | 412) => VolumeError::Conflict,
            Some(404)
                if code.is_none() && matches!(condition, Some(WriteCondition::Matches(_))) =>
            {
                VolumeError::Conflict
            }
            Some(404) => VolumeError::NotFound,
            Some(401 | 403) => VolumeError::Denied,
            Some(405 | 501) => VolumeError::Unsupported,
            _ => VolumeError::RequestFailed,
        },
    }
}

impl S3Volume {
    pub fn new(
        config: S3VolumeConfig,
        credentials: AwsCredentials,
        policy: ConnectionPolicy,
    ) -> Result<Self, VolumeError> {
        relative_key(&config.prefix)?;
        if config.bucket.is_empty()
            || config.region.is_empty()
            || (!config.prefix.is_empty() && !config.prefix.ends_with('/'))
        {
            return Err(VolumeError::InvalidRequest);
        }
        if let Some(endpoint) = &config.endpoint {
            let uri: http::Uri = endpoint.parse().map_err(|_| VolumeError::InvalidRequest)?;
            if !matches!(uri.scheme_str(), Some("http" | "https"))
                || uri.host().is_none()
                || uri.authority().is_some_and(|a| a.as_str().contains('@'))
                || uri.query().is_some()
            {
                return Err(VolumeError::InvalidRequest);
            }
        }
        let transport = SharedHttpConnector::new(
            Transport::new(policy, Arc::new(SystemResolver))
                .map_err(|_| VolumeError::InvalidRequest)?,
        );
        let mut sdk = aws_sdk_s3::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new(config.region.to_string()))
            .force_path_style(true)
            .http_client(http_client_fn(move |_, _| transport.clone()))
            .retry_config(RetryConfig::disabled())
            .timeout_config(
                TimeoutConfig::builder()
                    .operation_timeout(Duration::from_secs(60))
                    .build(),
            )
            .request_checksum_calculation(RequestChecksumCalculation::WhenRequired)
            .push_auth_scheme(Signing {
                region: config.region.clone(),
                identity: SharedIdentityResolver::new(credentials),
            });
        if let Some(endpoint) = &config.endpoint {
            sdk = sdk.endpoint_url(endpoint.as_ref());
        }
        Ok(Self {
            client: aws_sdk_s3::Client::from_conf(sdk.build()),
            config,
        })
    }
}

#[async_trait]
impl VolumeClient for S3Volume {
    async fn stat(
        &self,
        key: &str,
        version_id: Option<&str>,
    ) -> Result<ObjectMetadata, VolumeError> {
        relative_key(key)?;
        let object_key = format!("{}{key}", self.config.prefix);
        if object_key.is_empty() {
            return Err(VolumeError::NotFound);
        }
        full_key(0, &object_key)?;
        let slash_key = object_key.ends_with('/');
        let status = StatusObserver::default();
        let result = self
            .client
            .head_object()
            .bucket(self.config.bucket.as_ref())
            .key(object_key)
            .set_version_id(version_id.map(str::to_owned))
            .customize()
            .interceptor(status.clone())
            .send()
            .with_subscriber(NoSubscriber::default())
            .await;
        let result = readonly_result(result, status.0.get().copied(), 200)?;
        let size = result
            .content_length()
            .and_then(|n| u64::try_from(n).ok())
            .ok_or(VolumeError::InvalidResponse)?;
        let version = version(result.e_tag(), result.version_id())?;
        check_reference(version_id, None, &version)?;
        if size == 0 && slash_key {
            return Err(VolumeError::NotFound);
        }
        Ok(ObjectMetadata {
            key: key.into(),
            size,
            version,
        })
    }

    /// Returns one recursive page. The continuation token is opaque.
    /// Sends one ListObjectsV2 request and takes each key, size and ETag from that page.
    /// Hides zero-byte keys ending in `/`. The page has no version IDs.
    async fn list(
        &self,
        prefix: &str,
        continuation: Option<&str>,
    ) -> Result<ObjectPage, VolumeError> {
        relative_key(prefix)?;
        let requested_prefix = format!("{}{prefix}", self.config.prefix);
        let status = StatusObserver::default();
        let result = self
            .client
            .list_objects_v2()
            .bucket(self.config.bucket.as_ref())
            .prefix(&requested_prefix)
            .encoding_type(EncodingType::Url)
            .set_continuation_token(continuation.map(str::to_owned))
            .customize()
            .interceptor(status.clone())
            .send()
            .with_subscriber(NoSubscriber::default())
            .await;
        let result = readonly_result(result, status.0.get().copied(), 200)?;
        if result.encoding_type() != Some(&EncodingType::Url) {
            return Err(VolumeError::InvalidResponse);
        }
        match (result.is_truncated(), result.next_continuation_token()) {
            (Some(true), Some(next)) if !next.is_empty() && Some(next) != continuation => (),
            (Some(false), None) => (),
            _ => return Err(VolumeError::InvalidResponse),
        }
        let objects = result
            .contents()
            .iter()
            .try_fold(
                Vec::with_capacity(result.contents().len()),
                |mut objects, object| {
                    let object_key =
                        decoded_list_key(object.key().ok_or(VolumeError::InvalidResponse)?)?;
                    if !object_key.starts_with(&requested_prefix) {
                        return Err(VolumeError::InvalidResponse);
                    }
                    if full_key(0, &object_key).is_err() {
                        return Ok(objects);
                    }
                    let key = object_key
                        .strip_prefix(self.config.prefix.as_ref())
                        .ok_or(VolumeError::InvalidResponse)?;
                    if relative_key(key).is_err() {
                        return Ok(objects);
                    }
                    let size = object
                        .size()
                        .and_then(|size| u64::try_from(size).ok())
                        .ok_or(VolumeError::InvalidResponse)?;
                    let etag = object
                        .e_tag()
                        .filter(|etag| strong_etag(etag))
                        .ok_or(VolumeError::InvalidResponse)?;
                    if size != 0 || !object_key.ends_with('/') {
                        objects.push(ListedObject {
                            key: key.into(),
                            size,
                            etag: etag.into(),
                        });
                    }
                    Ok::<_, VolumeError>(objects)
                },
            )?
            .into_boxed_slice();
        Ok(ObjectPage {
            objects,
            continuation: result.next_continuation_token().map(Into::into),
        })
    }

    async fn read(
        &self,
        key: &str,
        reference: &ObjectVersion,
        range: Option<RangeInclusive<u64>>,
    ) -> Result<ObjectRead, VolumeError> {
        relative_key(key)?;
        validate_read(reference, range.as_ref())?;
        let version_id = reference.version_id.as_deref();
        let object_key = format!("{}{key}", self.config.prefix);
        if object_key.is_empty() {
            return Err(VolumeError::NotFound);
        }
        full_key(0, &object_key)?;
        let slash_key = object_key.ends_with('/');
        let status = StatusObserver::default();
        let result = self
            .client
            .get_object()
            .bucket(self.config.bucket.as_ref())
            .key(&object_key)
            .set_version_id(version_id.map(str::to_owned))
            .if_match(reference.etag.as_ref())
            .set_range(range.as_ref().map(|r| {
                // S3 content lengths are signed. This offset is beyond every valid object byte.
                let limit = i64::MAX as u64;
                format!(
                    "bytes={}-{}",
                    (*r.start()).min(limit),
                    (*r.end()).min(limit)
                )
            }))
            .customize()
            .interceptor(status.clone())
            .send()
            .with_subscriber(NoSubscriber::default())
            .await;
        let result = match result {
            Err(error)
                if range.is_some()
                    && matches!(error, SdkError::ServiceError(_))
                    && error
                        .raw_response()
                        .is_some_and(|response| response.status().as_u16() == 416)
                    && matches!(
                        error
                            .as_service_error()
                            .and_then(ProvideErrorMetadata::code),
                        None | Some("InvalidRange" | "RequestedRangeNotSatisfiable")
                    ) =>
            {
                let head_status = StatusObserver::default();
                let head = self
                    .client
                    .head_object()
                    .bucket(self.config.bucket.as_ref())
                    .key(object_key)
                    .set_version_id(version_id.map(str::to_owned))
                    .if_match(reference.etag.as_ref())
                    .customize()
                    .interceptor(head_status.clone())
                    .send()
                    .with_subscriber(NoSubscriber::default())
                    .await;
                let head = readonly_result(head, head_status.0.get().copied(), 200)?;
                let size = head
                    .content_length()
                    .and_then(|n| u64::try_from(n).ok())
                    .ok_or(VolumeError::InvalidResponse)?;
                let version = version(head.e_tag(), head.version_id())?;
                check_reference(
                    Some(version_id.unwrap_or("null")),
                    Some(&reference.etag),
                    &version,
                )?;
                if size == 0 && slash_key {
                    return Err(VolumeError::NotFound);
                }
                if range.as_ref().is_some_and(|range| *range.start() >= size) {
                    return Ok(ObjectRead {
                        version,
                        length: 0,
                        body: ObjectBody::from_bytes(Bytes::new()),
                    });
                }
                return Err(VolumeError::InvalidResponse);
            }
            result => readonly_result(
                result,
                status.0.get().copied(),
                if range.is_some() { 206 } else { 200 },
            )?,
        };
        let length = result
            .content_length()
            .and_then(|n| u64::try_from(n).ok())
            .ok_or(VolumeError::InvalidResponse)?;
        let version = version(result.e_tag(), result.version_id())?;
        check_reference(
            Some(version_id.unwrap_or("null")),
            Some(&reference.etag),
            &version,
        )?;
        check_range(range.as_ref(), result.content_range(), length)?;
        if length == 0 && slash_key {
            return Err(VolumeError::NotFound);
        }
        Ok(ObjectRead {
            version,
            length,
            body: ObjectBody::from_stream(futures::stream::try_unfold(
                result.body,
                |mut body| async move {
                    body.try_next()
                        .with_subscriber(NoSubscriber::default())
                        .await
                        .map_err(|_| VolumeError::RequestFailed)
                        .map(|chunk| chunk.map(|chunk| (chunk, body)))
                },
            )),
        })
    }

    async fn put(
        &self,
        target: WriteTarget<'_>,
        condition: WriteCondition<'_>,
        body: Bytes,
    ) -> Result<ObjectVersion, VolumeError> {
        let key = write_key(&self.config.prefix, target)?;
        check_not_marker(&key, body.is_empty())?;
        let (if_match, if_none_match) = condition_headers(condition)?;
        let (algorithm, kms_key) = encryption_headers(self.config.encryption.as_ref());
        let request = self
            .client
            .put_object()
            .bucket(self.config.bucket.as_ref())
            .key(key)
            .set_if_match(if_match)
            .set_if_none_match(if_none_match)
            .set_server_side_encryption(algorithm)
            .set_ssekms_key_id(kms_key)
            .body(ByteStream::from(body));
        let result = request
            .send()
            .with_subscriber(NoSubscriber::default())
            .await
            .map_err(|error| failure(error, Some(condition)))?;
        version(result.e_tag(), result.version_id()).map_err(|_| VolumeError::RequestFailed)
    }

    /// The reference size and ETag describe the same observed object. Folder markers are not files.
    async fn delete(
        &self,
        target: WriteTarget<'_>,
        reference: &ObjectMetadata,
    ) -> Result<Option<ResponseValue<Arc<str>>>, VolumeError> {
        let key = write_key("", target)?;
        validate_delete(&key, reference)?;
        check_not_marker(&key, reference.size == 0)?;
        let object_key = [self.config.prefix.as_ref(), key.as_ref()].concat();
        full_key(0, &object_key)?;
        let etag = reference.version.etag.as_ref();
        let (if_match, _) = condition_headers(WriteCondition::Matches(etag))?;
        let result = self
            .client
            .delete_object()
            .bucket(self.config.bucket.as_ref())
            .key(object_key)
            .set_if_match(if_match)
            .send()
            .with_subscriber(NoSubscriber::default())
            .await
            .map_err(|error| failure(error, Some(WriteCondition::Matches(etag))))?;
        Ok(normalized_version_id(result.version_id())
            .map_err(|_| VolumeError::RequestFailed)?
            .map(|id| ResponseValue::new(id.into())))
    }

    async fn start_multipart(
        &self,
        target: WriteTarget<'_>,
    ) -> Result<ResponseValue<Arc<str>>, VolumeError> {
        let (algorithm, kms_key) = encryption_headers(self.config.encryption.as_ref());
        let request = self
            .client
            .create_multipart_upload()
            .bucket(self.config.bucket.as_ref())
            .key(write_key(&self.config.prefix, target)?)
            .set_server_side_encryption(algorithm)
            .set_ssekms_key_id(kms_key);
        let result = request
            .send()
            .with_subscriber(NoSubscriber::default())
            .await
            .map_err(|error| failure(error, None))?;
        result
            .upload_id()
            .filter(|id| !id.is_empty())
            .map(|id| ResponseValue::new(id.into()))
            .ok_or(VolumeError::InvalidResponse)
    }

    async fn upload_part(
        &self,
        target: WriteTarget<'_>,
        upload_id: &str,
        number: i32,
        body: Bytes,
    ) -> Result<UploadedPart, VolumeError> {
        if !(1..=10000).contains(&number) {
            return Err(VolumeError::InvalidRequest);
        }
        let size = body.len() as u64;
        let result = self
            .client
            .upload_part()
            .bucket(self.config.bucket.as_ref())
            .key(write_key(&self.config.prefix, target)?)
            .upload_id(upload_id)
            .part_number(number)
            .body(ByteStream::from(body))
            .send()
            .with_subscriber(NoSubscriber::default())
            .await
            .map_err(|error| failure(error, None))?;
        Ok(UploadedPart {
            number,
            etag: result
                .e_tag()
                .filter(|etag| strong_etag(etag))
                .ok_or(VolumeError::InvalidResponse)?
                .into(),
            size,
        })
    }

    async fn complete_multipart(
        &self,
        target: WriteTarget<'_>,
        upload_id: &str,
        parts: &[UploadedPart],
        condition: WriteCondition<'_>,
    ) -> Result<ObjectVersion, VolumeError> {
        validate_parts(parts)?;
        let key = write_key(&self.config.prefix, target)?;
        check_not_marker(&key, parts.iter().all(|part| part.size == 0))?;
        let parts = parts
            .iter()
            .map(|part| {
                CompletedPart::builder()
                    .part_number(part.number)
                    .e_tag(part.etag.as_ref())
                    .build()
            })
            .collect::<Vec<_>>();
        let (if_match, if_none_match) = condition_headers(condition)?;
        let result = self
            .client
            .complete_multipart_upload()
            .bucket(self.config.bucket.as_ref())
            .key(key)
            .upload_id(upload_id)
            .multipart_upload(
                CompletedMultipartUpload::builder()
                    .set_parts(Some(parts))
                    .build(),
            )
            .set_if_match(if_match)
            .set_if_none_match(if_none_match)
            .send()
            .with_subscriber(NoSubscriber::default())
            .await
            .map_err(|error| failure(error, Some(condition)))?;
        version(result.e_tag(), result.version_id()).map_err(|_| VolumeError::RequestFailed)
    }

    async fn abort_multipart(
        &self,
        target: WriteTarget<'_>,
        upload_id: &str,
    ) -> Result<(), VolumeError> {
        self.client
            .abort_multipart_upload()
            .bucket(self.config.bucket.as_ref())
            .key(write_key(&self.config.prefix, target)?)
            .upload_id(upload_id)
            .send()
            .with_subscriber(NoSubscriber::default())
            .await
            .map_err(|error| failure(error, None))?;
        Ok(())
    }

    async fn versioning(&self) -> Result<BucketVersioning, VolumeError> {
        let status = StatusObserver::default();
        let result = self
            .client
            .get_bucket_versioning()
            .bucket(self.config.bucket.as_ref())
            .customize()
            .interceptor(status.clone())
            .send()
            .with_subscriber(NoSubscriber::default())
            .await;
        let result = readonly_result(result, status.0.get().copied(), 200)?;
        Ok(match result.status() {
            None => BucketVersioning::Unversioned,
            Some(BucketVersioningStatus::Enabled) => BucketVersioning::Enabled,
            Some(BucketVersioningStatus::Suspended) => BucketVersioning::Suspended,
            Some(other) => BucketVersioning::Unknown(ResponseValue::new(other.as_str().into())),
        })
    }

    async fn lifecycle(&self) -> Result<Box<[LifecycleRule]>, VolumeError> {
        let status = StatusObserver::default();
        let result = self
            .client
            .get_bucket_lifecycle_configuration()
            .bucket(self.config.bucket.as_ref())
            .customize()
            .interceptor(status.clone())
            .send()
            .with_subscriber(NoSubscriber::default())
            .await;
        let result = match result {
            Err(error)
                if error
                    .as_service_error()
                    .and_then(ProvideErrorMetadata::code)
                    == Some("NoSuchLifecycleConfiguration") =>
            {
                return Ok(Box::new([]));
            }
            result => readonly_result(result, status.0.get().copied(), 200)?,
        };
        result
            .rules
            .unwrap_or_default()
            .into_iter()
            .map(lifecycle_rule)
            .collect()
    }
}

#[cfg(test)]
mod tests;
