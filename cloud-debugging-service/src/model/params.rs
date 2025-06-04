// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use cloud_common::model::TokenSecret;
use golem_common::model::oplog::OplogIndex;
use golem_common::model::public_oplog::PublicOplogEntry;
use golem_common::model::WorkerId;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
pub struct ConnectParams {
    pub worker_id: WorkerId,
    pub token: TokenSecret,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct PlaybackParams {
    pub target_index: OplogIndex,
    pub overrides: Option<Vec<PlaybackOverride>>,
    pub ensure_invocation_boundary: Option<bool>,
    pub time_out_in_seconds: Option<u64>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PlaybackOverride {
    pub index: OplogIndex,
    pub oplog: PublicOplogEntry,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct RewindParams {
    pub target_index: OplogIndex,
    pub ensure_invocation_boundary: Option<bool>,
    pub time_out_in_seconds: Option<u64>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ForkParams {
    pub target_worker_id: WorkerId,
    pub oplog_index_cut_off: OplogIndex,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ConnectResult {
    pub worker_id: WorkerId,
    pub success: bool,
    pub message: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct PlaybackResult {
    pub worker_id: WorkerId,
    pub current_index: OplogIndex,
    pub success: bool,
    pub message: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct RewindResult {
    pub worker_id: WorkerId,
    pub current_index: OplogIndex,
    pub success: bool,
    pub message: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ForkResult {
    pub source_worker_id: WorkerId,
    pub target_worker_id: WorkerId,
    pub success: bool,
    pub message: String,
}
