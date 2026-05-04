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

//! Integration tests verifying that cross-account (shared-environment) RPC calls produce the correct
//! `RpcCallOutcome` variant (typed, not a panic or internal error).
//!
//! The `RpcAuthTester` agent wraps an RPC call to `RpcCounter` and returns a typed
//! `RpcCallOutcome` variant instead of panicking, allowing tests to assert on the
//! exact error case without string matching.
//!
//! ## Coverage note
//!
//! These tests currently cover same-target-environment sharing semantics (different accounts,
//! one environment owned by the target account and optionally shared to the caller account).
//! Auth decisions in this flow are caller-account + target-environment + action based.
//! They do not cover source-env-A -> target-env-B RPC from a single guest invocation.
//! This is a current technical limitation of `WasmRpc`: the constructor resolves by agent type
//! and constructor args in the caller context and does not expose an explicit target environment id.
//!
//! Each test run exercises whichever executor path the shard manager assigns for the
//! target worker — either local (same executor as the caller) or remote (different
//! executor). The test framework does not expose shard-pinning, so both paths cannot be
//! guaranteed in a single run. The local-path auth logic is independently covered by
//! unit tests in `golem-worker-executor/src/services/direct_invocation_auth.rs`.
//!
//! ## `RpcCallOutcome` variant layout (must match the Rust enum in test-components)
//!
//! ```text
//! case_idx 0 => Ok
//! case_idx 1 => Denied   { details: String }
//! case_idx 2 => NotFound { details: String }
//! case_idx 3 => ProtocolError { details: String }
//! case_idx 4 => RemoteInternalError { details: String }
//! ```

use golem_common::model::auth::EnvironmentRole;
use golem_common::{agent_id, data_value};
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
use golem_wasm::Value;
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(EnvBasedTestDependencies);

// The case index of `RpcCallOutcome::Ok`.
const OK_CASE_IDX: u32 = 0;

/// Store the `golem-it:agent-rpc-rust` component under `user`'s environment
/// and return the component DTO.
async fn store_rpc_component(
    user: &impl TestDsl,
    env_id: &golem_common::model::environment::EnvironmentId,
) -> anyhow::Result<golem_client::model::ComponentDto> {
    user.component(env_id, "golem_it_agent_rpc_rust_release")
        .name("golem-it:agent-rpc-rust")
        .unique()
        .store()
        .await
}

/// Assert that a `DataValue` result from `RpcAuthTester.try_call_counter` is `Ok`.
fn assert_rpc_outcome_is_ok(result: golem_common::model::agent::DataValue) {
    let value = result.into_return_value().expect("Expected a return value");
    match value {
        Value::Variant { case_idx, .. } => {
            assert_eq!(
                case_idx, OK_CASE_IDX,
                "Expected RpcCallOutcome::Ok (case_idx={}), got case_idx={}",
                OK_CASE_IDX, case_idx
            );
        }
        other => {
            panic!("Expected Value::Variant for RpcCallOutcome, got: {other:?}");
        }
    }
}

/// An authorized caller (granted `Deployer` role) invoking a worker in another account-owned
/// environment succeeds — `RpcCallOutcome::Ok` is returned.
///
/// The two targets are created by different accounts in the same environment:
/// - environment owner account
/// - shared grantee account (caller)
///
#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn authorized_cross_account_rpc_via_share_succeeds(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let owner = deps.user().await?;
    let caller = deps.user().await?;

    let (_, owner_env) = owner.app_and_env().await?;
    let owner_component = store_rpc_component(&owner, &owner_env.id).await?;

    let (_, caller_env) = caller.app_and_env().await?;
    let caller_component = store_rpc_component(&caller, &caller_env.id).await?;

    // Grant caller Deployer access to owner's environment.
    owner
        .share_environment(
            &owner_env.id,
            &caller.account_id,
            &[EnvironmentRole::Deployer],
        )
        .await?;

    let env_owner_created_counter_name = "auth-test-allowed-counter-env-owner";
    let env_owner_created_counter_agent_id =
        agent_id!("RpcCounter", env_owner_created_counter_name);
    owner
        .start_agent(
            &owner_component.id,
            env_owner_created_counter_agent_id.clone(),
        )
        .await?;

    let shared_grantee_created_counter_name = "auth-test-allowed-counter-shared-grantee";
    let shared_grantee_created_counter_agent_id =
        agent_id!("RpcCounter", shared_grantee_created_counter_name);
    caller
        .start_agent(
            &owner_component.id,
            shared_grantee_created_counter_agent_id.clone(),
        )
        .await?;

    let tester_agent_id = agent_id!("RpcAuthTester", "tester-allowed");
    caller
        .start_agent(&caller_component.id, tester_agent_id.clone())
        .await?;

    let env_owner_created_target_result = caller
        .invoke_and_await_agent(
            &caller_component,
            &tester_agent_id,
            "try_call_counter",
            data_value!(env_owner_created_counter_name.to_string()),
        )
        .await?;

    let shared_grantee_created_target_result = caller
        .invoke_and_await_agent(
            &caller_component,
            &tester_agent_id,
            "try_call_counter",
            data_value!(shared_grantee_created_counter_name.to_string()),
        )
        .await?;

    assert_rpc_outcome_is_ok(env_owner_created_target_result);
    assert_rpc_outcome_is_ok(shared_grantee_created_target_result);

    Ok(())
}

/// An owner calling their own worker (same account) via RPC always succeeds — fast path.
#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn owner_calling_own_worker_via_rpc_succeeds(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let owner = deps.user().await?;

    let (_, env) = owner.app_and_env().await?;
    let component = store_rpc_component(&owner, &env.id).await?;

    let counter_name = "auth-test-own-counter";
    let counter_agent_id = agent_id!("RpcCounter", counter_name);
    owner
        .start_agent(&component.id, counter_agent_id.clone())
        .await?;

    let tester_agent_id = agent_id!("RpcAuthTester", "tester-own");
    owner
        .start_agent(&component.id, tester_agent_id.clone())
        .await?;

    let result = owner
        .invoke_and_await_agent(
            &component,
            &tester_agent_id,
            "try_call_counter",
            data_value!(counter_name.to_string()),
        )
        .await?;

    assert_rpc_outcome_is_ok(result);

    Ok(())
}
