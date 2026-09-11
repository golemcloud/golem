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

pub mod error;
pub mod etcd_connection;
pub mod etcd_retry;
pub mod healthcheck;
pub mod healthcheck_loop;
pub mod leader_election;
mod model;
pub mod persistence;
pub mod rebalancing;
pub mod shard_management;
pub mod worker_executor;

pub use model::{
    ExecutorAddr, ExecutorAddrs, ExecutorId, ExecutorLease, ExecutorShards, RegisterAck,
    ShardAssignmentEntry, ShardAssignmentPush, ShardEpoch, ShardLeaseGrant, ShardLeaseRevision,
    ShardLeaseState,
};
