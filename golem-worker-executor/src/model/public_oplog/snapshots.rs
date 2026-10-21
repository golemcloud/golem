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

use super::*;

pub(super) fn raw_snapshot_to_public(snapshot: RawSnapshotData) -> PublicSnapshotData {
    if snapshot.mime_type == "application/json" {
        match serde_json::from_slice(&snapshot.data) {
            Ok(json_value) => PublicSnapshotData::Json(JsonSnapshotData { data: json_value }),
            Err(_) => PublicSnapshotData::Raw(snapshot),
        }
    } else if snapshot.mime_type.starts_with("multipart/mixed") {
        parse_multipart_snapshot(snapshot)
    } else {
        PublicSnapshotData::Raw(snapshot)
    }
}

fn parse_multipart_snapshot(snapshot: RawSnapshotData) -> PublicSnapshotData {
    use golem_common::base_model::oplog::multipart::{extract_boundary, parse_multipart_mixed};

    let boundary = match extract_boundary(&snapshot.mime_type) {
        Some(b) => b.to_string(),
        None => return PublicSnapshotData::Raw(snapshot),
    };

    let parsed = match parse_multipart_mixed(&boundary, &snapshot.data) {
        Some(parts) => parts,
        None => return PublicSnapshotData::Raw(snapshot),
    };

    let parts = parsed
        .iter()
        .map(|p| {
            let name = p.name.clone().unwrap_or_default();
            let content_type = p.content_type.clone().unwrap_or_default();
            let data = if content_type == "application/json" {
                match serde_json::from_slice(p.body) {
                    Ok(json_value) => {
                        MultipartPartData::Json(JsonSnapshotData { data: json_value })
                    }
                    Err(_) => MultipartPartData::Raw(RawSnapshotData {
                        data: p.body.to_vec(),
                        mime_type: content_type.clone(),
                    }),
                }
            } else {
                MultipartPartData::Raw(RawSnapshotData {
                    data: p.body.to_vec(),
                    mime_type: content_type.clone(),
                })
            };
            MultipartSnapshotPart {
                name,
                content_type,
                data,
            }
        })
        .collect();

    PublicSnapshotData::Multipart(MultipartSnapshotData {
        mime_type: snapshot.mime_type,
        parts,
    })
}
