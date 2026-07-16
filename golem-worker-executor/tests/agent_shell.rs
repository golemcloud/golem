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

//! Integration tests for the `golem agent shell` wire contract, driven against a real SDK-built
//! component (`test-components/agent-shell`).
//!
//! The CLI decodes each result POSITIONALLY into its own `EvalResult` mirror, so these tests decode
//! through structs declared in the same field order the CLI uses — proving an SDK-built agent's
//! `eval-result` record and the CLI's expectations cannot drift apart without a failure here. The
//! pause protocol (question surfaces in `pending-prompt`, answer/abort arrive on a SEPARATE
//! invocation because invocations are serialized per instance) is exercised end to end.

use crate::Tracing;

use golem_common::model::agent::AgentMode;
use golem_common::schema::agent::InputSchema;
use golem_common::schema::{FromSchema, SchemaType};
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, WorkerExecutorTestDependencies, start,
};
use pretty_assertions::assert_eq;
use test_r::{inherit_test_dep, test};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(
    #[tagged_as("agent_shell")]
    PrecompiledComponent
);
inherit_test_dep!(Tracing);

/// The CLI's `EvalResult` mirror — field order is the positional wire contract the CLI relies on
/// (`stdout`, `stderr`, `exit_code`, `pending_prompt`). If an SDK change breaks positional record
/// encoding, or the reference component's declaration drifts, decoding here fails the same way
/// `golem agent shell` would.
#[derive(Debug, FromSchema)]
struct EvalResult {
    stdout: String,
    stderr: String,
    exit_code: u8,
    pending_prompt: Option<PendingPrompt>,
}

/// The CLI's `PendingPrompt` mirror — positional order `question`, `choices`.
#[derive(Debug, FromSchema)]
struct PendingPrompt {
    question: String,
    choices: Option<Vec<String>>,
}

/// The reflected agent-type schema satisfies exactly what `golem agent shell` validates on
/// connect: a durable agent exposing `eval(string)`, `answer_prompt(string)` and `abort_prompt()`.
#[test]
#[tracing::instrument]
async fn agent_shell_surface_is_reflected_as_the_cli_expects(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_shell")] agent_shell: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_shell)
        .store()
        .await?;

    let agent_types = component.metadata.agent_types();
    let surface = agent_types
        .iter()
        .find(|t| t.type_name.0 == "ShellSurface")
        .expect("ShellSurface agent type is reflected");

    assert_eq!(surface.mode, AgentMode::Durable);

    let single_string = |name: &str| {
        let method = surface
            .methods
            .iter()
            .find(|m| m.name == name)
            .unwrap_or_else(|| panic!("method `{name}` is reflected"));
        let InputSchema::Parameters(params) = &method.input_schema;
        assert_eq!(params.len(), 1, "`{name}` takes one parameter");
        assert!(
            matches!(params[0].schema, SchemaType::String { .. }),
            "`{name}`'s parameter is a string"
        );
    };
    single_string("eval");
    single_string("answer_prompt");

    let abort = surface
        .methods
        .iter()
        .find(|m| m.name == "abort_prompt")
        .expect("method `abort_prompt` is reflected");
    let InputSchema::Parameters(params) = &abort.input_schema;
    assert!(params.is_empty(), "`abort_prompt` takes no parameters");

    Ok(())
}

/// The full session: plain output, durable per-instance state, failure exit codes, and the pause
/// protocol round-trip (surface → eval-while-pending rejection with re-surfaced prompt → invalid
/// answer rejection → valid answer; then free-form question → abort at exit 130).
#[test]
#[tracing::instrument]
async fn agent_shell_surface_round_trips_the_pause_protocol(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_shell")] agent_shell: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_shell)
        .store()
        .await?;
    let agent_id = agent_id!("ShellSurface", "shell-session");
    executor
        .start_agent(&component.id, agent_id.clone())
        .await?;

    let eval = async |line: &str| -> anyhow::Result<EvalResult> {
        executor
            .invoke_and_await_agent(&component, &agent_id, "eval", data_value!(line))
            .await?
            .into_typed()
    };

    // Plain output.
    let result = eval("echo hello").await?;
    assert_eq!(result.stdout, "hello\n");
    assert_eq!(result.exit_code, 0);
    assert!(result.pending_prompt.is_none());

    // Durable per-instance state accumulates across invocations — a session, not fresh instances.
    assert_eq!(eval("count").await?.stdout, "1\n");
    assert_eq!(eval("count").await?.stdout, "2\n");

    // Failure exit codes propagate as results, not protocol errors.
    let result = eval("fail").await?;
    assert_eq!(result.exit_code, 1);
    assert_eq!(result.stderr, "failed\n");

    // A question surfaces in pending_prompt; the invocation RETURNS (never blocks on a human).
    let result = eval("ask-choice").await?;
    assert_eq!(result.exit_code, 0);
    let prompt = result.pending_prompt.expect("question surfaced");
    assert_eq!(prompt.question, "Proceed?");
    assert_eq!(
        prompt.choices.as_deref(),
        Some(&["yes".to_string(), "no".to_string()][..])
    );

    // While pending, eval is rejected AND the prompt re-surfaces — how a fresh client attaching
    // mid-question (e.g. after a died session) discovers and recovers it.
    let result = eval("echo blocked").await?;
    assert_eq!(result.exit_code, 1);
    let resurfaced = result
        .pending_prompt
        .expect("prompt re-surfaces on rejection");
    assert_eq!(resurfaced.question, "Proceed?");

    // An answer outside the allowed choices is rejected; the question stays pending.
    let result: EvalResult = executor
        .invoke_and_await_agent(&component, &agent_id, "answer_prompt", data_value!("maybe"))
        .await?
        .into_typed()?;
    assert_eq!(result.exit_code, 1);
    assert!(result.pending_prompt.is_some(), "question still pending");

    // A valid answer resolves it — on a separate invocation, per the serialized-invocation model.
    let result: EvalResult = executor
        .invoke_and_await_agent(&component, &agent_id, "answer_prompt", data_value!("yes"))
        .await?
        .into_typed()?;
    assert_eq!(result.exit_code, 0);
    assert_eq!(result.stdout, "answered: yes\n");
    assert!(result.pending_prompt.is_none());

    // Free-form question → abort follows the Ctrl-C convention (exit 130) and clears the pause.
    let result = eval("ask-free").await?;
    assert!(
        result
            .pending_prompt
            .expect("question surfaced")
            .choices
            .is_none()
    );
    let result: EvalResult = executor
        .invoke_and_await_agent(&component, &agent_id, "abort_prompt", data_value!())
        .await?
        .into_typed()?;
    assert_eq!(result.exit_code, 130);
    assert!(result.pending_prompt.is_none());

    // The session is usable again after the abort.
    assert_eq!(eval("count").await?.stdout, "3\n");

    Ok(())
}
