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
use futures::future::{join, join_all};
use golem_rust::{
    Invocation, OwnedInvocationId, agent_definition, agent_implementation, read_only,
};

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

    /// Triggers the read-only `get_count` on the target and returns the
    /// invocation's ID.
    async fn trigger_read(&self, target_id: String) -> String;

    /// Awaits a read-only `slow_read(ms)` on the target and, while it is in
    /// flight, triggers an equivalent `slow_read(ms)`. Returns the awaited
    /// value and the triggered invocation's ID.
    async fn trigger_read_beside_slow_read(&self, target_id: String, ms: u64) -> (u64, String);

    /// The status of the invocation with this ID, through a rebuilt handle.
    async fn read_status(&self, id: String) -> String;

    /// The `u64` result of the invocation with this ID, through a rebuilt
    /// handle.
    async fn await_read(&self, id: String) -> u64;
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
        let client = ReadonlyAgentClient::upsert(target_id);
        client.get_count().await
    }

    async fn slow_then_read(&self, target_id: String, ms: u64) -> u64 {
        let mut client = ReadonlyAgentClient::upsert(target_id);
        // Fire-and-forget: enqueue `slow_increment` on the target without
        // waiting for it to complete. The next `get_count` call must bypass
        // the target's invocation queue via the read-only cache.
        client
            .trigger_slow_increment(ms)
            .await
            .expect("failed to trigger slow_increment");
        client.get_count().await
    }

    async fn parallel_reads(&self, target_id: String, n: u32) -> Vec<u64> {
        let futures = (0..n)
            .map(|_| {
                let client = ReadonlyAgentClient::upsert(target_id.clone());
                async move { client.get_count().await }
            })
            .collect::<Vec<_>>();
        join_all(futures).await
    }

    async fn bad_rpc(&self, target_id: String) -> u64 {
        let client = ReadonlyAgentClient::upsert(target_id);
        // `WriteRemote`: traps before the RPC is dispatched because
        // the caller is in read-only strictness mode.
        client.get_count().await
    }

    async fn trigger_read(&self, target_id: String) -> String {
        let mut client = ReadonlyAgentClient::upsert(target_id);
        client
            .trigger_get_count()
            .await
            .expect("failed to trigger get_count")
            .id()
            .to_string()
    }

    async fn trigger_read_beside_slow_read(&self, target_id: String, ms: u64) -> (u64, String) {
        let awaited_client = ReadonlyAgentClient::upsert(target_id.clone());
        let mut triggering_client = ReadonlyAgentClient::upsert(target_id);
        let awaited = awaited_client.slow_read(ms);
        let triggered = async {
            golem_rust::wasip3::clocks::monotonic_clock::wait_for(TRIGGER_DELAY_NANOS).await;
            triggering_client
                .trigger_slow_read(ms)
                .await
                .expect("failed to trigger slow_read")
                .id()
                .to_string()
        };
        join(awaited, triggered).await
    }

    async fn read_status(&self, id: String) -> String {
        let status = Invocation::<u64>::get(&id_of(&id))
            .status()
            .await
            .expect("failed to read the status");
        format!("{status:?}")
    }

    async fn await_read(&self, id: String) -> u64 {
        Invocation::<u64>::get(&id_of(&id))
            .await_result()
            .await
            .expect("failed to await the result")
    }
}

/// How long `trigger_read_beside_slow_read` waits before it triggers, so the
/// awaited `slow_read` is in flight on the target.
const TRIGGER_DELAY_NANOS: u64 = 200_000_000;

fn id_of(id: &str) -> OwnedInvocationId {
    id.parse().expect("invalid invocation ID")
}
