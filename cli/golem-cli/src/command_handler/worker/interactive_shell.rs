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

//! `golem agent shell` — drive an agent that presents a *shell surface* as an interactive REPL.
//!
//! The sibling of `agent stream` rather than a mode of it: `stream` tails the agent's output channels
//! one-way, whereas this invokes the agent and renders what comes back.
//!
//! Nothing here is specific to any one agent: the surface is validated structurally, the line is
//! shipped verbatim, and the prompt label derives from the agent type ([`shell_prompt_label`] —
//! `ClankAgent` shows `clank$`, `RandomAgent` shows `random$`).
//!
//! **The contract.** `agent shell` requires a **durable** agent type exposing three methods, named
//! exactly as written (snake_case is normative — the surface is matched against the reflected method
//! names verbatim):
//!
//! | Method | Signature | Role |
//! |---|---|---|
//! | `eval` | `(string) -> eval-result` | run one command line, return its output |
//! | `answer_prompt` | `(string) -> eval-result` | deliver a human answer to an outstanding question |
//! | `abort_prompt` | `() -> eval-result` | cancel an outstanding question |
//!
//! where `eval-result` is the record `{ stdout: string, stderr: string, exit-code: u8,
//! pending-prompt: option<{ question: string, choices: option<list<string>> }> }`.
//!
//! An agent that does not present that surface gets a clear error, and **every agent is otherwise
//! unaffected** — `stream` log-streams exactly as before. Ephemeral agent types are rejected up
//! front: each invocation runs on a fresh instance, so session state could not persist between
//! lines and a pending question could never be answered.
//!
//! **Why the pause is a separate call.** Such an agent never blocks waiting on a human: a question is
//! recorded as durable state and returned *immediately* in `pending-prompt`; the answer arrives on a
//! **separate invocation**. That shape is forced by the runtime — agent invocations are serialized per
//! instance, so an agent parked on a human would be unreachable. It also means a prompt left dangling
//! by a previous session is picked up and resolved by the next one, which this loop does.

use anyhow::{anyhow, bail};
use golem_client::api::AgentClient;
use golem_client::model::{AgentInvocationMode, AgentInvocationRequest};
use golem_common::model::IdempotencyKey;
use golem_common::model::agent::AgentMode;
use golem_common::model::agent::ParsedAgentId;
use golem_common::schema::agent::{AgentMethodSchema, AgentTypeSchema, InputSchema};
use golem_common::schema::{FromSchema, SchemaType, SchemaValue};
use inquire::{InquireError, Select, Text};

use super::WorkerCommandHandler;
use crate::error::service::MapServiceError;
use crate::log::{LogColorize, log_action, log_error, log_preformatted, logln};
use crate::model::text::worker::format_agent_name_match;
use crate::model::worker::AgentNameMatch;

/// Run one command line and return its output.
const EVAL: &str = "eval";
/// Deliver a human answer to an outstanding question raised by [`EVAL`].
const ANSWER_PROMPT: &str = "answer_prompt";
/// Cancel an outstanding question raised by [`EVAL`].
const ABORT_PROMPT: &str = "abort_prompt";
/// Typed in at the answer prompt to cancel the outstanding question instead of answering it.
const ABORT_ANSWER: &str = ":abort";

/// The record every interactive-shell method returns.
#[derive(Debug, FromSchema)]
struct EvalResult {
    stdout: String,
    stderr: String,
    exit_code: u8,
    /// `Some` when the command paused on a question the caller must answer (via [`ANSWER_PROMPT`])
    /// before any further [`EVAL`] will run.
    pending_prompt: Option<PendingPrompt>,
}

/// A question the agent is waiting on. Field order is load-bearing — see [`EvalResult`].
#[derive(Debug, FromSchema)]
struct PendingPrompt {
    question: String,
    /// When present, the answer must be one of these.
    choices: Option<Vec<String>>,
}

