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

pub mod connection;
mod keys;
pub mod memory;
pub mod s3;
mod signing;
mod transport;

use async_trait::async_trait;
use bytes::Bytes;
use futures::{Stream, StreamExt};
use std::fmt::{Debug, Formatter};
use std::ops::RangeInclusive;
use std::pin::Pin;
use std::sync::Arc;

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
    pub etag: Arc<str>,
    pub version_id: Option<Arc<str>>,
}

impl Debug for ObjectVersion {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("ObjectVersion")
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct ObjectMetadata {
    pub key: Arc<str>,
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

/// Object bytes with opaque errors.
pub struct ObjectBody(Pin<Box<dyn Stream<Item = Result<Bytes, VolumeError>> + Send>>);

impl Debug for ObjectBody {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("ObjectBody")
    }
}

impl ObjectBody {
    pub fn from_bytes(bytes: Bytes) -> Self {
        Self::from_stream(futures::stream::iter(
            (!bytes.is_empty()).then_some(Ok(bytes)),
        ))
    }

    pub fn from_stream(
        stream: impl Stream<Item = Result<Bytes, VolumeError>> + Send + 'static,
    ) -> Self {
        Self(Box::pin(stream))
    }

    pub async fn next(&mut self) -> Result<Option<Bytes>, VolumeError> {
        self.0.next().await.transpose()
    }

    pub async fn collect(self) -> Result<Bytes, VolumeError> {
        use futures::TryStreamExt;
        self.0
            .try_fold(bytes::BytesMut::new(), |mut all, chunk| async move {
                all.extend_from_slice(&chunk);
                Ok(all)
            })
            .await
            .map(bytes::BytesMut::freeze)
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct ListedObject {
    pub key: Arc<str>,
    pub size: u64,
    pub etag: Arc<str>,
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

#[derive(Clone, Copy)]
pub enum WriteCondition<'a> {
    Absent,
    /// One strong, quoted ETag. Wildcards and lists are not reference versions.
    Matches(&'a str),
}

impl Debug for WriteCondition<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Absent => "Absent",
            Self::Matches(_) => "Matches",
        })
    }
}

/// Probe names are relative to `.golem/`. File writes cannot use that reserved prefix.
#[derive(Clone, Copy)]
pub enum WriteTarget<'a> {
    File(&'a str),
    Probe(&'a str),
}

impl Debug for WriteTarget<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::File(_) => "File",
            Self::Probe(_) => "Probe",
        })
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct UploadedPart {
    pub number: i32,
    pub etag: Arc<str>,
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
    pub fn new(value: T) -> Self {
        Self(value)
    }

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BucketVersioning {
    Unversioned,
    Enabled,
    Suspended,
    Unknown(ResponseValue<Box<str>>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LifecycleRuleStatus {
    Enabled,
    Disabled,
    Unknown(ResponseValue<Box<str>>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoncurrentExpiration {
    pub days: i32,
    pub newer_versions: Option<i32>,
}

/// A conservative view of rules that can expire noncurrent versions.
/// Tag and object-size filters do not exclude possible applicability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifecycleRule {
    pub status: LifecycleRuleStatus,
    pub prefix: Option<ResponseValue<Box<str>>>,
    pub noncurrent_expiration: Option<NoncurrentExpiration>,
}

/// Conditional object operations. A read uses the complete reference, with a null version
/// represented by None. A range starting at or beyond the object size returns no bytes.
/// Multipart operations reject keys that end in `/`.
#[async_trait]
pub trait VolumeClient: Debug + Send + Sync {
    async fn stat(
        &self,
        key: &str,
        version_id: Option<&str>,
    ) -> Result<ObjectMetadata, VolumeError>;
    async fn read(
        &self,
        key: &str,
        reference: &ObjectVersion,
        range: Option<RangeInclusive<u64>>,
    ) -> Result<ObjectRead, VolumeError>;
    async fn list(
        &self,
        prefix: &str,
        continuation: Option<&str>,
    ) -> Result<ObjectPage, VolumeError>;
    async fn put(
        &self,
        target: WriteTarget<'_>,
        condition: WriteCondition<'_>,
        body: Bytes,
    ) -> Result<ObjectVersion, VolumeError>;
    async fn delete(
        &self,
        target: WriteTarget<'_>,
        reference: &ObjectMetadata,
    ) -> Result<Option<ResponseValue<Arc<str>>>, VolumeError>;
    async fn start_multipart(
        &self,
        target: WriteTarget<'_>,
    ) -> Result<ResponseValue<Arc<str>>, VolumeError>;
    async fn upload_part(
        &self,
        target: WriteTarget<'_>,
        upload_id: &str,
        number: i32,
        body: Bytes,
    ) -> Result<UploadedPart, VolumeError>;
    async fn complete_multipart(
        &self,
        target: WriteTarget<'_>,
        upload_id: &str,
        parts: &[UploadedPart],
        condition: WriteCondition<'_>,
    ) -> Result<ObjectVersion, VolumeError>;
    async fn abort_multipart(
        &self,
        target: WriteTarget<'_>,
        upload_id: &str,
    ) -> Result<(), VolumeError>;
    async fn versioning(&self) -> Result<BucketVersioning, VolumeError>;
    async fn lifecycle(&self) -> Result<Box<[LifecycleRule]>, VolumeError>;
}
