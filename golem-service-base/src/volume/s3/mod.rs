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

use super::connection::{ConnectionPolicy, ConnectionRefusal};
use super::keys::{full_key, strong_etag};
use super::placement::{
    BucketVersioning, LifecycleRule, LifecycleRuleStatus, NoncurrentExpiration, Placement,
    PlacementObservations, VersionRead, verdict,
};
use super::transport::{SystemResolver, Transport};
use super::{
    Cursor, Entry, FileReader, FileState, Node, Page, ReadStream, VolumeClient, VolumeError,
    VolumeFile, VolumePath, VolumeRecovery,
};
use crate::aws::AwsCredentials;
use async_trait::async_trait;
use aws_sdk_s3::config::{BehaviorVersion, Region};
use aws_sdk_s3::error::{ProvideErrorMetadata, SdkError};
use aws_sdk_s3::types::{BucketVersioningStatus, ExpirationStatus};
use aws_smithy_runtime_api::client::http::{SharedHttpConnector, http_client_fn};
use aws_smithy_types::retry::RetryConfig;
use aws_smithy_types::timeout::TimeoutConfig;
use futures::TryStreamExt;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt::{Debug, Formatter};
use std::ops::{Range, RangeInclusive};
use std::sync::Arc;
use std::time::Duration;
use tracing::instrument::WithSubscriber;
use tracing::subscriber::NoSubscriber;

#[derive(Debug)]
pub struct S3VolumeConfig {
    pub endpoint: Option<Box<str>>,
    pub region: Arc<str>,
    pub bucket: Box<str>,
    /// An empty prefix selects the bucket root. A nonempty prefix ends in `/`.
    pub prefix: Box<str>,
}

/// Read-only S3 access with static credentials and checked connections.
/// No operation retries a request or follows a redirect.
/// SDK tracing is disabled because a response can reflect credentials in an error.
pub struct S3Volume {
    client: aws_sdk_s3::Client,
    bucket: Arc<str>,
    prefix: Box<str>,
}

impl Debug for S3Volume {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        // The SDK can retain untrusted endpoint metadata that reflects credentials.
        f.write_str("S3Volume([redacted])")
    }
}

#[derive(Serialize, Deserialize)]
struct ListCursor {
    directory: Box<str>,
    continuation: Box<str>,
    last_child: Option<VolumePath>,
    files_before_directory: Box<[VolumePath]>,
}

/// Builds both handles without sending a request.
#[expect(
    clippy::type_complexity,
    reason = "the two handles expose live reads and recovery separately"
)]
pub fn connect(
    config: S3VolumeConfig,
    credentials: AwsCredentials,
    policy: ConnectionPolicy,
) -> Result<(Arc<dyn VolumeClient>, Arc<dyn VolumeRecovery>), VolumeError> {
    let volume = Arc::new(S3Volume::new(config, credentials, policy)?);
    Ok((volume.clone(), volume))
}

/// Runs the read-only placement checks.
pub async fn place(
    config: S3VolumeConfig,
    credentials: AwsCredentials,
    policy: ConnectionPolicy,
) -> Result<Placement, VolumeError> {
    let volume = S3Volume::new(config, credentials, policy)?;
    let versioning = volume.versioning().await;
    let version_read = if matches!(versioning, Ok(BucketVersioning::Enabled)) {
        volume.version_read().await
    } else {
        Ok(VersionRead::NoObject)
    };
    let lifecycle = volume.lifecycle().await;
    verdict(PlacementObservations {
        versioning,
        version_read,
        lifecycle,
    })
}

fn normalized_version_id(id: Option<&str>) -> Result<Option<&str>, VolumeError> {
    match id {
        Some("") => Err(VolumeError::InvalidResponse),
        Some("null") | None => Ok(None),
        Some(id) => Ok(Some(id)),
    }
}