/// Whether `agent_type` presents the interactive-shell surface, and why not if it doesn't.
pub fn validate_interactive_surface(agent_type: &AgentTypeSchema) -> anyhow::Result<()> {
    if agent_type.mode == AgentMode::Ephemeral {
        bail!(
            "Agent type {} cannot be used with `agent shell`: it is ephemeral, so each invocation \
             runs on a fresh instance — shell state would reset between lines and a pending \
             question could never be answered.",
            agent_type.type_name.0.log_color_highlight(),
        );
    }

    let find = |name: &str| agent_type.methods.iter().find(|m| m.name == name);

    let missing: Vec<&str> = [EVAL, ANSWER_PROMPT, ABORT_PROMPT]
        .into_iter()
        .filter(|m| find(m).is_none())
        .collect();

    if !missing.is_empty() {
        bail!(
            "Agent type {} cannot be used with `agent shell`: it has no {} method(s).\n\
             `agent shell` requires an agent exposing `{EVAL}(string)`, `{ANSWER_PROMPT}(string)` and \
             `{ABORT_PROMPT}()` (snake_case, matched verbatim), each returning an eval-result record \
             {{stdout, stderr, exit-code, pending-prompt}}.",
            agent_type.type_name.0.log_color_highlight(),
            missing.join(", ").log_color_error_highlight(),
        );
    }

    single_string_parameter(agent_type, find(EVAL).unwrap())?;
    single_string_parameter(agent_type, find(ANSWER_PROMPT).unwrap())?;
    zero_parameters(agent_type, find(ABORT_PROMPT).unwrap())?;

    Ok(())
}

/// Assert `method` takes exactly one `string` parameter, naming the actual mismatch (count vs type).
fn single_string_parameter(
    agent_type: &AgentTypeSchema,
    method: &AgentMethodSchema,
) -> anyhow::Result<()> {
    let InputSchema::Parameters(params) = &method.input_schema;
    match params.as_slice() {
        [only] if matches!(only.schema, SchemaType::String { .. }) => Ok(()),
        [only] => bail!(
            "Agent type {} cannot be used with `agent shell`: `{}` must take one string parameter, \
             but its parameter `{}` is not a string.",
            agent_type.type_name.0.log_color_highlight(),
            method.name,
            only.name,
        ),
        _ => bail!(
            "Agent type {} cannot be used with `agent shell`: `{}` must take exactly one string \
             parameter, but it takes {} parameters.",
            agent_type.type_name.0.log_color_highlight(),
            method.name,
            params.len(),
        ),
    }
}

/// Assert `method` takes no parameters (the CLI invokes it with an empty parameter record).
fn zero_parameters(agent_type: &AgentTypeSchema, method: &AgentMethodSchema) -> anyhow::Result<()> {
    let InputSchema::Parameters(params) = &method.input_schema;
    if params.is_empty() {
        Ok(())
    } else {
        bail!(
            "Agent type {} cannot be used with `agent shell`: `{}` must take no parameters, but it \
             takes {}.",
            agent_type.type_name.0.log_color_highlight(),
            method.name,
            params.len(),
        )
    }
}

/// Render an [`EvalResult`]'s output as it should appear in the terminal.
fn render(result: &EvalResult) -> String {
    let mut out = String::new();
    out.push_str(&result.stdout);
    if !result.stdout.is_empty() && !result.stdout.ends_with('\n') {
        out.push('\n');
    }
    if !result.stderr.is_empty() {
        out.push_str(&result.stderr);
        if !result.stderr.ends_with('\n') {
            out.push('\n');
        }
    }
    if result.exit_code != 0 {
        out.push_str(&format!("exit {}\n", result.exit_code));
    }
    out
}

