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

//! Read-only test agent used by issue #3393.
//!
//! Mirrors the `agent-readonly` component described in the issue, but lives
//! inside the shared `agent-sdk-rust` component to avoid spinning up yet
//! another build target.

use golem_rust::agentic::Principal;
use golem_rust::{
    agent_definition, agent_implementation, create_promise, description, endpoint, read_only,
};
use wstd::http::{Body, Client, Request};

// NOTE on the `bad_*` surface used by T9 / R4:
//
// Only `Write*` `DurableFunctionType`s are uniformly trapped by the generic
// read-only check in `begin_durable_function`. `ReadLocal` (clock, random) and
// `ReadRemote` (most blob reads) are NOT trapped by the generic check;
// outgoing HTTP is special-cased via `check_read_only_allows` at the host
// function entry, and all RPC variants are similarly special-cased.
//
// Therefore the only `bad_*` methods we keep here are ones that *currently
// trap*: `bad_write` (`WriteLocal` via `create_promise`) and
// `bad_remote_read` (HTTP — special-cased). The RPC trap is exercised from
// the `ReadonlyCaller::bad_rpc` method in caller.rs.

#[agent_definition(mount = "/readonly-agents/{id}")]
pub trait ReadonlyAgent {
    fn new(id: String) -> Self;

    // -- non-readonly writes -------------------------------------------------
    /// Non-readonly write. Exposed over HTTP so H3 can trigger a write
    /// between two read-only HTTP calls to invalidate cached ETags.
    #[endpoint(post = "/increment")]
    fn increment(&mut self) -> u64;

    /// Non-readonly write that sleeps for `ms` milliseconds before incrementing.
    /// Used to hold the per-worker invocation queue while a concurrent
    /// read-only call bypasses the queue from the cache (test T4 / R2).
    async fn slow_increment(&mut self, ms: u64) -> u64;

    // -- read-only reads -----------------------------------------------------
    /// Default cache policy (`until_write`) and `uses_principal = false`.
    #[read_only]
    #[endpoint(get = "/count")]
    #[description("Returns the current counter value (read-only)")]
    fn get_count(&self) -> u64;

    /// Read-only method that sleeps for `ms` milliseconds before returning
    /// the current count. Used by T11b to keep the coalesced owner future
    /// pending long enough to reliably abort the original caller while the
    /// pending cache entry is still in flight.
    #[read_only]
    async fn slow_read(&self, ms: u64) -> u64;

    /// Principal-aware read; `uses_principal` is auto-derived from the
    /// presence of the `Principal` parameter in the signature. Exposed over
    /// HTTP so the HTTP integration tests can assert that
    /// `uses_principal = true` produces `Cache-Control: private, ...` and a
    /// `Vary` header that includes the principal-carrying request header.
    #[read_only]
    #[endpoint(get = "/count-for")]
    fn get_count_for(&self, _principal: Principal) -> u64;

    /// TTL-based cache policy. Exposed over HTTP so H5 can assert
    /// `Cache-Control: ..., max-age=2`.
    #[read_only(cache = "ttl", ttl = "2s")]
    #[endpoint(get = "/ttl-count")]
    fn read_only_with_ttl(&self) -> u64;

    /// Side-effect-free guarantee with no caching.
    #[read_only(cache = "no_cache")]
    fn pure_compute(&self, x: u32, y: u32) -> u32;

    // -- read-only violations ------------------------------------------------
    /// `#[read_only]` doing a `WriteLocal` host call (create-promise).
    /// Must trap with `AgentError::ReadOnlyViolation`.
    #[read_only]
    fn bad_write(&self) -> u64;

    /// `#[read_only]` doing an outgoing HTTP call.
    /// Must trap with `AgentError::ReadOnlyViolation` because
    /// `outgoing_handler::handle` calls `check_read_only_allows` before
    /// dispatching.
    #[read_only]
    async fn bad_remote_read(&self) -> u64;
}

pub struct ReadonlyAgentImpl {
    _id: String,
    count: u64,
}

#[agent_implementation]
impl ReadonlyAgent for ReadonlyAgentImpl {
    fn new(id: String) -> Self {
        Self { _id: id, count: 0 }
    }

    fn increment(&mut self) -> u64 {
        self.count += 1;
        self.count
    }

    async fn slow_increment(&mut self, ms: u64) -> u64 {
        wstd::task::sleep(std::time::Duration::from_millis(ms).into()).await;
        self.count += 1;
        self.count
    }

    fn get_count(&self) -> u64 {
        self.count
    }

    async fn slow_read(&self, ms: u64) -> u64 {
        wstd::task::sleep(std::time::Duration::from_millis(ms).into()).await;
        self.count
    }

    fn get_count_for(&self, _principal: Principal) -> u64 {
        self.count
    }

    fn read_only_with_ttl(&self) -> u64 {
        self.count
    }

    fn pure_compute(&self, x: u32, y: u32) -> u32 {
        // A read-only, `no_cache` method that is genuinely pure — it must not
        // touch the host, only its inputs.
        x.wrapping_add(y).wrapping_mul(3)
    }

    fn bad_write(&self) -> u64 {
        // `create_promise` is a `DurableFunctionType::WriteLocal` host call.
        let _ = create_promise();
        // Unreachable — the host call traps before returning.
        self.count
    }

    async fn bad_remote_read(&self) -> u64 {
        // Outgoing HTTP. `outgoing_handler::handle` traps with
        // `WorkerReadOnlyViolation` before the request is dispatched, so the
        // exact URL is irrelevant.
        let request = Request::get("http://127.0.0.1:1/")
            .body(Body::empty())
            .unwrap();
        let _ = Client::new().send(request).await;
        self.count
    }
}
