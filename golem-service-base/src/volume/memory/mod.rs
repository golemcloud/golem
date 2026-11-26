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

use super::keys::{
    check_not_marker, full_key, multipart_key, relative_key, strong_etag, validate_delete,
    validate_parts, validate_read, write_key,
};
use super::{
    BucketVersioning, LifecycleRule, ListedObject, ObjectBody, ObjectMetadata, ObjectPage,
    ObjectRead, ObjectVersion, ResponseValue, UploadedPart, VolumeClient, VolumeError,
    WriteCondition, WriteTarget,
};
use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use bytes::{Bytes, BytesMut};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fmt::{Debug, Formatter};
use std::ops::RangeInclusive;
use std::sync::{Arc, Mutex, Weak};
use uuid::Uuid;

/// An isolated volume with conditional writes and optional retained versions.
pub struct InMemoryVolume {
    versioned: bool,
    page_size: usize,
    prefix_len: usize,
    state: Mutex<State>,
}

impl Debug for InMemoryVolume {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("InMemoryVolume")
    }
}

#[derive(Default)]
struct State {
    objects: BTreeMap<Arc<str>, ObjectPhase>,
    uploads: BTreeMap<Arc<str>, Upload>,
}

enum ObjectPhase {
    Missing,
    Present {
        object: Arc<StoredObject>,
        versions: BTreeMap<Arc<str>, HistoricalVersion>,
    },
    Deleted {
        versions: BTreeMap<Arc<str>, HistoricalVersion>,
    },
}

enum ObjectEvent {
    Publish(Arc<StoredObject>),
    Delete { marker_id: Arc<str> },
}

enum HistoricalVersion {
    Object(Arc<StoredObject>),
    DeleteMarker,
}

struct StoredObject {
    version: ObjectVersion,
    body: Bytes,
}

struct Upload {
    key: Arc<str>,
    phase: UploadPhase,
}

enum UploadPhase {
    Open(BTreeMap<i32, Arc<StoredPart>>),
    Completed(Box<CompletedUpload>),
    Aborted,
}

struct CompletedUpload {
    object: Weak<StoredObject>,
    parts: Box<[(i32, Arc<str>)]>,
}

struct StoredPart {
    number: i32,
    etag: Arc<str>,
    body: Bytes,
}

enum UploadEvent {
    Part(Arc<StoredPart>),
    Complete(Box<CompletedUpload>),
    Abort,
}

impl InMemoryVolume {
    pub fn new(versioned: bool, page_size: usize, prefix: &str) -> Result<Self, VolumeError> {
        relative_key(prefix)?;
        if page_size == 0 || (!prefix.is_empty() && !prefix.ends_with('/')) {
            return Err(VolumeError::InvalidRequest);
        }
        Ok(Self {
            versioned,
            page_size,
            prefix_len: prefix.len(),
            state: Mutex::new(State::default()),
        })
    }
}

fn validate_condition(condition: WriteCondition<'_>) -> Result<(), VolumeError> {
    match condition {
        WriteCondition::Matches(etag) if !strong_etag(etag) => Err(VolumeError::InvalidRequest),
        _ => Ok(()),
    }
}

fn check_condition(
    current: Option<&StoredObject>,
    condition: WriteCondition<'_>,
) -> Result<(), VolumeError> {
    match (condition, current) {
        (WriteCondition::Absent, Some(_)) => Err(VolumeError::Exists),
        (WriteCondition::Matches(etag), Some(current)) if current.version.etag.as_ref() == etag => {
            Ok(())
        }
        (WriteCondition::Matches(_), _) => Err(VolumeError::Conflict),
        (WriteCondition::Absent, None) => Ok(()),
    }
}

fn normalized_id(id: Option<&str>) -> Option<&str> {
    id.filter(|id| *id != "null")
}

fn current_object(phase: &ObjectPhase) -> Option<&StoredObject> {
    match phase {
        ObjectPhase::Present { object, .. } => Some(object.as_ref()),
        ObjectPhase::Missing | ObjectPhase::Deleted { .. } => None,
    }
}