/// The shell's prompt label, derived from the agent type so the surface reads as the agent's own:
/// kebab-case the type name, drop a trailing `-agent` (the conventional suffix carries no
/// information at a shell prompt), and append `$`. `ClankAgent` → `clank$`, `GreeterAgent` →
/// `greeter$`, `RpcCounter` → `rpc-counter$`. A type named literally `Agent` (or an empty name)
/// falls back to `agent$`.
fn shell_prompt_label(type_name: &str) -> String {
    let kebab = heck::ToKebabCase::to_kebab_case(type_name);
    let stem = kebab.strip_suffix("-agent").unwrap_or(&kebab);
    if stem.is_empty() {
        "agent$".to_string()
    } else {
        format!("{stem}$")
    }
}

/// Format a pending question for display, appending its allowed answers when it has them.
fn format_question(prompt: &PendingPrompt) -> String {
    match &prompt.choices {
        Some(choices) if !choices.is_empty() => {
            format!("{} [{}]", prompt.question, choices.join(", "))
        }
        _ => prompt.question.clone(),
    }
}

impl WorkerCommandHandler {
    /// Drive the interactive shell: read a line, invoke `eval`, render, resolve any pause, repeat.
    /// Runs until EOF (Ctrl-D), Ctrl-C, or `exit`.
    pub(super) async fn run_interactive_shell(
        &self,
        agent_name_match: &AgentNameMatch,
        agent_type: &AgentTypeSchema,
        agent_id: &ParsedAgentId,
    ) -> anyhow::Result<()> {
        log_action(
            "Connecting",
            format!(
                "to agent {} (Ctrl-D, Esc, or `exit` to leave)",
                format_agent_name_match(agent_name_match)
            ),
        );
        logln("");

        let prompt = shell_prompt_label(&agent_type.type_name.0);
        loop {
            let line = match Text::new(&prompt).prompt() {
                Ok(line) => line,
                // Ctrl-D / Esc / Ctrl-C are ordinary ways to leave a shell, not errors.
                Err(InquireError::OperationCanceled | InquireError::OperationInterrupted) => break,
                Err(err) => return Err(err.into()),
            };

            let line = line.trim();
            if line == "exit" {
                break;
            }
            if line.is_empty() {
                continue;
            }

            let result = match self
                .interactive_invoke(
                    agent_name_match,
                    agent_type,
                    agent_id,
                    EVAL,
                    Some(line.to_string()),
                )
                .await
            {
                Ok(result) => result,
                Err(err) => {
                    log_error(format!("{err:#}"));
                    continue;
                }
            };

            self.render_and_resolve(agent_name_match, agent_type, agent_id, result)
                .await;
        }

        Ok(())
    }

    /// Invoke one method with an optional single string argument, and decode its [`EvalResult`].
    async fn interactive_invoke(
        &self,
        agent_name_match: &AgentNameMatch,
        agent_type: &AgentTypeSchema,
        agent_id: &ParsedAgentId,
        method: &str,
        argument: Option<String>,
    ) -> anyhow::Result<EvalResult> {
        let method_parameters = SchemaValue::Record {
            fields: argument.into_iter().map(SchemaValue::String).collect(),
        };

        let environment = &agent_name_match.environment;
        let idempotency_key = IdempotencyKey::fresh();
        let request = AgentInvocationRequest {
            app_name: environment.application_name.to_string(),
            env_name: environment.environment_name.to_string(),
            agent_type_name: agent_id.agent_type.0.clone(),
            parameters: serde_json::to_value(agent_id.parameters.value())?,
            phantom_id: agent_id.phantom_id,
            method_name: method.to_string(),
            method_parameters: serde_json::to_value(method_parameters)?,
            mode: AgentInvocationMode::Await,
            schedule_at: None,
            idempotency_key: Some(idempotency_key.value.clone()),
            deployment_revision: None,
            owner_account_email: None,
        };

        let clients = self.ctx.golem_clients().await?;
        let result = clients
            .agent
            .invoke_agent(Some(&idempotency_key.value), &request)
            .await
            .map_service_error()?;

        let typed = result.result.ok_or_else(|| {
            anyhow!(
                "Agent type {} returned no value from `{method}`; `agent shell` requires it to return \
                 an eval-result record.",
                agent_type.type_name.0
            )
        })?;

        EvalResult::from_value(typed.value()).map_err(|err| {
            anyhow!(
                "Could not decode the eval-result returned by `{method}` on agent type {}: {err}",
                agent_type.type_name.0
            )
        })
    }

