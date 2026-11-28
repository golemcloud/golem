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

//! The shared permission-card scenario for the MoonBit SDK, run against the
//! `MoonBitPermissionAgent` of the MoonBit tool-streaming test component.

use super::*;
use golem_worker_executor_test_utils::{EmulatedWorkerService, WorkerServiceOptions};

use test_r::{test, timeout};

const AGENT_TYPE: &str = "MoonBitPermissionAgent";

pub(super) async fn start_local_card_transfer_executor(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    authority: Arc<ScopeCardAuthority>,
) -> anyhow::Result<TestWorkerExecutor> {
    // The executor keeps the service alive: it holds the overrides it started with.
    let worker_service = EmulatedWorkerService::start(WorkerServiceOptions::default()).await?;
    start_with_overrides(
        deps,
        context,
        TestExecutorOverrides {
            create_card_service: Some(Arc::new(move || {
                Arc::new(ScopeCardService {
                    authority: authority.clone(),
                })
            })),
            ..Default::default()
        }
        .with_worker_service(&worker_service),
    )
    .await
}

/// Starts an executor with the scope-card authority and local card-transfer
/// delivery, and stores the MoonBit component with card authority for the
/// permission agent.
async fn start_moonbit_permission_executor(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    authority: Arc<ScopeCardAuthority>,
    component: &PrecompiledComponent,
) -> anyhow::Result<(
    TestWorkerExecutor,
    golem_common::model::component::ComponentDto,
)> {
    let executor = start_local_card_transfer_executor(deps, context, authority).await?;
    let component = executor
        .component_dep(&context.default_environment_id, component)
        .update_agent_provision_config(AGENT_TYPE, |config| {
            config
                .initial_permissions
                .lower_bound
                .positive
                .extend(scope_card_initial_permissions());
            config.initial_permissions.lower_bound.positive.extend(
                [
                    "card(*) @ * : install : *",
                    "card(*) @ * : revoke : *",
                    "agent(?agent) @ * : view : *",
                ]
                .map(|pattern| parse_polymorphic_permission(pattern).unwrap()),
            );
        })
        .store()
        .await?;
    Ok((executor, component))
}

async fn invoke(
    executor: &TestWorkerExecutor,
    component: &golem_common::model::component::ComponentDto,
    agent: &golem_common::model::agent::ParsedAgentKey,
    method: &str,
    params: golem_common::schema::TypedSchemaValue,
) -> anyhow::Result<golem_test_framework::dsl::AgentResult> {
    executor
        .invoke_and_await_agent(component, agent, method, params)
        .await
}

#[test]
#[timeout("4m")]
#[tracing::instrument]
async fn moonbit_sdk_derives_inspects_and_rejects_permission_cards(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_moonbit")] tool_streaming_moonbit: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let authority = Arc::new(ScopeCardAuthority::default());
    let (executor, component) = start_moonbit_permission_executor(
        deps,
        &context,
        authority.clone(),
        tool_streaming_moonbit,
    )
    .await?;
    let caller = agent_key!(AGENT_TYPE, "derive-caller");
    let target = agent_key!(AGENT_TYPE, "derive-target");
    configure_scope_card_root(&authority, &component, &caller)?;
    executor.start_agent(&component.id, caller.clone()).await?;
    let target_id = executor.start_agent(&component.id, target.clone()).await?;

    let observations = invoke(
        &executor,
        &component,
        &caller,
        "derive_chain",
        data_value!(),
    )
    .await?
    .into_typed::<Result<Vec<String>, String>>()?
    .map_err(|error| anyhow::anyhow!("derive_chain failed: {error}"))?;
    assert_eq!(
        observations,
        vec![
            "wallet-card-inspect-grants=1",
            "wallet-card-other-grants=0",
            "wallet-card-expires=false",
            "child-parent-is-card=true",
            "child-id-differs=true",
            "child-expires=true",
            "child-inspect-grants=1",
            "open-scope-expires=false",
            "derived-scope-parent=not-permitted",
            "root-child-parent-is-root=true",
            "root-child-expires=true",
            "calendar-lifespan=invalid-pattern",
            "zero-lifespan=invalid-pattern",
        ]
    );

    let negatives = invoke(
        &executor,
        &component,
        &caller,
        "negative_cases",
        data_value!(target_id.agent_id.to_string()),
    )
    .await?
    .into_typed::<Result<Vec<String>, String>>()?
    .map_err(|error| anyhow::anyhow!("negative_cases failed: {error}"))?;
    assert_eq!(
        negatives,
        vec![
            "recipient-mismatch=install-recipient-mismatch",
            "mismatched-card-moved=true",
            "account-target=not-permitted",
            "invalid-verb=invalid-verb",
        ]
    );

    // The same read succeeds while the card is held, so the abort below comes
    // from the move and not from a missing root.
    let held = invoke(
        &executor,
        &component,
        &caller,
        "use_after_move",
        data_value!(false),
    )
    .await?
    .into_typed::<String>()?;
    assert_eq!(held, "root-present");
    let error = invoke(
        &executor,
        &component,
        &caller,
        "use_after_move",
        data_value!(true),
    )
    .await
    .expect_err("using a moved card must abort the invocation");
    // MoonBit's `abort` traps with `unreachable`; its message is not part of
    // the invocation error.
    assert!(
        format!("{error:#}").contains("wasm `unreachable` instruction executed"),
        "unexpected moved-card failure: {error:#}"
    );
    Ok(())
}