fn select<'a>(
    state: &'a State,
    key: &str,
    version_id: Option<&str>,
) -> Result<(&'a Arc<str>, &'a StoredObject), VolumeError> {
    let (key, phase) = state
        .objects
        .get_key_value(key)
        .ok_or(VolumeError::NotFound)?;
    match version_id {
        None => current_object(phase),
        Some("null") => current_object(phase).filter(|object| object.version.version_id.is_none()),
        Some(id) => {
            let historical = match phase {
                ObjectPhase::Present { versions, .. } | ObjectPhase::Deleted { versions } => {
                    versions.get(id)
                }
                ObjectPhase::Missing => None,
            };
            match historical {
                Some(HistoricalVersion::Object(object)) => Some(object.as_ref()),
                Some(HistoricalVersion::DeleteMarker) => return Err(VolumeError::Unsupported),
                None => None,
            }
        }
    }
    .map(|object| (key, object))
    .ok_or(VolumeError::NotFound)
}

fn check_reference(reference: &ObjectVersion, actual: &ObjectVersion) -> Result<(), VolumeError> {
    if reference.etag.as_ref() != actual.etag.as_ref() {
        Err(VolumeError::Conflict)
    } else if normalized_id(reference.version_id.as_deref()) != actual.version_id.as_deref() {
        Err(VolumeError::InvalidResponse)
    } else {
        Ok(())
    }
}

fn check_visible(key: &str, object: &StoredObject) -> Result<(), VolumeError> {
    check_not_marker(key, object.body.is_empty()).map_err(|_| VolumeError::NotFound)
}

fn selected_body(body: &Bytes, range: Option<RangeInclusive<u64>>) -> Bytes {
    match range {
        Some(range) if *range.start() >= body.len() as u64 => Bytes::new(),
        Some(range) => {
            let end = (*range.end()).min(body.len() as u64 - 1) as usize;
            body.slice(*range.start() as usize..=end)
        }
        None => body.clone(),
    }
}

fn new_version(versioned: bool, body: &[u8], version_id: Uuid) -> ObjectVersion {
    ObjectVersion {
        etag: format!("\"{:x}\"", Sha256::digest(body)).into(),
        version_id: versioned.then(|| version_id.to_string().into()),
    }
}

fn object_transition(
    phase: ObjectPhase,
    event: ObjectEvent,
    versioned: bool,
) -> (ObjectPhase, Result<Option<Arc<str>>, VolumeError>) {
    match event {
        ObjectEvent::Publish(object) => {
            let mut versions = match phase {
                ObjectPhase::Present { versions, .. } | ObjectPhase::Deleted { versions } => {
                    versions
                }
                ObjectPhase::Missing => BTreeMap::new(),
            };
            if let Some(id) = &object.version.version_id {
                versions.insert(id.clone(), HistoricalVersion::Object(object.clone()));
            }
            (ObjectPhase::Present { object, versions }, Ok(None))
        }
        ObjectEvent::Delete { marker_id } => match phase {
            ObjectPhase::Present { mut versions, .. } if versioned => {
                versions.insert(marker_id.clone(), HistoricalVersion::DeleteMarker);
                (ObjectPhase::Deleted { versions }, Ok(Some(marker_id)))
            }
            ObjectPhase::Present { .. } => (ObjectPhase::Missing, Ok(None)),
            phase => (phase, Err(VolumeError::NotFound)),
        },
    }
}

fn apply_object(
    state: &mut State,
    key: Arc<str>,
    event: ObjectEvent,
    versioned: bool,
) -> Result<Option<Arc<str>>, VolumeError> {
    let (key, phase) = state
        .objects
        .remove_entry(key.as_ref())
        .unwrap_or((key, ObjectPhase::Missing));
    let (next, result) = object_transition(phase, event, versioned);
    if !matches!(next, ObjectPhase::Missing) {
        state.objects.insert(key, next);
    }
    result
}