    /// Print a result and, while it carries a pending question, ask the human and deliver the answer.
    async fn render_and_resolve(
        &self,
        agent_name_match: &AgentNameMatch,
        agent_type: &AgentTypeSchema,
        agent_id: &ParsedAgentId,
        mut result: EvalResult,
    ) {
        loop {
            let output = render(&result);
            if !output.is_empty() {
                log_preformatted(&output);
            }

            let Some(prompt) = result.pending_prompt.take() else {
                return;
            };

            let (method, argument) = match self.ask_human(&prompt) {
                Ok(Some(answer)) => (ANSWER_PROMPT, Some(answer)),
                Ok(None) => (ABORT_PROMPT, None),
                Err(err) => {
                    log_error(format!("{err:#}"));
                    return;
                }
            };

            result = match self
                .interactive_invoke(agent_name_match, agent_type, agent_id, method, argument)
                .await
            {
                Ok(result) => result,
                Err(err) => {
                    log_error(format!("{err:#}"));
                    return;
                }
            };
        }
    }

    /// Ask the human the agent's question. `Ok(None)` means "abort this question".
    fn ask_human(&self, prompt: &PendingPrompt) -> anyhow::Result<Option<String>> {
        logln(format!("⏸  {}", format_question(prompt)));

        let answer = match &prompt.choices {
            Some(choices) if !choices.is_empty() => {
                let mut options = choices.clone();
                options.push(ABORT_ANSWER.to_string());
                Select::new("answer:", options).prompt()
            }
            _ => Text::new(&format!("answer (`{ABORT_ANSWER}` to cancel):")).prompt(),
        };

        match answer {
            Ok(answer) if answer.trim() == ABORT_ANSWER => Ok(None),
            Ok(answer) => Ok(Some(answer)),
            // Ctrl-D / Esc / Ctrl-C at a question aborts the question, not the shell.
            Err(InquireError::OperationCanceled | InquireError::OperationInterrupted) => Ok(None),
            Err(err) => Err(err.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::schema::SchemaValue;
    use pretty_assertions::assert_eq;
    use test_r::test;

    /// Build the schema-value an agent would return for an `eval-result`, so the tests exercise the
    /// real decode path rather than a hand-written guess at the wire JSON.
    fn eval_result_value(
        stdout: &str,
        stderr: &str,
        exit_code: u8,
        pending: Option<SchemaValue>,
    ) -> SchemaValue {
        SchemaValue::Record {
            fields: vec![
                SchemaValue::String(stdout.to_string()),
                SchemaValue::String(stderr.to_string()),
                SchemaValue::U8(exit_code),
                SchemaValue::Option {
                    inner: pending.map(Box::new),
                },
            ],
        }
    }

    fn pending_prompt_value(question: &str, choices: Option<&[&str]>) -> SchemaValue {
        SchemaValue::Record {
            fields: vec![
                SchemaValue::String(question.to_string()),
                SchemaValue::Option {
                    inner: choices.map(|choices| {
                        Box::new(SchemaValue::List {
                            elements: choices
                                .iter()
                                .map(|c| SchemaValue::String(c.to_string()))
                                .collect(),
                        })
                    }),
                },
            ],
        }
    }

    #[test]
    fn decodes_a_plain_result() {
        let value = eval_result_value("hi\n", "", 0, None);
        let result = EvalResult::from_value(&value).expect("decode");
        assert_eq!(result.stdout, "hi\n");
        assert_eq!(result.stderr, "");
        assert_eq!(result.exit_code, 0);
        assert!(result.pending_prompt.is_none());
    }

    #[test]
    fn decodes_a_pending_prompt_with_choices() {
        let value = eval_result_value(
            "pick one\n",
            "",
            0,
            Some(pending_prompt_value("pick one", Some(&["alpha", "beta"]))),
        );
        let result = EvalResult::from_value(&value).expect("decode");
        let prompt = result.pending_prompt.expect("pending prompt");
        assert_eq!(prompt.question, "pick one");
        assert_eq!(
            prompt.choices.as_deref(),
            Some(["alpha".to_string(), "beta".to_string()].as_slice())
        );
    }

    #[test]
    fn decodes_a_pending_prompt_without_choices() {
        let value = eval_result_value("q?\n", "", 0, Some(pending_prompt_value("q?", None)));
        let result = EvalResult::from_value(&value).expect("decode");
        let prompt = result.pending_prompt.expect("pending prompt");
        assert_eq!(prompt.question, "q?");
        assert!(prompt.choices.is_none());
    }

    #[test]
    fn a_mismatched_shape_is_an_error() {
        let value = SchemaValue::Record {
            fields: vec![SchemaValue::String("only one field".to_string())],
        };
        assert!(EvalResult::from_value(&value).is_err());
    }

    #[test]
    fn render_emits_stdout_then_stderr_and_notes_a_failure() {
        let result = EvalResult {
            stdout: "out\n".to_string(),
            stderr: "err\n".to_string(),
            exit_code: 1,
            pending_prompt: None,
        };
        assert_eq!(render(&result), "out\nerr\nexit 1\n");
    }

    #[test]
    fn render_of_a_clean_result_is_just_its_stdout() {
        let result = EvalResult {
            stdout: "hi\n".to_string(),
            stderr: String::new(),
            exit_code: 0,
            pending_prompt: None,
        };
        assert_eq!(render(&result), "hi\n");
    }

    #[test]
    fn render_terminates_unterminated_output() {
        let result = EvalResult {
            stdout: "no newline".to_string(),
            stderr: String::new(),
            exit_code: 0,
            pending_prompt: None,
        };
        assert_eq!(render(&result), "no newline\n");
    }

    #[test]
    fn render_of_an_empty_success_is_empty() {
        let result = EvalResult {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            pending_prompt: None,
        };
        assert_eq!(render(&result), "");
    }

    #[test]
    fn a_question_shows_its_choices() {
        let prompt = PendingPrompt {
            question: "Deploy?".to_string(),
            choices: Some(vec!["yes".to_string(), "no".to_string()]),
        };
        assert_eq!(format_question(&prompt), "Deploy? [yes, no]");
    }

    #[test]
    fn a_free_form_question_shows_no_brackets() {
        let prompt = PendingPrompt {
            question: "your name?".to_string(),
            choices: None,
        };
        assert_eq!(format_question(&prompt), "your name?");
    }

    #[test]
    fn prompt_label_drops_the_agent_suffix() {
        assert_eq!(shell_prompt_label("ClankAgent"), "clank$");
        assert_eq!(shell_prompt_label("GreeterAgent"), "greeter$");
    }

    #[test]
    fn prompt_label_kebab_cases_multiword_names() {
        assert_eq!(shell_prompt_label("RpcCounter"), "rpc-counter$");
        assert_eq!(shell_prompt_label("RevisionEnvAgent"), "revision-env$");
    }

    #[test]
    fn prompt_label_never_goes_empty() {
        // A type named literally `Agent` must not strip down to nothing.
        assert_eq!(shell_prompt_label("Agent"), "agent$");
        assert_eq!(shell_prompt_label(""), "agent$");
    }

    // --- validate_interactive_surface -------------------------------------------------------

    use golem_common::base_model::Empty;
    use golem_common::model::agent::{AgentTypeName, Snapshotting};
    use golem_common::schema::SchemaGraph;
    use golem_common::schema::agent::{AgentConstructorSchema, NamedField, OutputSchema};

    fn method(name: &str, params: Vec<NamedField>) -> AgentMethodSchema {
        AgentMethodSchema {
            name: name.to_string(),
            description: String::new(),
            prompt_hint: None,
            input_schema: InputSchema::Parameters(params),
            output_schema: OutputSchema::Unit,
            http_endpoint: Vec::new(),
            read_only: None,
        }
    }

    fn string_param(name: &str) -> NamedField {
        NamedField::user_supplied(
            name,
            SchemaType::String {
                metadata: Default::default(),
            },
        )
    }

    fn u8_param(name: &str) -> NamedField {
        NamedField::user_supplied(
            name,
            SchemaType::U8 {
                metadata: Default::default(),
                restrictions: Default::default(),
            },
        )
    }

    fn agent_type(mode: AgentMode, methods: Vec<AgentMethodSchema>) -> AgentTypeSchema {
        AgentTypeSchema {
            type_name: AgentTypeName("TestAgent".to_string()),
            description: String::new(),
            source_language: String::new(),
            schema: SchemaGraph::empty(),
            constructor: AgentConstructorSchema {
                name: None,
                description: String::new(),
                prompt_hint: None,
                input_schema: InputSchema::Parameters(Vec::new()),
            },
            methods,
            dependencies: Vec::new(),
            mode,
            http_mount: None,
            snapshotting: Snapshotting::Disabled(Empty {}),
            config: Vec::new(),
        }
    }

    fn shell_surface() -> Vec<AgentMethodSchema> {
        vec![
            method(EVAL, vec![string_param("cmd")]),
            method(ANSWER_PROMPT, vec![string_param("response")]),
            method(ABORT_PROMPT, Vec::new()),
        ]
    }

    #[test]
    fn a_conforming_durable_surface_validates() {
        let agent = agent_type(AgentMode::Durable, shell_surface());
        assert!(validate_interactive_surface(&agent).is_ok());
    }

    #[test]
    fn an_ephemeral_agent_is_rejected_up_front() {
        let agent = agent_type(AgentMode::Ephemeral, shell_surface());
        let err = validate_interactive_surface(&agent)
            .unwrap_err()
            .to_string();
        assert!(err.contains("ephemeral"), "err: {err}");
    }

    #[test]
    fn an_abort_prompt_with_parameters_is_rejected_at_connect() {
        let agent = agent_type(
            AgentMode::Durable,
            vec![
                method(EVAL, vec![string_param("cmd")]),
                method(ANSWER_PROMPT, vec![string_param("response")]),
                method(ABORT_PROMPT, vec![string_param("reason")]),
            ],
        );
        let err = validate_interactive_surface(&agent)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("abort_prompt") && err.contains("no parameters"),
            "err: {err}"
        );
    }

    #[test]
    fn a_wrong_typed_parameter_is_diagnosed_as_a_type_problem() {
        let agent = agent_type(
            AgentMode::Durable,
            vec![
                method(EVAL, vec![u8_param("cmd")]),
                method(ANSWER_PROMPT, vec![string_param("response")]),
                method(ABORT_PROMPT, Vec::new()),
            ],
        );
        let err = validate_interactive_surface(&agent)
            .unwrap_err()
            .to_string();
        assert!(err.contains("is not a string"), "err: {err}");
        assert!(!err.contains("takes 1"), "must not misreport arity: {err}");
    }

    #[test]
    fn a_missing_method_is_named_in_the_error() {
        let agent = agent_type(
            AgentMode::Durable,
            vec![method(EVAL, vec![string_param("cmd")])],
        );
        let err = validate_interactive_surface(&agent)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("answer_prompt") && err.contains("abort_prompt"),
            "err: {err}"
        );
    }
}
