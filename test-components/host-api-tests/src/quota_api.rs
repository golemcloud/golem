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

use golem_rust::quota::{QuotaToken, with_reservation};
use golem_rust::{Schema, agent_definition, agent_implementation};
use golem_wasi_http::Client;
use serde::{Deserialize, Serialize};

/// The amount that was actually reserved (always equal to the requested amount
/// on the happy path) and the amount committed back to the pool.
#[derive(Clone, Debug, Schema, Serialize, Deserialize)]
pub struct ReserveCommitResult {
    pub reserved: u64,
    pub committed: u64,
}

/// Result of a split-then-reserve pair.
#[derive(Clone, Debug, Schema, Serialize, Deserialize)]
pub struct SplitResult {
    /// How many units were reserved from the parent half.
    pub parent_reserved: u64,
    /// How many units were reserved from the child half.
    pub child_reserved: u64,
}

#[agent_definition]
pub trait QuotaApi {
    fn new(name: String) -> Self;

    /// Acquire a token, reserve `amount`, commit `commit_amount` (≤ amount),
    /// and return both values.  Tests the basic reserve/commit round-trip.
    fn reserve_and_commit(
        &self,
        resource_name: String,
        expected_use: u64,
        amount: u64,
        commit_amount: u64,
    ) -> ReserveCommitResult;

    /// Acquire a token, reserve `amount`, then drop the reservation without
    /// committing (implicit commit of 0).  Returns the reserved amount.
    fn reserve_and_drop(&self, resource_name: String, expected_use: u64, amount: u64) -> u64;

    /// Acquire a token and attempt to reserve `amount`.  Returns `true` when
    /// the reservation succeeded, `false` when a `FailedReservation` was
    /// returned.
    fn try_reserve(&self, resource_name: String, expected_use: u64, amount: u64) -> bool;

    /// Acquire a token with `expected_use`, split off a child with
    /// `child_expected_use`, reserve `amount` from each, and return both
    /// reserved amounts.  Tests that split tokens are independently functional.
    fn split_and_reserve(
        &self,
        resource_name: String,
        expected_use: u64,
        child_expected_use: u64,
        amount: u64,
    ) -> SplitResult;

    /// Acquire two tokens for the same resource, merge the second into the
    /// first, then reserve `amount` from the merged token.  Returns the
    /// reserved amount.  Tests that merging combines capacity correctly.
    fn merge_and_reserve(
        &self,
        resource_name: String,
        expected_use_a: u64,
        expected_use_b: u64,
        amount: u64,
    ) -> u64;

    /// Acquire a token and drop it immediately — used to verify the lease
    /// reference count decrements without a panic.
    fn acquire_and_drop(&self, resource_name: String, expected_use: u64);

    /// Acquire a token and loop up to `max_iterations` times, reserving 1 unit
    /// per iteration and making a GET request to `http://{host}:{port}/call`
    /// on each successful reservation.  Stops early if the reservation is
    /// rejected (enforcement = reject) or `max_iterations` is reached.
    ///
    /// Returns the number of HTTP calls successfully made.  This is the
    /// primary driver for rate-limit tests: the test counts incoming requests
    /// at the server side and compares to the expected quota limit.
    fn reserve_in_loop(
        &self,
        resource_name: String,
        expected_use: u64,
        host: String,
        port: u16,
        max_iterations: u64,
    ) -> u64;

    /// Try to reserve `amount` units in a single call.  Returns the number of
    /// units actually reserved (the full amount on success, 0 on rejection).
    /// Commits the full amount on success.  Used to test shared-quota
    /// enforcement across multiple agents: each agent calls this once and the
    /// test verifies the sum of returned values does not exceed the capacity.
    fn try_reserve_and_commit(&self, resource_name: String, expected_use: u64, amount: u64) -> u64;
}

pub struct QuotaApiImpl {
    _name: String,
}

#[agent_implementation]
impl QuotaApi for QuotaApiImpl {
    fn new(name: String) -> Self {
        Self { _name: name }
    }

    fn reserve_and_commit(
        &self,
        resource_name: String,
        expected_use: u64,
        amount: u64,
        commit_amount: u64,
    ) -> ReserveCommitResult {
        let token = QuotaToken::new(&resource_name, expected_use);
        let reservation = token.reserve(amount).expect("reservation should succeed");
        reservation.commit(commit_amount);
        ReserveCommitResult {
            reserved: amount,
            committed: commit_amount,
        }
    }

    fn reserve_and_drop(&self, resource_name: String, expected_use: u64, amount: u64) -> u64 {
        let token = QuotaToken::new(&resource_name, expected_use);
        token.reserve(amount).expect("reservation should succeed");
        amount
    }

    fn try_reserve(&self, resource_name: String, expected_use: u64, amount: u64) -> bool {
        let token = QuotaToken::new(&resource_name, expected_use);
        token.reserve(amount).is_ok()
    }

    fn split_and_reserve(
        &self,
        resource_name: String,
        expected_use: u64,
        child_expected_use: u64,
        amount: u64,
    ) -> SplitResult {
        let mut token = QuotaToken::new(&resource_name, expected_use);
        let child = token.split(child_expected_use);

        let parent_reservation = token
            .reserve(amount)
            .expect("parent reservation should succeed");
        let child_reservation = child
            .reserve(amount)
            .expect("child reservation should succeed");

        parent_reservation.commit(amount);
        child_reservation.commit(amount);

        SplitResult {
            parent_reserved: amount,
            child_reserved: amount,
        }
    }

    fn merge_and_reserve(
        &self,
        resource_name: String,
        expected_use_a: u64,
        expected_use_b: u64,
        amount: u64,
    ) -> u64 {
        let mut token_a = QuotaToken::new(&resource_name, expected_use_a);
        let token_b = QuotaToken::new(&resource_name, expected_use_b);
        token_a.merge(token_b);
        let reservation = token_a
            .reserve(amount)
            .expect("merged reservation should succeed");
        reservation.commit(amount);
        amount
    }

    fn acquire_and_drop(&self, resource_name: String, expected_use: u64) {
        let _token = QuotaToken::new(&resource_name, expected_use);
        // _token is dropped here; lease reference count decrements
    }

    fn reserve_in_loop(
        &self,
        resource_name: String,
        expected_use: u64,
        host: String,
        port: u16,
        max_iterations: u64,
    ) -> u64 {
        let token = QuotaToken::new(&resource_name, expected_use);
        let client = Client::builder().build().unwrap();
        let mut calls_made = 0u64;

        for _ in 0..max_iterations {
            match token.reserve(1) {
                Err(_) => break,
                Ok(reservation) => {
                    let _resp = client
                        .get(&format!("http://{host}:{port}/call"))
                        .send()
                        .unwrap();
                    reservation.commit(1);
                    calls_made += 1;
                }
            }
        }

        calls_made
    }

    fn try_reserve_and_commit(&self, resource_name: String, expected_use: u64, amount: u64) -> u64 {
        let token = QuotaToken::new(&resource_name, expected_use);
        match token.reserve(amount) {
            Err(_) => 0,
            Ok(reservation) => {
                reservation.commit(amount);
                amount
            }
        }
    }
}