fn versioning_status(versioned: bool) -> BucketVersioning {
    if versioned {
        BucketVersioning::Enabled
    } else {
        BucketVersioning::Unversioned
    }
}

fn continuation_key(token: &str) -> Result<Box<str>, VolumeError> {
    token
        .strip_prefix("memory:")
        .and_then(|token| URL_SAFE_NO_PAD.decode(token).ok())
        .and_then(|key| String::from_utf8(key).ok())
        .map(String::into_boxed_str)
        .ok_or(VolumeError::InvalidRequest)
}

fn object_page(
    objects: &BTreeMap<Arc<str>, ObjectPhase>,
    prefix: &str,
    after: Option<&str>,
    page_size: usize,
) -> ObjectPage {
    let page = objects
        .iter()
        .filter_map(|(key, phase)| current_object(phase).map(|object| (key, object)))
        .filter(|(key, _)| {
            key.starts_with(prefix) && after.is_none_or(|after| key.as_ref() > after)
        })
        .take(page_size.saturating_add(1))
        .collect::<Vec<_>>();
    let continuation = (page.len() > page_size).then(|| {
        format!(
            "memory:{}",
            URL_SAFE_NO_PAD.encode(page[page_size - 1].0.as_bytes())
        )
        .into_boxed_str()
    });
    let objects = page
        .into_iter()
        .take(page_size)
        .filter(|(key, object)| relative_key(key).is_ok() && check_visible(key, object).is_ok())
        .map(|(key, object)| ListedObject {
            key: key.clone(),
            size: object.body.len() as u64,
            etag: object.version.etag.clone(),
        })
        .collect();
    ObjectPage {
        objects,
        continuation,
    }
}

fn upload<'a>(state: &'a mut State, key: &str, id: &str) -> Result<&'a mut Upload, VolumeError> {
    state
        .uploads
        .get_mut(id)
        .filter(|upload| upload.key.as_ref() == key)
        .ok_or(VolumeError::NotFound)
}

fn upload_transition(upload: &Upload, event: UploadEvent) -> Result<Upload, VolumeError> {
    let UploadPhase::Open(parts) = &upload.phase else {
        return Err(VolumeError::NotFound);
    };
    let phase = match event {
        UploadEvent::Part(part) => {
            let mut parts = parts.clone();
            parts.insert(part.number, part);
            UploadPhase::Open(parts)
        }
        UploadEvent::Complete(completed) => UploadPhase::Completed(completed),
        UploadEvent::Abort => UploadPhase::Aborted,
    };
    Ok(Upload {
        key: upload.key.clone(),
        phase,
    })
}

fn multipart_body(upload: &Upload, parts: &[UploadedPart]) -> Result<Bytes, VolumeError> {
    let UploadPhase::Open(stored) = &upload.phase else {
        return Err(VolumeError::NotFound);
    };
    parts
        .iter()
        .enumerate()
        .try_fold(BytesMut::new(), |mut body, (index, part)| {
            let actual = stored
                .get(&part.number)
                .filter(|actual| actual.etag.as_ref() == part.etag.as_ref())
                .ok_or(VolumeError::RequestFailed)?;
            if index + 1 != parts.len() && actual.body.len() < 5 * 1024 * 1024 {
                return Err(VolumeError::RequestFailed);
            }
            body.extend_from_slice(&actual.body);
            Ok(body)
        })
        .map(BytesMut::freeze)
}

fn retained_object(upload: &Upload) -> Option<Arc<StoredObject>> {
    match &upload.phase {
        UploadPhase::Completed(completed) => completed.object.upgrade(),
        _ => None,
    }
}

