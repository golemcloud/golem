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

use super::VolumeError;

pub(super) fn full_key(prefix_len: usize, key: &str) -> Result<(), VolumeError> {
    match prefix_len.checked_add(key.len()) {
        Some(0) => Err(VolumeError::NotFound),
        Some(1..=1024) => Ok(()),
        _ => Err(VolumeError::UnrepresentablePath),
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