#[test]
#[timeout("4m")]
#[tracing::instrument]
async fn moonbit_sdk_lends_scope_cards_installs_and_revokes(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_moonbit")] tool_streaming_moonbit: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let authority = Arc::new(ScopeCardAuthority::default());
    let (executor, component) = start_moonbit_permission_executor(
        deps,
        &context,
        authority.clone(),
        tool_streaming_moonbit,
    )
    .await?;
    let caller = agent_key!(AGENT_TYPE, "scope-caller");
    let scope_target_name = "scope-target";
    let scope_target = agent_key!(AGENT_TYPE, scope_target_name);
    let install_target = agent_key!(AGENT_TYPE, "install-target");
    configure_scope_card_root(&authority, &component, &caller)?;
    let caller_id = executor.start_agent(&component.id, caller.clone()).await?;
    executor
        .start_agent(&component.id, scope_target.clone())
        .await?;
    let install_target_id = executor
        .start_agent(&component.id, install_target.clone())
        .await?;

    // A requested grant must be concrete, so the caller gets its own owner
    // path. The test executor stores every component under the application
    // `test-app` and the environment `test-env`; the agent segment is the
    // agent ID.
    let caller_owner = format!(
        "{}/test-app/test-env/{}/{}",
        executor.account_email().as_str(),
        component.component_name.0,
        caller_id.agent_id
    );

    // Step 4: the callee sees the scope card only during awaited calls that
    // attach it, and loses it when the invocation ends. Only the scope card
    // lets the callee read the caller's oplog. The inspected `?agent`
    // template grant is not a valid request.
    let scope = invoke(
        &executor,
        &component,
        &caller,
        "scope_rpc",
        data_value!(scope_target_name, caller_owner.clone()),
    )
    .await?
    .into_typed::<Result<Vec<String>, String>>()?
    .map_err(|error| anyhow::anyhow!("scope_rpc failed: {error}"))?;
    assert_eq!(
        scope,
        vec![
            "template-grant=invalid-pattern",
            "per-call=present/true/2/readable",
            "default=present/true/2/readable",
            "without=absent/denied",
            "scope-kept=true",
        ]
    );
    let scope_target_cards = invoke(
        &executor,
        &component,
        &scope_target,
        "wallet_card_count",
        data_value!(),
    )
    .await?
    .into_typed::<u32>()?;
    assert_eq!(
        scope_target_cards, 1,
        "the scope card must not survive the call"
    );

    // Step 5: installing a card adds it to the target's wallet, and with it
    // the authority to read the caller's oplog.
    let caller_name = caller_id.agent_id.to_string();
    let can_read_caller_oplog = async || -> anyhow::Result<bool> {
        invoke(
            &executor,
            &component,
            &install_target,
            "can_read_oplog",
            data_value!(caller_name.clone()),
        )
        .await?
        .into_typed::<bool>()
    };
    let wallet_count = async || -> anyhow::Result<u32> {
        invoke(
            &executor,
            &component,
            &install_target,
            "wallet_card_count",
            data_value!(),
        )
        .await?
        .into_typed::<u32>()
    };
    let before = wallet_count().await?;
    assert!(
        !can_read_caller_oplog().await?,
        "the target must not read the caller's oplog yet"
    );
    let moved = invoke(
        &executor,
        &component,
        &caller,
        "install_on",
        data_value!(install_target_id.agent_id.to_string(), caller_owner.clone()),
    )
    .await?
    .into_typed::<Result<bool, String>>()?
    .map_err(|error| anyhow::anyhow!("install_on failed: {error}"))?;
    assert!(moved, "install must move the card");
    assert_eq!(
        wallet_count().await?,
        before + 1,
        "the installed card must reach the target"
    );
    assert!(
        can_read_caller_oplog().await?,
        "the installed card must let the target read the caller's oplog"
    );

    // Step 6: revoking the installed card removes it and its authority from
    // the target.
    let revoked_installed = invoke(
        &executor,
        &component,
        &install_target,
        "revoke_installed_card",
        data_value!(),
    )
    .await?
    .into_typed::<Result<Vec<String>, String>>()?
    .map_err(|error| anyhow::anyhow!("revoke_installed_card failed: {error}"))?;
    assert_eq!(revoked_installed, vec!["revoked=1", "card-moved=true"]);
    assert_eq!(
        wallet_count().await?,
        before,
        "the revoked card must leave the wallet"
    );
    assert!(
        !can_read_caller_oplog().await?,
        "the target must lose the authority of the revoked card"
    );

    // Revoking the wallet root returns the revoked-card count.
    let revoke_calls = authority.revoke_card_count();
    let revoked = invoke(&executor, &component, &caller, "revoke_root", data_value!())
        .await?
        .into_typed::<Result<u32, String>>()?
        .map_err(|error| anyhow::anyhow!("revoke_root failed: {error}"))?;
    assert_eq!(revoked, 1);
    assert_eq!(authority.revoke_card_count(), revoke_calls + 1);
    Ok(())
}