fn complete_upload(
    upload: &Upload,
    retained: Option<Arc<StoredObject>>,
    parts: &[UploadedPart],
    versioned: bool,
    version_id: Uuid,
) -> Result<(Option<Upload>, Arc<StoredObject>), VolumeError> {
    match &upload.phase {
        UploadPhase::Open(stored) => {
            let body = multipart_body(upload, parts)?;
            let version = new_version(versioned, &body, version_id);
            let selected = parts
                .iter()
                .map(|part| {
                    stored
                        .get(&part.number)
                        .map(|actual| (actual.number, actual.etag.clone()))
                        .ok_or(VolumeError::RequestFailed)
                })
                .collect::<Result<_, _>>()?;
            let object = Arc::new(StoredObject { version, body });
            let next = upload_transition(
                upload,
                UploadEvent::Complete(Box::new(CompletedUpload {
                    object: Arc::downgrade(&object),
                    parts: selected,
                })),
            )?;
            Ok((Some(next), object))
        }
        UploadPhase::Completed(completed) => {
            let object = retained.ok_or(VolumeError::NotFound)?;
            if completed.parts.len() != parts.len()
                || completed
                    .parts
                    .iter()
                    .zip(parts)
                    .any(|((number, etag), part)| {
                        *number != part.number || etag.as_ref() != part.etag.as_ref()
                    })
            {
                return Err(VolumeError::RequestFailed);
            }
            Ok((None, object))
        }
        UploadPhase::Aborted => Err(VolumeError::NotFound),
    }
}

