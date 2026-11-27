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
use crate::services::oplog::{ScanPage, decode_scan_cursor, next_scan_cursor};
use crate::storage::indexed::ScanResume;
use golem_common::model::ScanCursor;
use golem_common::model::type_id::TypeId;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct SealScanResume {
    after: PathBuf,
    pending: Option<PendingAgent>,
}

#[derive(Serialize, Deserialize)]
struct PendingAgent {
    id: AgentId,
    head: Option<PathBuf>,
}

impl PendingAgent {
    fn listed(self, environment_id: EnvironmentId) -> Option<OwnedAgentId> {
        self.head
            .as_deref()
            .and_then(SealRecordKey::parse)
            .filter(|head| matches!(head.kind, SealRecordKind::Sealed { .. }))
            .map(|_| OwnedAgentId::new(environment_id, &self.id))
    }
}

impl SealService {
    pub(crate) async fn is_marked(
        &self,
        id: &OwnedAgentId,
        mode: AgentMode,
    ) -> Result<bool, WorkerExecutorError> {
        let observation = self
            .observe(id)
            .await
            .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
        Ok(match mode {
            AgentMode::Durable => observation.durable_marker.is_some(),
            AgentMode::Ephemeral => observation.ephemeral_marker.is_some(),
        })
    }

    pub(crate) async fn scan_for_type(
        &self,
        environment_id: &EnvironmentId,
        type_id: TypeId,
        modes: Option<AgentMode>,
        cursor: ScanCursor,
        count: u64,
    ) -> Result<ScanPage, WorkerExecutorError> {
        let state = decode_scan_cursor(&cursor, modes)?;
        let resume = match &state.resume {
            None => None,
            Some(ScanResume::Marker(value)) => {
                let resume: SealScanResume = serde_json::from_str(value).map_err(|_| {
                    WorkerExecutorError::invalid_request("Invalid seal scan resume")
                })?;
                if !resume.after.starts_with("seals")
                    || resume.pending.as_ref().is_some_and(|pending| {
                        pending.id.type_id() != Some(type_id)
                            || pending.head.as_deref().is_some_and(|head| {
                                SealRecordKey::parse(head)
                                    .is_none_or(|head| head.agent_id != pending.id)
                            })
                    })
                {
                    return Err(WorkerExecutorError::invalid_request(
                        "Invalid seal scan resume",
                    ));
                }
                Some(resume)
            }
            Some(ScanResume::Cursor(_)) => {
                return Err(WorkerExecutorError::invalid_request(
                    "Invalid seal scan resume",
                ));
            }
        };
        let after = resume.as_ref().map(|resume| resume.after.as_path());
        let max_keys = count.clamp(1, 1000) as u32;
        let page = self
            .blobs
            .with("oplog_seal", "scan")
            .list_page(
                BlobStorageNamespace::SealRecords {
                    environment_id: *environment_id,
                    type_tag: type_id.get(),
                    agent_mode: state.mode,
                },
                Path::new("seals"),
                after,
                max_keys,
            )
            .await
            .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
        let full = page.len() == max_keys as usize;
        let examined_keys = page.len() as u64;
        let mut previous = after.map(Path::to_path_buf);
        let mut pending = resume.and_then(|resume| resume.pending);
        let mut page_agent = None;
        let mut page_agents = 0;
        let mut pending_start_after = previous.clone();
        let mut agent_ids = Vec::new();
        for entry in page {
            let path = entry.path.to_path_buf();
            let Some(key) = SealRecordKey::parse(&path) else {
                warn!(path = %path.display(), "Ignoring malformed seal record key");
                previous = Some(path);
                continue;
            };
            if !key.agent_id.is_issued() || key.agent_id.type_id() != Some(type_id) {
                previous = Some(path);
                continue;
            }
            if page_agent != Some(key.agent_id) {
                page_agents += 1;
                page_agent = Some(key.agent_id);
            }
            if pending
                .as_ref()
                .is_none_or(|agent| agent.id != key.agent_id)
            {
                if let Some(id) = pending
                    .take()
                    .and_then(|agent| agent.listed(*environment_id))
                {
                    agent_ids.push(id);
                }
                pending = Some(PendingAgent {
                    id: key.agent_id,
                    head: None,
                });
                pending_start_after = previous.clone();
            }
            let listed = ListedSealRecord { key, entry };
            let agent = pending.as_mut().expect("current agent was initialized");
            let head = agent.head.as_deref().and_then(SealRecordKey::parse);
            if listed.admissible() && head.is_none_or(|head| key > head) {
                agent.head = Some(path.clone());
            }
            previous = Some(path);
        }
        let next_resume = if full {
            let resume = if page_agents > 1 {
                // Revisit the last agent from its first key so a page never reports a prefix.
                SealScanResume {
                    after: pending_start_after.expect("a previous agent was examined"),
                    pending: None,
                }
            } else {
                // A single agent can occupy many pages. Carry only its highest admissible key.
                SealScanResume {
                    after: previous.expect("a full page has a last key"),
                    pending,
                }
            };
            Some(ScanResume::Marker(serde_json::to_string(&resume).map_err(
                |error| WorkerExecutorError::runtime(error.to_string()),
            )?))
        } else {
            if let Some(id) = pending.and_then(|agent| agent.listed(*environment_id)) {
                agent_ids.push(id);
            }
            None
        };
        Ok(ScanPage {
            cursor: next_scan_cursor(state, modes, next_resume)?,
            agent_ids,
            examined_keys,
        })
    }
}
