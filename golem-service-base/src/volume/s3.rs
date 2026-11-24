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
use super::signing::Signing;
use super::transport::{SystemResolver, Transport};
use crate::aws::AwsCredentials;
use aws_sdk_s3::config::{BehaviorVersion, Region, RequestChecksumCalculation};
use aws_sdk_s3::error::{ProvideErrorMetadata, SdkError};
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::types::{
    BucketVersioningStatus, CompletedMultipartUpload, CompletedPart, LifecycleRule,
    ServerSideEncryption,
};
use aws_smithy_runtime_api::client::http::{SharedHttpConnector, http_client_fn};
use aws_smithy_runtime_api::client::identity::SharedIdentityResolver;
use aws_smithy_types::{retry::RetryConfig, timeout::TimeoutConfig};
use bytes::Bytes;
use std::fmt::{Debug, Formatter};
use std::ops::RangeInclusive;
use std::sync::Arc;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum VolumeError {
    #[error("invalid volume configuration or request")]
    InvalidRequest,
    #[error("volume object does not exist")]
    NotFound,
    #[error("volume object already exists")]
    Exists,
    #[error("volume object changed")]
    Conflict,
    #[error("volume request denied")]
    Denied,
    #[error("volume operation is not supported")]
    Unsupported,
    #[error("volume response is invalid")]
    InvalidResponse,
    #[error("volume request failed; a sent write may have taken effect")]
    RequestFailed,
}

#[derive(Clone, PartialEq, Eq)]
pub struct ObjectVersion {
    pub etag: Box<str>,
    pub version_id: Option<Box<str>>,
}

impl Debug for ObjectVersion {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("ObjectVersion")
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct ObjectMetadata {
    pub key: Box<str>,
    pub size: u64,
    pub version: ObjectVersion,
}

impl Debug for ObjectMetadata {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("ObjectMetadata")
    }
}

pub struct ObjectRead {
    pub version: ObjectVersion,
    pub length: u64,
    pub body: ObjectBody,
}

impl Debug for ObjectRead {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("ObjectRead")
    }
}

/// Object bytes with opaque errors, also when a response checksum fails.
pub struct ObjectBody(ByteStream);

impl Debug for ObjectBody {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("ObjectBody")
    }
}

impl ObjectBody {
    pub async fn next(&mut self) -> Result<Option<Bytes>, VolumeError> {
        self.0
            .try_next()
            .with_subscriber(NoSubscriber::default())
            .await
            .map_err(|_| VolumeError::RequestFailed)
    }

    pub async fn collect(self) -> Result<Bytes, VolumeError> {
        self.0
            .collect()
            .with_subscriber(NoSubscriber::default())
            .await
            .map(|body| body.into_bytes())
            .map_err(|_| VolumeError::RequestFailed)
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct ListedObject {
    pub key: Box<str>,
    pub size: u64,
    pub etag: Box<str>,
}

impl Debug for ListedObject {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("ListedObject")
    }
}

pub struct ObjectPage {
    pub objects: Box<[ListedObject]>,
    pub continuation: Option<Box<str>>,
}

impl Debug for ObjectPage {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("ObjectPage")
    }
}

#[derive(Debug, Clone, Copy)]
pub enum WriteCondition<'a> {
    Absent,
    /// One strong, quoted ETag. Wildcards and lists are not reference versions.
    Matches(&'a str),
}

/// Probe names are relative to `.golem/`. File writes cannot use that reserved prefix.
#[derive(Debug, Clone, Copy)]
pub enum WriteTarget<'a> {
    File(&'a str),
    Probe(&'a str),
}

#[derive(Clone, PartialEq, Eq)]
pub struct UploadedPart {
    pub number: i32,
    pub etag: Box<str>,
    pub size: u64,
}

impl Debug for UploadedPart {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("UploadedPart")
    }
}

/// A response payload with opaque Debug. Extracted data can contain credentials.
#[derive(Clone, PartialEq, Eq)]
pub struct ResponseValue<T>(T);

impl<T> ResponseValue<T> {
    pub fn payload(&self) -> &T {
        &self.0
    }