#[async_trait]
impl VolumeClient for InMemoryVolume {
    async fn stat(
        &self,
        key: &str,
        version_id: Option<&str>,
    ) -> Result<ObjectMetadata, VolumeError> {
        relative_key(key)?;
        full_key(self.prefix_len, key)?;
        let state = self.state.lock().map_err(|_| VolumeError::RequestFailed)?;
        let (key, object) = select(&state, key, version_id)?;
        check_visible(key, object)?;
        Ok(ObjectMetadata {
            key: key.clone(),
            size: object.body.len() as u64,
            version: object.version.clone(),
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
        full_key(self.prefix_len, key)?;
        let state = self.state.lock().map_err(|_| VolumeError::RequestFailed)?;
        let (_, object) = select(&state, key, reference.version_id.as_deref())?;
        check_reference(reference, &object.version)?;
        check_visible(key, object)?;
        let body = selected_body(&object.body, range);
        Ok(ObjectRead {
            version: object.version.clone(),
            length: body.len() as u64,
            body: ObjectBody::from_bytes(body),
        })
    }

    async fn list(
        &self,
        prefix: &str,
        continuation: Option<&str>,
    ) -> Result<ObjectPage, VolumeError> {
        relative_key(prefix)?;
        let after = continuation.map(continuation_key).transpose()?;
        let state = self.state.lock().map_err(|_| VolumeError::RequestFailed)?;
        Ok(object_page(
            &state.objects,
            prefix,
            after.as_deref(),
            self.page_size,
        ))
    }

    async fn put(
        &self,
        target: WriteTarget<'_>,
        condition: WriteCondition<'_>,
        body: Bytes,
    ) -> Result<ObjectVersion, VolumeError> {
        let key: Arc<str> = write_key("", target)?.into();
        full_key(self.prefix_len, &key)?;
        check_not_marker(&key, body.is_empty())?;
        validate_condition(condition)?;
        let mut state = self.state.lock().map_err(|_| VolumeError::RequestFailed)?;
        check_condition(state.objects.get(&key).and_then(current_object), condition)?;
        let version = new_version(self.versioned, &body, Uuid::new_v4());
        let reference = version.clone();
        apply_object(
            &mut state,
            key,
            ObjectEvent::Publish(Arc::new(StoredObject { version, body })),
            self.versioned,
        )?;
        Ok(reference)
    }

    async fn delete(
        &self,
        target: WriteTarget<'_>,
        reference: &ObjectMetadata,
    ) -> Result<Option<ResponseValue<Arc<str>>>, VolumeError> {
        let key: Arc<str> = write_key("", target)?.into();
        validate_delete(&key, reference)?;
        check_not_marker(&key, reference.size == 0)?;
        full_key(self.prefix_len, &key)?;
        let mut state = self.state.lock().map_err(|_| VolumeError::RequestFailed)?;
        check_condition(
            state.objects.get(&key).and_then(current_object),
            WriteCondition::Matches(&reference.version.etag),
        )?;
        let marker = apply_object(
            &mut state,
            key,
            ObjectEvent::Delete {
                marker_id: Uuid::new_v4().to_string().into(),
            },
            self.versioned,
        )?;
        Ok(marker.map(ResponseValue::new))
    }

    async fn start_multipart(
        &self,
        target: WriteTarget<'_>,
    ) -> Result<ResponseValue<Arc<str>>, VolumeError> {
        let key: Arc<str> = multipart_key("", target)?.into();
        full_key(self.prefix_len, &key)?;
        let id: Arc<str> = Uuid::new_v4().to_string().into();
        let result = ResponseValue::new(id.clone());
        let mut state = self.state.lock().map_err(|_| VolumeError::RequestFailed)?;
        state.uploads.insert(
            id,
            Upload {
                key,
                phase: UploadPhase::Open(BTreeMap::new()),
            },
        );
        Ok(result)
    }

    async fn upload_part(
        &self,
        target: WriteTarget<'_>,
        upload_id: &str,
        number: i32,
        body: Bytes,
    ) -> Result<UploadedPart, VolumeError> {
        let key = multipart_key("", target)?;
        full_key(self.prefix_len, &key)?;
        let metadata = UploadedPart {
            number,
            etag: format!("\"{:x}\"", Sha256::digest(&body)).into(),
            size: body.len() as u64,
        };
        validate_parts(std::slice::from_ref(&metadata))?;
        let part = Arc::new(StoredPart {
            number,
            etag: metadata.etag.clone(),
            body,
        });
        let mut state = self.state.lock().map_err(|_| VolumeError::RequestFailed)?;
        let upload = upload(&mut state, &key, upload_id)?;
        let next = upload_transition(upload, UploadEvent::Part(part))?;
        *upload = next;
        Ok(metadata)
    }

    async fn complete_multipart(
        &self,
        target: WriteTarget<'_>,
        upload_id: &str,
        parts: &[UploadedPart],
        condition: WriteCondition<'_>,
    ) -> Result<ObjectVersion, VolumeError> {
        validate_parts(parts)?;
        validate_condition(condition)?;
        let key = multipart_key("", target)?;
        full_key(self.prefix_len, &key)?;
        let mut state = self.state.lock().map_err(|_| VolumeError::RequestFailed)?;
        check_condition(
            state.objects.get(key.as_ref()).and_then(current_object),
            condition,
        )?;
        let upload = upload(&mut state, &key, upload_id)?;
        let retained = retained_object(upload);
        let (next, object) =
            complete_upload(upload, retained, parts, self.versioned, Uuid::new_v4())?;
        let reference = object.version.clone();
        if let Some(next) = next {
            let publication_key = upload.key.clone();
            *upload = next;
            apply_object(
                &mut state,
                publication_key,
                ObjectEvent::Publish(object),
                self.versioned,
            )?;
        }
        Ok(reference)
    }

    async fn abort_multipart(
        &self,
        target: WriteTarget<'_>,
        upload_id: &str,
    ) -> Result<(), VolumeError> {
        let key = multipart_key("", target)?;
        full_key(self.prefix_len, &key)?;
        let mut state = self.state.lock().map_err(|_| VolumeError::RequestFailed)?;
        let upload = upload(&mut state, &key, upload_id)?;
        let next = upload_transition(upload, UploadEvent::Abort)?;
        *upload = next;
        Ok(())
    }

    async fn versioning(&self) -> Result<BucketVersioning, VolumeError> {
        Ok(versioning_status(self.versioned))
    }

    async fn lifecycle(&self) -> Result<Box<[LifecycleRule]>, VolumeError> {
        Ok(Box::new([]))
    }
}

#[cfg(test)]
mod tests;
