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

//! Caller agent used by the RPC-side read-only tests (R1..R4).
//!
//! `ReadonlyAgentClient` is auto-generated from
//! [`super::agent::ReadonlyAgent`] by the `#[agent_definition]` macro.

use super::agent::ReadonlyAgentClient;
use futures::future::join_all;
use golem_rust::{agent_definition, agent_implementation, read_only};

#[agent_definition]
pub trait ReadonlyCaller {
    fn new(id: String) -> Self;

    /// Calls `ReadonlyAgent::get_count` on the target agent over RPC.
    /// Two back-to-back calls of this method against the same `target_id`
    /// must result in only one `AgentInvocationStarted` on the target
    /// (R1: the second hit is served from the per-worker read-only cache).
    async fn read_via_rpc(&self, target_id: String) -> u64;

    /// Fires `slow_increment(ms)` on the target as fire-and-forget, then
    /// awaits a read-only `get_count` RPC against the same target.
    /// Used by R2 to verify that the read-only call bypasses the target's
    /// invocation queue even while the slow non-readonly invocation holds it.
    async fn slow_then_read(&self, target_id: String, ms: u64) -> u64;

    /// Fires `n` concurrent `get_count` RPCs against the target and returns
    /// the collected results. Used by R3 (concurrent miss coalescing).
    async fn parallel_reads(&self, target_id: String, n: u32) -> Vec<u64>;

    /// `#[read_only]` method that performs an RPC call — a `WriteRemote`
    /// host operation that must trap with `AgentError::ReadOnlyViolation`
    /// (R4).
    #[read_only]
    async fn bad_rpc(&self, target_id: String) -> u64;
}

pub struct ReadonlyCallerImpl {
    _id: String,
}

#[agent_implementation]
impl ReadonlyCaller for ReadonlyCallerImpl {
    fn new(id: String) -> Self {
        Self { _id: id }
    }

    async fn read_via_rpc(&self, target_id: String) -> u64 {
        let client = ReadonlyAgentClient::get(target_id);
        client.get_count().await
    }

    async fn slow_then_read(&self, target_id: String, ms: u64) -> u64 {
        let mut client = ReadonlyAgentClient::get(target_id);
        // Fire-and-forget: enqueue `slow_increment` on the target without
        // waiting for it to complete. The next `get_count` call must bypass
        // the target's invocation queue via the read-only cache.
        client.trigger_slow_increment(ms);
        client.get_count().await
    }

    async fn parallel_reads(&self, target_id: String, n: u32) -> Vec<u64> {
        let futures = (0..n)
            .map(|_| {
                let client = ReadonlyAgentClient::get(target_id.clone());
                async move { client.get_count().await }
            })
            .collect::<Vec<_>>();
        join_all(futures).await
    }

    async fn bad_rpc(&self, target_id: String) -> u64 {
        let client = ReadonlyAgentClient::get(target_id);
        // `WriteRemote`: traps before the RPC is dispatched because
        // the caller is in read-only strictness mode.
        client.get_count().await
    }
}