/// `derive-from-wallet` reports a request the wallet does not cover, and a
/// wallet that lost its derive authority, as permission errors.
#[test]
#[timeout("4m")]
#[tracing::instrument]
async fn moonbit_sdk_derive_from_wallet_reports_permission_errors(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_moonbit")] tool_streaming_moonbit: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let authority = Arc::new(ScopeCardAuthority::default());
    let (executor, component) = start_moonbit_permission_executor(
        deps,
        &context,
        authority.clone(),
        tool_streaming_moonbit,
    )
    .await?;
    let caller = agent_key!(AGENT_TYPE, "wallet-errors-caller");
    configure_scope_card_root(&authority, &component, &caller)?;
    executor.start_agent(&component.id, caller.clone()).await?;

    let beyond_wallet = invoke(
        &executor,
        &component,
        &caller,
        "derive_beyond_wallet",
        data_value!(),
    )
    .await?
    .into_typed::<String>()?;
    assert_eq!(beyond_wallet, "lower-bound-too-broad");

    assert_eq!(
        invoke(
            &executor,
            &component,
            &caller,
            "derive_from_wallet_kind",
            data_value!(),
        )
        .await?
        .into_typed::<String>()?,
        "derived"
    );
    let revoked = invoke(&executor, &component, &caller, "revoke_root", data_value!())
        .await?
        .into_typed::<Result<u32, String>>()?
        .map_err(|error| anyhow::anyhow!("revoke_root failed: {error}"))?;
    assert_eq!(
        revoked, 2,
        "revoking the root also revokes the card derived from the wallet"
    );
    let after_revoke = invoke(
        &executor,
        &component,
        &caller,
        "derive_from_wallet_kind",
        data_value!(),
    )
    .await?
    .into_typed::<String>()?;
    assert_eq!(after_revoke, "not-permitted");
    Ok(())
}