    pub fn into_payload(self) -> T {
        self.0
    }
}

impl<T> Debug for ResponseValue<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("ResponseValue")
    }
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

fn strong_etag(etag: &str) -> bool {
    etag.strip_prefix('"')
        .and_then(|tag| tag.strip_suffix('"'))
        .is_some_and(|tag| {
            tag.bytes()
                .all(|byte| byte >= 0x21 && byte != b'"' && byte != 0x7f)
        })
}

fn version(etag: Option<&str>, version_id: Option<&str>) -> Result<ObjectVersion, VolumeError> {
    Ok(ObjectVersion {
        etag: etag
            .filter(|etag| strong_etag(etag))
            .ok_or(VolumeError::InvalidResponse)?
            .into(),
        version_id: version_id.filter(|id| *id != "null").map(Into::into),
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

fn check_not_marker(key: &str, empty: bool) -> Result<(), VolumeError> {
    if empty && key.ends_with('/') {
        Err(VolumeError::InvalidRequest)
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
        Some("AccessDenied") => VolumeError::Denied,
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

fn relative_key(key: &str) -> Result<(), VolumeError> {
    if key.starts_with('/') || key.split('/').any(|part| matches!(part, "." | "..")) {
        Err(VolumeError::InvalidRequest)
    } else {
        Ok(())
    }
}

fn write_key(prefix: &str, target: WriteTarget<'_>) -> Result<String, VolumeError> {
    let (reserved, key) = match target {
        WriteTarget::File(key) if key != ".golem" && !key.starts_with(".golem/") => ("", key),
        WriteTarget::Probe(key) => (".golem/", key),
        _ => return Err(VolumeError::InvalidRequest),
    };
    relative_key(key)?;
    if key.is_empty() {
        return Err(VolumeError::InvalidRequest);
    }
    Ok(format!("{prefix}{reserved}{key}"))
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

    pub async fn stat(
        &self,
        key: &str,
        version_id: Option<&str>,
    ) -> Result<ObjectMetadata, VolumeError> {
        relative_key(key)?;
        let object_key = format!("{}{key}", self.config.prefix);
        let slash_key = object_key.ends_with('/');
        let result = self
            .client
            .head_object()
            .bucket(self.config.bucket.as_ref())
            .key(object_key)
            .set_version_id(version_id.map(str::to_owned))
            .send()
            .with_subscriber(NoSubscriber::default())
            .await
            .map_err(|error| failure(error, None))?;
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
    pub async fn list(
        &self,
        prefix: &str,
        continuation: Option<&str>,
    ) -> Result<ObjectPage, VolumeError> {
        relative_key(prefix)?;
        let result = self
            .client
            .list_objects_v2()
            .bucket(self.config.bucket.as_ref())
            .prefix(format!("{}{prefix}", self.config.prefix))
            .set_continuation_token(continuation.map(str::to_owned))
            .send()
            .with_subscriber(NoSubscriber::default())
            .await
            .map_err(|error| failure(error, None))?;
        if result.is_truncated() == Some(true) && result.next_continuation_token().is_none() {
            return Err(VolumeError::InvalidResponse);
        }
        let objects = result
            .contents()
            .iter()
            .try_fold(
                Vec::with_capacity(result.contents().len()),
                |mut objects, object| {
                    let object_key = object.key().ok_or(VolumeError::InvalidResponse)?;
                    let key = object_key
                        .strip_prefix(self.config.prefix.as_ref())
                        .ok_or(VolumeError::InvalidResponse)?;
                    relative_key(key).map_err(|_| VolumeError::InvalidResponse)?;
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

    pub async fn read(
        &self,
        key: &str,
        version_id: Option<&str>,
        etag: Option<&str>,
        range: Option<RangeInclusive<u64>>,
    ) -> Result<ObjectRead, VolumeError> {
        relative_key(key)?;
        if range.as_ref().is_some_and(RangeInclusive::is_empty) {
            return Err(VolumeError::InvalidRequest);
        }
        let object_key = format!("{}{key}", self.config.prefix);
        let slash_key = object_key.ends_with('/');
        let result = self
            .client
            .get_object()
            .bucket(self.config.bucket.as_ref())
            .key(object_key)
            .set_version_id(version_id.map(str::to_owned))
            .set_if_match(etag.map(str::to_owned))
            .set_range(
                range
                    .as_ref()
                    .map(|r| format!("bytes={}-{}", r.start(), r.end())),
            )
            .send()
            .with_subscriber(NoSubscriber::default())
            .await
            .map_err(|error| failure(error, None))?;
        let length = result
            .content_length()
            .and_then(|n| u64::try_from(n).ok())
            .ok_or(VolumeError::InvalidResponse)?;
        if length == 0 && slash_key {
            return Err(VolumeError::NotFound);
        }
        let version = version(result.e_tag(), result.version_id())?;
        check_reference(version_id, etag, &version)?;
        check_range(range.as_ref(), result.content_range(), length)?;
        Ok(ObjectRead {
            version,
            length,
            body: ObjectBody(result.body),
        })
    }

    pub async fn put(
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
        version(result.e_tag(), result.version_id())
    }

    /// The reference size and ETag describe the same observed object. Folder markers are not files.
    pub async fn delete(
        &self,
        target: WriteTarget<'_>,
        etag: &str,
        reference_size: u64,
    ) -> Result<ObjectVersion, VolumeError> {
        let key = write_key(&self.config.prefix, target)?;
        check_not_marker(&key, reference_size == 0)?;
        let (if_match, _) = condition_headers(WriteCondition::Matches(etag))?;
        let result = self
            .client
            .delete_object()
            .bucket(self.config.bucket.as_ref())
            .key(key)
            .set_if_match(if_match)
            .send()
            .with_subscriber(NoSubscriber::default())
            .await
            .map_err(|error| failure(error, Some(WriteCondition::Matches(etag))))?;
        version(Some(etag), result.version_id())
    }

    pub async fn start_multipart(
        &self,
        target: WriteTarget<'_>,
    ) -> Result<ResponseValue<Box<str>>, VolumeError> {
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
            .map(|id| ResponseValue(id.into()))
            .ok_or(VolumeError::InvalidResponse)
    }

    pub async fn upload_part(
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

    pub async fn complete_multipart(
        &self,
        target: WriteTarget<'_>,
        upload_id: &str,
        parts: &[UploadedPart],
        condition: WriteCondition<'_>,
    ) -> Result<ObjectVersion, VolumeError> {
        if parts.is_empty()
            || parts.iter().any(|part| !(1..=10000).contains(&part.number))
            || parts
                .windows(2)
                .any(|pair| pair[0].number >= pair[1].number)
        {
            return Err(VolumeError::InvalidRequest);
        }
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
        version(result.e_tag(), result.version_id())
    }

    pub async fn abort_multipart(
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

    pub async fn versioning(
        &self,
    ) -> Result<Option<ResponseValue<BucketVersioningStatus>>, VolumeError> {
        let result = self
            .client
            .get_bucket_versioning()
            .bucket(self.config.bucket.as_ref())
            .send()
            .with_subscriber(NoSubscriber::default())
            .await
            .map_err(|error| failure(error, None))?;
        Ok(result.status.map(ResponseValue))
    }

    pub async fn lifecycle(&self) -> Result<Box<[ResponseValue<LifecycleRule>]>, VolumeError> {
        match self
            .client
            .get_bucket_lifecycle_configuration()
            .bucket(self.config.bucket.as_ref())
            .send()
            .with_subscriber(NoSubscriber::default())
            .await
        {
            Ok(result) => Ok(result
                .rules
                .unwrap_or_default()
                .into_iter()
                .map(ResponseValue)
                .collect()),
            Err(error)
                if error
                    .as_service_error()
                    .and_then(ProvideErrorMetadata::code)
                    == Some("NoSuchLifecycleConfiguration") =>
            {
                Ok(Box::new([]))
            }
            Err(error) => Err(failure(error, None)),
        }
    }
}

#[cfg(test)]
mod tests;
