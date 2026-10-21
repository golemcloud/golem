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

impl From<PublicUpdateDescription> for oplog::UpdateDescription {
    fn from(value: PublicUpdateDescription) -> Self {
        match value {
            PublicUpdateDescription::Automatic(_) => Self::AutoUpdate,
            PublicUpdateDescription::SnapshotAssistedAutomatic(_) => {
                Self::SnapshotAssistedAutomatic
            }
            PublicUpdateDescription::SnapshotBased(SnapshotBasedUpdateParameters {
                payload,
                mime_type,
                filesystem_snapshot,
            }) => Self::SnapshotBased(oplog::SnapshotBasedUpdateParameters {
                payload,
                mime_type,
                filesystem_snapshot,
            }),
        }
    }
}

impl TryFrom<oplog::RawUpdateDescription> for golem_common::model::oplog::UpdateDescription {
    type Error = String;

    fn try_from(desc: oplog::RawUpdateDescription) -> Result<Self, String> {
        match desc {
            oplog::RawUpdateDescription::Automatic(target_revision) => Ok(Self::Automatic {
                target_revision: golem_common::model::component::ComponentRevision::try_from(
                    target_revision,
                )
                .map_err(|e| e.to_string())?,
            }),
            oplog::RawUpdateDescription::SnapshotAssistedAutomatic(update) => {
                Ok(Self::SnapshotAssistedAutomatic {
                    target_revision: golem_common::model::component::ComponentRevision::try_from(
                        update.target_revision,
                    )
                    .map_err(|e| e.to_string())?,
                    source_component_revision:
                        golem_common::model::component::ComponentRevision::try_from(
                            update.source_component_revision,
                        )
                        .map_err(|e| e.to_string())?,
                    source_revision_start_index: golem_common::model::oplog::OplogIndex::from_u64(
                        update.source_revision_start_index,
                    ),
                    snapshot_index: golem_common::model::oplog::OplogIndex::from_u64(
                        update.snapshot_index,
                    ),
                    snapshot_revision: golem_common::model::component::ComponentRevision::try_from(
                        update.snapshot_revision,
                    )
                    .map_err(|e| e.to_string())?,
                    filesystem_snapshot: update
                        .filesystem_snapshot
                        .map(|name| name.parse())
                        .transpose()?,
                })
            }
            oplog::RawUpdateDescription::SnapshotBased(sbu) => Ok(Self::SnapshotBased {
                target_revision: golem_common::model::component::ComponentRevision::try_from(
                    sbu.target_revision,
                )
                .map_err(|e| e.to_string())?,
                payload: oplog_payload_from_wit(sbu.payload),
                mime_type: sbu.mime_type,
                filesystem_snapshot: sbu
                    .filesystem_snapshot
                    .map(|name| name.parse())
                    .transpose()?,
            }),
        }
    }
}

pub(super) fn multipart_to_raw(multipart: MultipartSnapshotData) -> (Vec<u8>, String) {
    use golem_common::base_model::oplog::multipart::extract_boundary;

    let boundary = extract_boundary(&multipart.mime_type)
        .unwrap_or("boundary")
        .to_string();

    let mut output = Vec::new();
    for part in &multipart.parts {
        output.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        output.extend_from_slice(format!("Content-Type: {}\r\n", part.content_type).as_bytes());
        output.extend_from_slice(
            format!(
                "Content-Disposition: attachment; name=\"{}\"\r\n",
                part.name
            )
            .as_bytes(),
        );
        output.extend_from_slice(b"\r\n");
        match &part.data {
            MultipartPartData::Json(json) => {
                output.extend_from_slice(
                    serde_json::to_vec(&json.data)
                        .unwrap_or_default()
                        .as_slice(),
                );
            }
            MultipartPartData::Raw(raw) => {
                output.extend_from_slice(&raw.data);
            }
        }
        output.extend_from_slice(b"\r\n");
    }
    output.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    (output, multipart.mime_type)
}

impl TryFrom<golem_common::model::oplog::UpdateDescription> for oplog::RawUpdateDescription {
    type Error = String;

    fn try_from(desc: golem_common::model::oplog::UpdateDescription) -> Result<Self, String> {
        use golem_common::model::oplog::UpdateDescription;
        match desc {
            UpdateDescription::Automatic { target_revision } => {
                Ok(Self::Automatic(target_revision.into()))
            }
            UpdateDescription::SnapshotAssistedAutomatic {
                target_revision,
                source_component_revision,
                source_revision_start_index,
                snapshot_index,
                snapshot_revision,
                filesystem_snapshot,
            } => Ok(Self::SnapshotAssistedAutomatic(
                oplog::RawSnapshotAssistedAutomaticUpdate {
                    target_revision: target_revision.into(),
                    source_component_revision: source_component_revision.into(),
                    source_revision_start_index: source_revision_start_index.into(),
                    snapshot_index: snapshot_index.into(),
                    snapshot_revision: snapshot_revision.into(),
                    filesystem_snapshot: filesystem_snapshot.map(String::from),
                },
            )),
            UpdateDescription::SnapshotBased {
                target_revision,
                payload,
                mime_type,
                filesystem_snapshot,
            } => Ok(Self::SnapshotBased(oplog::RawSnapshotBasedUpdate {
                target_revision: target_revision.into(),
                payload: oplog_payload_to_wit(payload)?,
                mime_type,
                filesystem_snapshot: filesystem_snapshot.map(String::from),
            })),
        }
    }
}

impl From<RawSnapshotData> for crate::preview2::golem_api_1_x::host::Snapshot {
    fn from(value: RawSnapshotData) -> Self {
        Self {
            payload: value.data,
            mime_type: value.mime_type,
        }
    }
}

impl From<crate::preview2::golem_api_1_x::host::Snapshot> for RawSnapshotData {
    fn from(value: crate::preview2::golem_api_1_x::host::Snapshot) -> Self {
        Self {
            data: value.payload,
            mime_type: value.mime_type,
        }
    }
}
