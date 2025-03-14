// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::collections::HashMap;
use std::hash::Hash;

pub trait ToOss<T> {
    fn to_oss(self) -> T;
}

impl<A: ToOss<B>, B> ToOss<Box<B>> for Box<A> {
    fn to_oss(self) -> Box<B> {
        Box::new((*self).to_oss())
    }
}

impl<A: ToOss<B>, B> ToOss<Option<B>> for Option<A> {
    fn to_oss(self) -> Option<B> {
        self.map(|v| v.to_oss())
    }
}

impl<A: ToOss<B>, B> ToOss<Vec<B>> for Vec<A> {
    fn to_oss(self) -> Vec<B> {
        self.into_iter().map(|v| v.to_oss()).collect()
    }
}

impl<K: Eq + Hash, A: ToOss<B>, B> ToOss<HashMap<K, B>> for HashMap<K, A> {
    fn to_oss(self) -> HashMap<K, B> {
        self.into_iter().map(|(k, v)| (k, v.to_oss())).collect()
    }
}

impl ToOss<golem_client::model::WorkerId> for golem_cloud_client::model::WorkerId {
    fn to_oss(self) -> golem_client::model::WorkerId {
        golem_client::model::WorkerId {
            component_id: self.component_id,
            worker_name: self.worker_name,
        }
    }
}

impl ToOss<golem_client::model::ScanCursor> for golem_cloud_client::model::ScanCursor {
    fn to_oss(self) -> golem_client::model::ScanCursor {
        golem_client::model::ScanCursor {
            cursor: self.cursor,
            layer: self.layer,
        }
    }
}

impl ToOss<golem_client::model::WorkerStatus> for golem_cloud_client::model::WorkerStatus {
    fn to_oss(self) -> golem_client::model::WorkerStatus {
        match self {
            golem_cloud_client::model::WorkerStatus::Running => {
                golem_client::model::WorkerStatus::Running
            }
            golem_cloud_client::model::WorkerStatus::Idle => {
                golem_client::model::WorkerStatus::Idle
            }
            golem_cloud_client::model::WorkerStatus::Suspended => {
                golem_client::model::WorkerStatus::Suspended
            }
            golem_cloud_client::model::WorkerStatus::Interrupted => {
                golem_client::model::WorkerStatus::Interrupted
            }
            golem_cloud_client::model::WorkerStatus::Retrying => {
                golem_client::model::WorkerStatus::Retrying
            }
            golem_cloud_client::model::WorkerStatus::Failed => {
                golem_client::model::WorkerStatus::Failed
            }
            golem_cloud_client::model::WorkerStatus::Exited => {
                golem_client::model::WorkerStatus::Exited
            }
        }
    }
}

impl ToOss<golem_client::model::PendingUpdate> for golem_cloud_client::model::PendingUpdate {
    fn to_oss(self) -> golem_client::model::PendingUpdate {
        golem_client::model::PendingUpdate {
            timestamp: self.timestamp,
            target_version: self.target_version,
        }
    }
}

impl ToOss<golem_client::model::SuccessfulUpdate> for golem_cloud_client::model::SuccessfulUpdate {
    fn to_oss(self) -> golem_client::model::SuccessfulUpdate {
        golem_client::model::SuccessfulUpdate {
            timestamp: self.timestamp,
            target_version: self.target_version,
        }
    }
}

impl ToOss<golem_client::model::FailedUpdate> for golem_cloud_client::model::FailedUpdate {
    fn to_oss(self) -> golem_client::model::FailedUpdate {
        golem_client::model::FailedUpdate {
            timestamp: self.timestamp,
            target_version: self.target_version,
            details: self.details,
        }
    }
}

impl ToOss<golem_client::model::UpdateRecord> for golem_cloud_client::model::UpdateRecord {
    fn to_oss(self) -> golem_client::model::UpdateRecord {
        match self {
            golem_cloud_client::model::UpdateRecord::PendingUpdate(u) => {
                golem_client::model::UpdateRecord::PendingUpdate(u.to_oss())
            }
            golem_cloud_client::model::UpdateRecord::SuccessfulUpdate(u) => {
                golem_client::model::UpdateRecord::SuccessfulUpdate(u.to_oss())
            }
            golem_cloud_client::model::UpdateRecord::FailedUpdate(u) => {
                golem_client::model::UpdateRecord::FailedUpdate(u.to_oss())
            }
        }
    }
}

impl ToOss<golem_client::model::IndexedWorkerMetadata>
    for golem_cloud_client::model::IndexedWorkerMetadata
{
    fn to_oss(self) -> golem_client::model::IndexedWorkerMetadata {
        golem_client::model::IndexedWorkerMetadata {
            resource_name: self.resource_name,
            resource_params: self.resource_params,
        }
    }
}

impl ToOss<golem_client::model::ResourceMetadata> for golem_cloud_client::model::ResourceMetadata {
    fn to_oss(self) -> golem_client::model::ResourceMetadata {
        golem_client::model::ResourceMetadata {
            created_at: self.created_at,
            indexed: self.indexed.to_oss(),
        }
    }
}

impl ToOss<golem_client::model::ApiDefinitionInfo>
    for golem_cloud_client::model::ApiDefinitionInfo
{
    fn to_oss(self) -> golem_client::model::ApiDefinitionInfo {
        let golem_cloud_client::model::ApiDefinitionInfo { id, version } = self;

        golem_client::model::ApiDefinitionInfo { id, version }
    }
}

impl ToOss<golem_client::model::ApiSite> for golem_cloud_client::model::ApiSite {
    fn to_oss(self) -> golem_client::model::ApiSite {
        let golem_cloud_client::model::ApiSite { host, subdomain } = self;
        golem_client::model::ApiSite { host, subdomain }
    }
}

impl ToOss<golem_client::model::VersionedComponentId>
    for golem_cloud_client::model::VersionedComponentId
{
    fn to_oss(self) -> golem_client::model::VersionedComponentId {
        golem_client::model::VersionedComponentId {
            component_id: self.component_id,
            version: self.version,
        }
    }
}

impl ToOss<golem_client::model::InvokeParameters> for golem_cloud_client::model::InvokeParameters {
    fn to_oss(self) -> golem_client::model::InvokeParameters {
        golem_client::model::InvokeParameters {
            params: self.params,
        }
    }
}

impl ToOss<golem_client::model::InvokeResult> for golem_cloud_client::model::InvokeResult {
    fn to_oss(self) -> golem_client::model::InvokeResult {
        golem_client::model::InvokeResult {
            result: self.result,
        }
    }
}