fn state(etag: Option<&str>, version_id: Option<&str>) -> Result<FileState, VolumeError> {
    Ok(FileState::from_reference(
        etag.filter(|etag| strong_etag(etag))
            .ok_or(VolumeError::InvalidResponse)?
            .into(),
        normalized_version_id(version_id)?.map(Into::into),
    ))
}

fn check_reference(
    requested_version: Option<&str>,
    requested_etag: Option<&str>,
    actual: &FileState,
) -> Result<(), VolumeError> {
    let (etag, version) = actual.reference();
    if requested_version.is_some_and(|id| version != Some(id))
        || requested_etag.is_some_and(|requested| requested != etag)
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
    size: u64,
) -> Result<(), VolumeError> {
    let Some(requested) = requested else {
        return if content_range.is_none() && length == size {
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
        || total != size
        || total == 0
        || end != (*requested.end()).min(total - 1)
        || end.checked_sub(start).and_then(|n| n.checked_add(1)) != Some(length)
    {
        Err(VolumeError::InvalidResponse)
    } else {
        Ok(())
    }
}

fn lifecycle_rule(rule: aws_sdk_s3::types::LifecycleRule) -> Result<LifecycleRule, VolumeError> {
    let status = match rule.status() {
        ExpirationStatus::Enabled => LifecycleRuleStatus::Enabled,
        ExpirationStatus::Disabled => LifecycleRuleStatus::Disabled,
        other => LifecycleRuleStatus::Unknown(other.as_str().into()),
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
        .map(Into::into);
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

fn failure<E: ProvideErrorMetadata + Error + 'static>(
    error: SdkError<E>,
    action: &'static str,
    recorded: bool,
) -> VolumeError {
    if let Some(error) = std::iter::successors(Some(&error as &(dyn Error + 'static)), |error| {
        (*error).source()
    })
    .find_map(|error| {
        if error.is::<ConnectionRefusal>() {
            Some(VolumeError::ConnectionRefused)
        } else {
            error.downcast_ref::<VolumeError>().cloned()
        }
    }) {
        return error;
    }
    if matches!(error, SdkError::ConstructionFailure(_)) {
        return VolumeError::InvalidRequest;
    }
    let status = error
        .raw_response()
        .map(|response| response.status().as_u16());
    if status.is_some_and(|status| (200..300).contains(&status)) {
        return VolumeError::InvalidResponse;
    }
    let code = error
        .as_service_error()
        .and_then(ProvideErrorMetadata::code);
    match (status, code) {
        (Some(500..=599), _) => VolumeError::Unanswered,
        (Some(401 | 403), _)
        | (
            _,
            Some(
                "AccessDenied"
                | "ExpiredToken"
                | "InvalidToken"
                | "TokenRefreshRequired"
                | "SignatureDoesNotMatch"
                | "InvalidAccessKeyId",
            ),
        ) => VolumeError::Denied {
            action: action.into(),
        },
        (Some(404), _) | (_, Some("NoSuchKey" | "NotFound" | "NoSuchVersion" | "NoSuchBucket")) => {
            if recorded {
                VolumeError::Changed
            } else {
                VolumeError::NotFound
            }
        }
        (Some(412), _) | (_, Some("PreconditionFailed")) => VolumeError::Changed,
        (Some(405), _) if recorded => VolumeError::Changed,
        (Some(416), _) => VolumeError::InvalidResponse,
        (Some(400), _) => VolumeError::InvalidRequest,
        (Some(_), _) => VolumeError::InvalidResponse,
        _ if matches!(error, SdkError::ResponseError(_)) => VolumeError::InvalidResponse,
        _ => VolumeError::Unanswered,
    }
}

impl S3Volume {
    pub fn new(
        config: S3VolumeConfig,
        credentials: AwsCredentials,
        policy: ConnectionPolicy,
    ) -> Result<Self, VolumeError> {
        if config.bucket.is_empty()
            || config.region.is_empty()
            || config.prefix.len() > 1024
            || (!config.prefix.is_empty()
                && config
                    .prefix
                    .strip_suffix('/')
                    .is_none_or(|prefix| prefix.is_empty() || VolumePath::new(prefix).is_err()))
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
            .credentials_provider(credentials.0)
            .force_path_style(true)
            .http_client(http_client_fn(move |_, _| transport.clone()))
            .retry_config(RetryConfig::disabled())
            .timeout_config(
                TimeoutConfig::builder()
                    .operation_timeout(Duration::from_secs(60))
                    .build(),
            );
        if let Some(endpoint) = &config.endpoint {
            sdk = sdk.endpoint_url(endpoint.as_ref());
        }
        Ok(Self {
            client: aws_sdk_s3::Client::from_conf(sdk.build()),
            bucket: config.bucket.into(),
            prefix: config.prefix,
        })
    }

    async fn versioning(&self) -> Result<BucketVersioning, VolumeError> {
        let result = self
            .client
            .get_bucket_versioning()
            .bucket(self.bucket.as_ref())
            .send()
            .with_subscriber(NoSubscriber::default())
            .await
            .map_err(|error| failure(error, "GetBucketVersioning", false))?;
        Ok(match result.status() {
            None => BucketVersioning::Unversioned,
            Some(BucketVersioningStatus::Enabled) => BucketVersioning::Enabled,
            Some(BucketVersioningStatus::Suspended) => BucketVersioning::Suspended,
            Some(other) => BucketVersioning::Unknown(other.as_str().into()),
        })
    }

    async fn version_read(&self) -> Result<VersionRead, VolumeError> {
        // ListBucketVersions is not part of the volume's required credential permissions.
        let pages = futures::stream::try_unfold(Some(None), |continuation| async move {
            let Some(continuation) = continuation else {
                return Ok(None);
            };
            let result = self
                .client
                .list_objects_v2()
                .bucket(self.bucket.as_ref())
                .prefix(self.prefix.as_ref())
                .set_continuation_token(continuation.clone())
                .send()
                .with_subscriber(NoSubscriber::default())
                .await
                .map_err(|error| failure(error, "ListBucket", false))?;
            if result.encoding_type().is_some()
                || !result.common_prefixes().is_empty()
                || result
                    .prefix()
                    .is_some_and(|prefix| prefix != self.prefix.as_ref())
            {
                return Err(VolumeError::InvalidResponse);
            }
            let next = match (result.is_truncated(), result.next_continuation_token()) {
                (Some(true), Some(next))
                    if !next.is_empty() && Some(next) != continuation.as_deref() =>
                {
                    Some(Some(next.to_owned()))
                }
                (Some(false), None) => None,
                _ => return Err(VolumeError::InvalidResponse),
            };
            let keys = result
                .contents()
                .iter()
                .map(|object| {
                    let key = object
                        .key()
                        .filter(|key| key.starts_with(self.prefix.as_ref()))
                        .ok_or(VolumeError::InvalidResponse)?;
                    full_key(0, key).map_err(|_| VolumeError::InvalidResponse)?;
                    Ok::<_, VolumeError>(Box::<str>::from(key))
                })
                .collect::<Result<Box<[_]>, _>>()?;
            Ok::<_, VolumeError>(Some((keys, next)))
        })
        .map_ok(|keys| futures::stream::iter(keys.into_vec().into_iter().map(Ok)))
        .try_flatten()
        .try_filter_map(|key| async move {
            let (size, reference) = match self.head(&key, None).await {
                Ok(found) => found,
                Err(VolumeError::NotFound) => return Ok(None),
                Err(error) => return Err(error),
            };
            Ok(reference
                .reference()
                .1
                .is_some()
                .then_some((key, reference, size)))
        });
        futures::pin_mut!(pages);
        let Some((key, reference, size)) = pages.try_next().await? else {
            return Ok(VersionRead::NoObject);
        };
        let reader = S3FileReader {
            client: self.client.clone(),
            bucket: self.bucket.clone(),
            key,
            size,
        };
        reader
            .get(&reference, (size > 0).then_some(0..=0))
            .await?
            .try_for_each(|_| futures::future::ready(Ok(())))
            .await?;
        Ok(VersionRead::Verified)
    }

    async fn lifecycle(&self) -> Result<Box<[LifecycleRule]>, VolumeError> {
        let result = self
            .client
            .get_bucket_lifecycle_configuration()
            .bucket(self.bucket.as_ref())
            .send()
            .with_subscriber(NoSubscriber::default())
            .await;
        let result = match result {
            Err(error)
                if error
                    .raw_response()
                    .is_some_and(|response| response.status().as_u16() == 404)
                    && error
                        .as_service_error()
                        .and_then(ProvideErrorMetadata::code)
                        == Some("NoSuchLifecycleConfiguration") =>
            {
                return Ok(Box::new([]));
            }
            result => result.map_err(|error| failure(error, "GetLifecycleConfiguration", false))?,
        };
        result
            .rules
            .unwrap_or_default()
            .into_iter()
            .map(lifecycle_rule)
            .collect()
    }

    fn object_key(&self, path: &VolumePath) -> Result<Box<str>, VolumeError> {
        if path.as_str().is_empty() {
            return Err(VolumeError::NotFound);
        }
        full_key(self.prefix.len(), path.as_str())?;
        Ok(format!("{}{}", self.prefix, path.as_str()).into_boxed_str())
    }

    async fn open_at(
        &self,
        path: &VolumePath,
        recorded: Option<&FileState>,
    ) -> Result<VolumeFile, VolumeError> {
        let key = self.object_key(path).map_err(|error| {
            if recorded.is_some() && error == VolumeError::NotFound {
                VolumeError::Changed
            } else {
                error
            }
        })?;
        let (size, state) = self.head(&key, recorded).await?;
        let replayable = state.reference().1.is_some();
        Ok(VolumeFile::new(
            size,
            state,
            replayable,
            Arc::new(S3FileReader {
                client: self.client.clone(),
                bucket: self.bucket.clone(),
                key,
                size,
            }),
        ))
    }

    async fn head(
        &self,
        key: &str,
        recorded: Option<&FileState>,
    ) -> Result<(u64, FileState), VolumeError> {
        let (etag, version) = recorded.map_or((None, None), |state| {
            let (etag, version) = state.reference();
            (Some(etag), version)
        });
        if etag.is_some_and(|etag| !strong_etag(etag)) || version == Some("") {
            return Err(VolumeError::InvalidRequest);
        }
        let version = version.filter(|version| *version != "null");
        let result = self
            .client
            .head_object()
            .bucket(self.bucket.as_ref())
            .key(key)
            .set_version_id(version.map(str::to_owned))
            .set_if_match(etag.map(str::to_owned))
            .send()
            .with_subscriber(NoSubscriber::default())
            .await
            .map_err(|error| {
                failure(
                    error,
                    if version.is_some() {
                        "GetObjectVersion"
                    } else {
                        "GetObject"
                    },
                    recorded.is_some(),
                )
            })?;
        let size = result
            .content_length()
            .and_then(|n| u64::try_from(n).ok())
            .ok_or(VolumeError::InvalidResponse)?;
        check_range(None, result.content_range(), size, size)?;
        let state = state(result.e_tag(), result.version_id())?;
        check_reference(version, etag, &state)?;
        if result.delete_marker() == Some(true) {
            return Err(if recorded.is_some() {
                VolumeError::Changed
            } else {
                VolumeError::NotFound
            });
        }
        // A conditional response can name a new version without selecting it for this file.
        let state = etag.map_or(state, |etag| {
            FileState::from_reference(etag.into(), version.map(Into::into))
        });
        Ok((size, state))
    }
}

#[async_trait]
impl VolumeClient for S3Volume {
    async fn stat(&self, path: &VolumePath) -> Result<Option<Node>, VolumeError> {
        if path.as_str().is_empty() {
            return Ok(Some(Node::Directory));
        }
        match self.open_at(path, None).await {
            Ok(file) => Ok(Some(Node::File { size: file.size() })),
            Err(VolumeError::NotFound) => {
                if self.prefix.len() + path.as_str().len() >= 1024 {
                    return Ok(None);
                }
                let found = futures::stream::try_unfold(Some(None), |after| async move {
                    let Some(after) = after else { return Ok(None) };
                    let page = self.list(path, after.as_ref()).await?;
                    Ok::<_, VolumeError>(Some((!page.entries.is_empty(), page.after.map(Some))))
                })
                .try_any(futures::future::ready)
                .await?;
                Ok(found.then_some(Node::Directory))
            }
            Err(error) => Err(error),
        }
    }

    async fn list(
        &self,
        directory: &VolumePath,
        after: Option<&Cursor>,
    ) -> Result<Page, VolumeError> {
        let prefix = if directory.as_str().is_empty() {
            self.prefix.clone()
        } else {
            let key = self.object_key(directory)?;
            full_key(1, &key)?;
            format!("{key}/").into_boxed_str()
        };
        let cursor = after
            .map(|cursor| {
                serde_json::from_str::<ListCursor>(cursor.as_str())
                    .map_err(|_| VolumeError::InvalidRequest)
            })
            .transpose()?;
        if cursor
            .as_ref()
            .is_some_and(|cursor| cursor.directory != prefix || cursor.continuation.is_empty())
        {
            return Err(VolumeError::InvalidRequest);
        }
        let continuation = cursor.as_ref().map(|cursor| cursor.continuation.as_ref());
        let result = self
            .client
            .list_objects_v2()
            .bucket(self.bucket.as_ref())
            .prefix(prefix.as_ref())
            .set_continuation_token(continuation.map(str::to_owned))
            .send()
            .with_subscriber(NoSubscriber::default())
            .await
            .map_err(|error| failure(error, "ListBucket", false))?;
        if result.encoding_type().is_some()
            || !result.common_prefixes().is_empty()
            || result
                .prefix()
                .is_some_and(|returned| returned != prefix.as_ref())
        {
            return Err(VolumeError::InvalidResponse);
        }
        let next = match (result.is_truncated(), result.next_continuation_token()) {
            (Some(true), Some(next)) if !next.is_empty() && Some(next) != continuation => {
                Some(next)
            }
            (Some(false), None) => None,
            _ => return Err(VolumeError::InvalidResponse),
        };
        let last_child = cursor.as_ref().and_then(|cursor| cursor.last_child.clone());
        let files = cursor.as_ref().map_or_else(BTreeSet::new, |cursor| {
            cursor.files_before_directory.iter().cloned().collect()
        });
        let (entries, last_child, files) = result.contents().iter().try_fold(
            (BTreeMap::new(), last_child, files),
            |(mut entries, last_child, mut files), object| {
                let key = object.key().ok_or(VolumeError::InvalidResponse)?;
                let relative = key
                    .strip_prefix(prefix.as_ref())
                    .ok_or(VolumeError::InvalidResponse)?;
                let size = object
                    .size()
                    .and_then(|size| u64::try_from(size).ok())
                    .ok_or(VolumeError::InvalidResponse)?;
                if (size == 0 && key.ends_with('/'))
                    || full_key(0, key).is_err()
                    || relative.is_empty()
                    || VolumePath::new(relative).is_err()
                {
                    return Ok((entries, last_child, files));
                }
                let (child, node) = relative
                    .split_once('/')
                    .map_or((relative, Node::File { size }), |(child, _)| {
                        (child, Node::Directory)
                    });
                let child = VolumePath::new(if directory.as_str().is_empty() {
                    child.to_owned()
                } else {
                    format!("{}/{child}", directory.as_str())
                })?;
                let shadowed = files.contains(&child);
                let volume_relative = key
                    .strip_prefix(self.prefix.as_ref())
                    .ok_or(VolumeError::InvalidResponse)?;
                // Names such as `a-` sort between file `a` and its `a/` descendants.
                // Keep file precedence until that descendant range starts. The rolling
                // last child then suppresses the rest of its contiguous range.
                files.retain(|file| {
                    volume_relative
                        .bytes()
                        .cmp(file.as_str().bytes().chain(std::iter::once(b'/')))
                        .is_lt()
                });
                if matches!(node, Node::File { .. }) {
                    files.insert(child.clone());
                }
                if last_child.as_ref() != Some(&child) && !shadowed {
                    entries.entry(child.clone()).or_insert(node);
                }
                Ok::<_, VolumeError>((entries, Some(child), files))
            },
        )?;
        let after = next
            .map(|next| {
                serde_json::to_string(&ListCursor {
                    directory: prefix,
                    continuation: next.into(),
                    last_child,
                    files_before_directory: files.into_iter().collect(),
                })
                .map(Cursor::new)
                .map_err(|_| VolumeError::InvalidResponse)
            })
            .transpose()?;
        Ok(Page {
            entries: entries
                .into_iter()
                .map(|(path, node)| Entry { path, node })
                .collect(),
            after,
        })
    }

    async fn open(&self, path: &VolumePath) -> Result<VolumeFile, VolumeError> {
        self.open_at(path, None).await
    }
}

#[async_trait]
impl VolumeRecovery for S3Volume {
    async fn open_recorded(
        &self,
        path: &VolumePath,
        recorded: &FileState,
    ) -> Result<VolumeFile, VolumeError> {
        self.open_at(path, Some(recorded)).await
    }
}

struct S3FileReader {
    client: aws_sdk_s3::Client,
    bucket: Arc<str>,
    key: Box<str>,
    size: u64,
}

impl Debug for S3FileReader {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        // Untrusted endpoint metadata can reflect credentials in an object key.
        f.write_str("S3FileReader([redacted])")
    }
}

impl S3FileReader {
    async fn get(
        &self,
        reference: &FileState,
        range: Option<RangeInclusive<u64>>,
    ) -> Result<ReadStream, VolumeError> {
        let (etag, version) = reference.reference();
        let result = self
            .client
            .get_object()
            .bucket(self.bucket.as_ref())
            .key(self.key.as_ref())
            .set_version_id(version.map(str::to_owned))
            .if_match(etag)
            .set_range(
                range
                    .as_ref()
                    .map(|range| format!("bytes={}-{}", range.start(), range.end())),
            )
            .send()
            .with_subscriber(NoSubscriber::default())
            .await
            .map_err(|error| {
                failure(
                    error,
                    if version.is_some() {
                        "GetObjectVersion"
                    } else {
                        "GetObject"
                    },
                    true,
                )
            })?;
        let length = result
            .content_length()
            .and_then(|n| u64::try_from(n).ok())
            .ok_or(VolumeError::InvalidResponse)?;
        let actual = state(result.e_tag(), result.version_id())?;
        check_reference(version, Some(etag), &actual)?;
        check_range(range.as_ref(), result.content_range(), length, self.size)?;
        Ok(Box::pin(futures::stream::try_unfold(
            (result.body, length),
            |(mut body, remaining)| async move {
                let chunk = body
                    .try_next()
                    .with_subscriber(NoSubscriber::default())
                    .await
                    .map_err(|_| VolumeError::Unanswered)?;
                match chunk {
                    Some(chunk) => {
                        let remaining = remaining
                            .checked_sub(chunk.len() as u64)
                            .ok_or(VolumeError::InvalidResponse)?;
                        Ok(Some((chunk, (body, remaining))))
                    }
                    None if remaining == 0 => Ok(None),
                    None => Err(VolumeError::Unanswered),
                }
            },
        )))
    }
}

#[async_trait]
impl FileReader for S3FileReader {
    async fn read(
        &self,
        reference: &FileState,
        range: Range<u64>,
    ) -> Result<ReadStream, VolumeError> {
        self.get(reference, Some(range.start..=range.end - 1)).await
    }
}

#[cfg(test)]
mod tests;
