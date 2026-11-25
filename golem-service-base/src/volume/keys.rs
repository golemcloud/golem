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

use super::{ObjectMetadata, ObjectVersion, UploadedPart, VolumeError, WriteTarget};
use std::ops::RangeInclusive;

pub(super) fn relative_key(key: &str) -> Result<(), VolumeError> {
    if key.starts_with('/') || key.split('/').any(|part| matches!(part, "." | "..")) {
        Err(VolumeError::InvalidRequest)
    } else {
        Ok(())
    }
}

pub(super) fn full_key(key: &str) -> Result<(), VolumeError> {
    if key.is_empty() || key.len() > 1024 {
        Err(VolumeError::InvalidRequest)
    } else {
        Ok(())
    }
}

pub(super) fn write_key(prefix: &str, target: WriteTarget<'_>) -> Result<Box<str>, VolumeError> {
    let (reserved, key) = match target {
        WriteTarget::File(key) if key != ".golem" && !key.starts_with(".golem/") => ("", key),
        WriteTarget::Probe(key) => (".golem/", key),
        _ => return Err(VolumeError::InvalidRequest),
    };
    relative_key(key)?;
    if key.is_empty() {
        return Err(VolumeError::InvalidRequest);
    }
    let key = [prefix, reserved, key].concat().into_boxed_str();
    full_key(&key)?;
    Ok(key)
}

pub(super) fn check_not_marker(key: &str, empty: bool) -> Result<(), VolumeError> {
    if empty && key.ends_with('/') {
        Err(VolumeError::InvalidRequest)
    } else {
        Ok(())
    }
}

pub(super) fn validate_read(
    reference: &ObjectVersion,
    range: Option<&RangeInclusive<u64>>,
) -> Result<(), VolumeError> {
    if !strong_etag(&reference.etag) || range.is_some_and(RangeInclusive::is_empty) {
        Err(VolumeError::InvalidRequest)
    } else {
        Ok(())
    }
}

pub(super) fn validate_delete(key: &str, reference: &ObjectMetadata) -> Result<(), VolumeError> {
    if key != reference.key.as_ref() || !strong_etag(&reference.version.etag) {
        Err(VolumeError::InvalidRequest)
    } else {
        Ok(())
    }
}

pub(super) fn validate_parts(parts: &[UploadedPart]) -> Result<(), VolumeError> {
    if parts.is_empty()
        || parts.iter().any(|part| !(1..=10000).contains(&part.number))
        || parts
            .windows(2)
            .any(|pair| pair[0].number >= pair[1].number)
    {
        Err(VolumeError::InvalidRequest)
    } else {
        Ok(())
    }
}

pub(super) fn strong_etag(etag: &str) -> bool {
    etag.strip_prefix('"')
        .and_then(|tag| tag.strip_suffix('"'))
        .is_some_and(|tag| {
            tag.bytes()
                .all(|byte| byte >= 0x21 && byte != b'"' && byte != 0x7f)
        })
}
