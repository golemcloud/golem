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

//! `golem mcp serve`: the management commands of the command table as MCP tools over stdio.
//!
//! Each non-interactive row of [`COMMAND_TABLE`] is one tool. Its input schema comes from the
//! clap arguments of the row's command, without the global flags and the row's CLI-only
//! arguments; its output schema comes from the command's structured output types that are not
//! streams or the row's CLI-only output types. A tool call becomes the command's argv with the row's fixed flags and `--format json`,
//! which the CLI's own parser reads and the CLI's own handler runs. The JSON documents that the
//! command prints are the tool's result. A row whose command streams, such as `agent oplog`, has a
//! fixed flag that makes the command print one bounded document instead, such as `--page`, so
//! that every call reads a bounded amount.
//!
//! The server works in the one environment that it resolves at start. Every tool call gets a new
//! [`Context`], made from the server's context, that is bound to that environment. A destructive
//! tool runs only when the call passes `confirm: true`. Calls run one at a time, because the CLI's
//! log state and output are global. A call that the client cancels does not run when it waits,
//! and stops when it runs.

use super::agent::invocation_session::InvocationFailure;
use super::partial_match::ErrorHandler;
use super::{CommandHandler, CommandHandlerHooks, Handlers, LazyContext};
use crate::command::{GolemCliCommand, GolemCliCommandParseResult, GolemCliGlobalFlags};
use crate::command_name;
use crate::command_table::{COMMAND_TABLE, CommandClass, CommandRow};
use crate::context::{Cancellation, Context};
use crate::error::service::{ServiceError, ServiceErrorKind};
use crate::error::{
    ContextInitHintError, HintError, InputRequired, LoginRequired, NonSuccessfulExit,
    OutsideEnvironment, PipedExitCode, ProfileMissing, PromptAnswer,
};
use crate::log::{
    LogColorize, LogOutput, OUTPUT_CAPTURE_LIMIT, Output, OutputCapture, TracingSuppression,
    discard_held_log, error_message_for_output, log_action, log_error, log_warn,
    message_for_output, print_line, set_log_output,
};
use crate::model::cli_output::{
    CLI_OUTPUT_TYPE_FIELD, command_output_schema_value, command_output_types,
    focused_command_output_schema,
};
use crate::model::environment::{EnvironmentResolveMode, ResolvedEnvironmentIdentity};
use anyhow::{anyhow, bail};
use clap::{Arg, ArgAction, Command, CommandFactory};
use golem_common::base_model::api::error_code;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
    JsonObject, ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
    ToolAnnotations,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler, ServiceExt};
use serde_json::{Map, Value, json};
use std::any::TypeId;
use std::pin::Pin;
use std::sync::Arc;
use std::task::Poll;
use std::time::Duration;
use tokio::io::{AsyncRead, ReadBuf};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

mod oplog_export;

use oplog_export::{OPLOG_EXPORT_TOOL, OplogExportArguments};

/// The tool argument that confirms a destructive command, as `--yes` does on the CLI.
pub const CONFIRM_ARGUMENT: &str = "confirm";

/// What the tool returns as `structuredContent`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultShape {
    /// Every output type of the tool emits one document per run; it is the result.
    Document,
    /// An output type of the tool emits conditional documents per run; the result is
    /// `{"items": [...]}` with every document in order.
    Documents,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArgumentKind {
    Flag,
    Value,
    Values,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ValueType {
    String,
    Integer,
    OneOf(Vec<String>),
}

#[derive(Debug, Clone)]
struct ToolArgument {
    id: String,
    kind: ArgumentKind,
    value_type: ValueType,
    long: Option<String>,
    /// The position of a positional argument; `None` for a flag or an option.
    index: Option<usize>,
    /// The positional argument that takes the values after `--`.
    last: bool,
    /// The largest value that the tool accepts, from the row's `capped_args`.
    maximum: Option<u64>,
}

/// One MCP tool: a non-interactive row of the command table.
#[derive(Debug, Clone)]
pub struct ManagementTool {
    pub row: &'static CommandRow,
    pub shape: ResultShape,
    pub tool: Tool,
    /// The `$type`s of the documents that a call may return: the command's output types that
    /// are not streams and not the row's CLI-only output types.
    output_types: Vec<String>,
    arguments: Vec<ToolArgument>,
}

impl ManagementTool {
    pub fn name(&self) -> &str {
        &self.tool.name
    }
}

/// The MCP tool name of a command path: `invocation cancel` is `invocation_cancel`.
pub fn tool_name(path: &[&str]) -> String {
    path.join("_").replace('-', "_")
}

/// The tools of every non-interactive row of the command table.
pub fn management_tools() -> anyhow::Result<Vec<ManagementTool>> {
    let mut root = GolemCliCommand::command();
    root.build();
    let global_ids = root
        .get_arguments()
        .map(|arg| arg.get_id().to_string())
        .collect::<Vec<_>>();
    let schema = command_output_schema_value()?;

    COMMAND_TABLE
        .iter()
        .filter(|row| !row.interactive)
        .map(|row| {
            let command = row
                .path
                .iter()
                .try_fold(&root, |command, name| command.find_subcommand(name))
                .ok_or_else(|| anyhow!("`{}` is not a command", row.path.join(" ")))?;
            management_tool(row, command, &global_ids, &schema)
        })
        .collect()
}

fn management_tool(
    row: &'static CommandRow,
    command: &Command,
    global_ids: &[String],
    schema: &Value,
) -> anyhow::Result<ManagementTool> {
    let path = row.path.join(" ");
    let fixed_ids = command
        .get_arguments()
        .filter(|arg| {
            arg.get_long()
                .is_some_and(|long| row.fixed_args.contains(&format!("--{long}").as_str()))
        })
        .map(|arg| arg.get_id().to_string())
        .collect::<Vec<_>>();

    let mut arguments = Vec::new();
    let mut properties = Map::new();
    let mut required = Vec::new();
    for arg in command.get_arguments() {
        let id = arg.get_id().as_str();
        if arg.is_global_set()
            || arg.is_hide_set()
            || global_ids.iter().any(|global| global == id)
            || row.cli_only_args.contains(&id)
            || fixed_ids.iter().any(|fixed| fixed == id)
            || matches!(
                arg.get_action(),
                ArgAction::Help | ArgAction::HelpShort | ArgAction::HelpLong | ArgAction::Version
            )
        {
            continue;
        }
        if id == CONFIRM_ARGUMENT {
            bail!("`{path}` has an argument named `{CONFIRM_ARGUMENT}`");
        }
        let mut argument =
            tool_argument(arg).map_err(|error| anyhow!("`{path}` `{id}`: {error}"))?;
        if let Some((_, maximum)) = row.capped_args.iter().find(|(capped, _)| *capped == id) {
            if argument.value_type != ValueType::Integer {
                bail!("`{path}` caps `{id}`, which is not an integer");
            }
            argument.maximum = Some(*maximum);
        }
        properties.insert(id.to_string(), argument_schema(arg, &argument));
        if arg.is_required_set() {
            required.push(Value::String(id.to_string()));
        }
        arguments.push(argument);
    }
    if row.destructive {
        properties.insert(
            CONFIRM_ARGUMENT.to_string(),
            json!({
                "type": "boolean",
                "description": "Confirm this destructive call, as `--yes` does on the CLI. Without it the call is refused with `confirmation-required` when the command asks for a confirmation."
            }),
        );
    }
    let input_schema = json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    });

    let output_types = command_output_types(&path)?
        .into_iter()
        .filter(|output_type| {
            schema["definitions"][output_type]["x-golem-output-mode"] != "stream"
                && !row.cli_only_output_types.contains(&output_type.as_str())
        })
        .collect::<Vec<_>>();
    if output_types.is_empty() {
        bail!("`{path}` has no JSON output type that is not a stream");
    }
    let shape = if output_types
        .iter()
        .all(|output_type| schema["definitions"][output_type]["x-golem-output-mode"] == "single")
    {
        ResultShape::Document
    } else {
        ResultShape::Documents
    };
    let output_schema = output_schema(shape, focused_command_output_schema(&output_types)?);

    let about = command
        .get_long_about()
        .or(command.get_about())
        .map(|about| about.to_string())
        .unwrap_or_default();
    let description = match tool_note(&path) {
        Some(note) => format!("`golem {path}`: {about}\n\n{note}"),
        None => format!("`golem {path}`: {about}"),
    };
    let tool = Tool::new_with_raw(
        tool_name(row.path),
        Some(description.into()),
        Arc::new(as_object(input_schema)),
    )
    .with_raw_output_schema(Arc::new(as_object(output_schema)))
    .with_title(format!("golem {path}"))
    .with_annotations(
        ToolAnnotations::new()
            .read_only(row.class == CommandClass::ManagementRead)
            .destructive(row.destructive),
    );

    Ok(ManagementTool {
        row,
        shape,
        tool,
        output_types,
        arguments,
    })
}

/// What a tool's description adds to the command's help about the tool's own behaviour.
fn tool_note(path: &str) -> Option<&'static str> {
    match path {
        // The platform cancels only an invocation that waits in the agent's queue; the
        // invocation session has no message that stops one that has started.
        "agent invoke" => Some(
            "Cancelling the call of a method without streams only stops the wait: the agent keeps \
             running the invocation, and the call returns no result. Cancelling the call of a \
             method with streams also cancels its streams. The result holds every document that \
             the invocation emits; a result larger than 4 MiB, such as of a long stream, ends the \
             call with `output-too-large` and cancels the streams. To run an invocation at most \
             once across retries, pass the same `idempotency_key` on each try.",
        ),
        _ => None,
    }
}

fn tool_argument(arg: &Arg) -> anyhow::Result<ToolArgument> {
    let kind = match arg.get_action() {
        ArgAction::SetTrue => ArgumentKind::Flag,
        ArgAction::Set => ArgumentKind::Value,
        ArgAction::Append => ArgumentKind::Values,
        action => bail!("unsupported argument action {action:?}"),
    };
    let value_parser = arg.get_value_parser();
    let possible_values = arg
        .get_possible_values()
        .iter()
        .filter(|value| !value.is_hide_set())
        .map(|value| value.get_name().to_string())
        .collect::<Vec<_>>();
    let integer_types = [
        TypeId::of::<u8>(),
        TypeId::of::<u16>(),
        TypeId::of::<u32>(),
        TypeId::of::<u64>(),
        TypeId::of::<usize>(),
        TypeId::of::<i8>(),
        TypeId::of::<i16>(),
        TypeId::of::<i32>(),
        TypeId::of::<i64>(),
        TypeId::of::<isize>(),
    ];
    let value_type = if kind != ArgumentKind::Flag && !possible_values.is_empty() {
        ValueType::OneOf(possible_values)
    } else if integer_types
        .iter()
        .any(|type_id| value_parser.type_id() == *type_id)
    {
        ValueType::Integer
    } else {
        ValueType::String
    };
    let positional = arg.is_positional();
    if !positional && arg.get_long().is_none() {
        bail!("an option without a long name");
    }
    Ok(ToolArgument {
        id: arg.get_id().to_string(),
        kind,
        value_type,
        long: arg.get_long().map(str::to_string),
        index: if positional { arg.get_index() } else { None },
        last: arg.is_last_set(),
        maximum: None,
    })
}

fn argument_schema(arg: &Arg, argument: &ToolArgument) -> Value {
    let mut value_schema = match &argument.value_type {
        ValueType::String => json!({"type": "string"}),
        ValueType::Integer => json!({"type": "integer"}),
        ValueType::OneOf(values) => json!({"type": "string", "enum": values}),
    };
    if let Some(maximum) = argument.maximum {
        value_schema["maximum"] = json!(maximum);
    }
    let mut schema = match argument.kind {
        ArgumentKind::Flag => json!({"type": "boolean"}),
        ArgumentKind::Value => value_schema,
        ArgumentKind::Values => json!({"type": "array", "items": value_schema}),
    };
    let help = arg
        .get_long_help()
        .or(arg.get_help())
        .map(|help| help.to_string());
    if let Some(help) = help {
        schema["description"] = Value::String(help);
    }
    if argument.kind == ArgumentKind::Value
        && let [default] = arg.get_default_values()
    {
        let default = default.to_string_lossy().to_string();
        schema["default"] = match argument.value_type {
            ValueType::Integer => default
                .parse::<i64>()
                .map(Value::from)
                .unwrap_or(Value::String(default)),
            _ => Value::String(default),
        };
    }
    schema
}

fn output_schema(shape: ResultShape, focused: Value) -> Value {
    let mut focused = as_object(focused);
    match shape {
        ResultShape::Document => {
            focused.insert("type".to_string(), Value::String("object".to_string()));
            Value::Object(focused)
        }
        ResultShape::Documents => {
            let definitions = focused.remove("definitions").unwrap_or_default();
            let one_of = focused.remove("oneOf").unwrap_or_default();
            json!({
                "type": "object",
                "description": "The documents that the command emits, in order.",
                "properties": {
                    "items": {"type": "array", "items": {"oneOf": one_of}},
                },
                "required": ["items"],
                "additionalProperties": false,
                "definitions": definitions,
            })
        }
    }
}

fn as_object(value: Value) -> JsonObject {
    match value {
        Value::Object(object) => object,
        _ => JsonObject::new(),
    }
}

/// The argv of a tool call: the command path, the arguments, the row's fixed flags,
/// `--format=json`, and `--yes` when the call confirms. Every value is passed as `--long=value`
/// or after `--`, so that no value can become a flag. An argument that the tool does not take is
/// refused here, before the parser sees it.
pub fn tool_call_argv(
    tool: &ManagementTool,
    arguments: &JsonObject,
) -> Result<Vec<String>, String> {
    for key in arguments.keys() {
        let known = tool.arguments.iter().any(|argument| argument.id == *key)
            || (tool.row.destructive && key == CONFIRM_ARGUMENT);
        if !known {
            return Err(format!("`{}` takes no argument `{key}`", tool.name()));
        }
    }

    let mut argv = vec![command_name()];
    argv.extend(tool.row.path.iter().map(|segment| segment.to_string()));

    for argument in tool
        .arguments
        .iter()
        .filter(|argument| argument.index.is_none())
    {
        let long = argument.long.as_deref().expect("options have a long name");
        for value in argument_values(tool, argument, arguments.get(&argument.id))? {
            match argument.kind {
                ArgumentKind::Flag => argv.push(format!("--{long}")),
                _ => argv.push(format!("--{long}={value}")),
            }
        }
    }
    argv.extend(tool.row.fixed_args.iter().map(|flag| flag.to_string()));
    argv.push("--format=json".to_string());
    match arguments.get(CONFIRM_ARGUMENT) {
        None | Some(Value::Null) | Some(Value::Bool(false)) => {}
        Some(Value::Bool(true)) => argv.push("--yes".to_string()),
        Some(_) => return Err(format!("`{CONFIRM_ARGUMENT}` must be a boolean")),
    }

    let mut positionals = tool
        .arguments
        .iter()
        .filter(|argument| argument.index.is_some())
        .collect::<Vec<_>>();
    positionals.sort_by_key(|argument| argument.index);
    let has_last = positionals.iter().any(|argument| argument.last);
    if !has_last {
        argv.push("--".to_string());
    }
    let mut missing = None::<&str>;
    for argument in positionals {
        let values = argument_values(tool, argument, arguments.get(&argument.id))?;
        if values.is_empty() {
            missing.get_or_insert(&argument.id);
            continue;
        }
        if let Some(missing) = missing {
            return Err(format!("`{}` needs `{missing}`", argument.id));
        }
        if argument.last {
            argv.push("--".to_string());
        } else if has_last && let Some(value) = values.iter().find(|value| value.starts_with('-')) {
            return Err(format!(
                "`{}` must not start with `-`, but is `{value}`",
                argument.id
            ));
        }
        argv.extend(values);
    }
    Ok(argv)
}

/// The command-line values of one argument; none when it is absent.
fn argument_values(
    tool: &ManagementTool,
    argument: &ToolArgument,
    value: Option<&Value>,
) -> Result<Vec<String>, String> {
    let wrong_type = || {
        let expected = match (argument.kind, &argument.value_type) {
            (ArgumentKind::Flag, _) => "a boolean",
            (ArgumentKind::Value, ValueType::Integer) => "an integer",
            (ArgumentKind::Value, _) => "a string",
            (ArgumentKind::Values, ValueType::Integer) => "an array of integers",
            (ArgumentKind::Values, _) => "an array of strings",
        };
        format!("`{}` must be {expected}", argument.id)
    };
    let scalar = |value: &Value| -> Result<String, String> {
        let text = match (&argument.value_type, value) {
            (ValueType::Integer, Value::Number(number)) if number.is_i64() || number.is_u64() => {
                if let Some(maximum) = argument.maximum
                    && number.as_u64().is_some_and(|value| value > maximum)
                {
                    return Err(format!("`{}` must be at most {maximum}", argument.id));
                }
                number.to_string()
            }
            (ValueType::String | ValueType::OneOf(_), Value::String(text)) => text.clone(),
            _ => return Err(wrong_type()),
        };
        // The value `-` of these arguments reads the process's stdin, which carries JSON-RPC.
        if text == "-" && tool.row.stdin_marker_args.contains(&argument.id.as_str()) {
            return Err(format!(
                "`{}` does not take `-`, which binds stdin on the CLI",
                argument.id
            ));
        }
        Ok(text)
    };
    match (argument.kind, value) {
        (_, None | Some(Value::Null)) => Ok(vec![]),
        (ArgumentKind::Flag, Some(Value::Bool(true))) => Ok(vec![String::new()]),
        (ArgumentKind::Flag, Some(Value::Bool(false))) => Ok(vec![]),
        (ArgumentKind::Flag, Some(_)) => Err(wrong_type()),
        (ArgumentKind::Value, Some(value)) => Ok(vec![scalar(value)?]),
        (ArgumentKind::Values, Some(Value::Array(values))) => values.iter().map(scalar).collect(),
        (ArgumentKind::Values, Some(_)) => Err(wrong_type()),
    }
}

/// The error envelope of a tool call that failed, `{"error": "<code>", "payload": {"message",
/// ...}}`, as JSON text with `isError`. It is not `structuredContent`, because it does not match
/// the tool's output schema, and clients check `structuredContent` against that schema also for
/// an error.
pub fn error_result(code: &str, message: impl Into<String>) -> CallToolResult {
    error_envelope(code, message.into(), Map::new())
}

fn error_envelope(code: &str, message: String, details: Map<String, Value>) -> CallToolResult {
    let mut payload = Map::new();
    payload.insert("message".to_string(), Value::String(message));
    payload.extend(details);
    CallToolResult::error(vec![ContentBlock::text(
        json!({"error": code, "payload": payload}).to_string(),
    )])
}

/// How a failed call is reported: the error code, the message when it is not the command's
/// error message, and the other fields of the payload.
struct ToolError {
    code: String,
    message: Option<String>,
    details: Map<String, Value>,
}

impl ToolError {
    fn new(code: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: None,
            details: Map::new(),
        }
    }
}

/// The error identifier of a platform error code: `AGENT_NOT_FOUND` is `agent-not-found`.
fn platform_error_id(code: &str) -> String {
    code.to_lowercase().replace('_', "-")
}

/// The first cause of type `T` of `error`, also inside the `Arc<anyhow::Error>` that the CLI's
/// shared caches keep as their error, which hides its own causes from `source`.
fn find_cause<T: std::error::Error + Send + Sync + 'static>(error: &anyhow::Error) -> Option<&T> {
    error
        .chain()
        .find_map(|cause| cause.downcast_ref::<T>())
        .or_else(|| {
            error
                .downcast_ref::<Arc<anyhow::Error>>()
                .and_then(|shared| find_cause(shared))
        })
}

/// How `error` is reported. `tool` is the command tool of the call; `None` for a typed tool.
fn classify(error: &anyhow::Error, tool: Option<&ManagementTool>) -> ToolError {
    if let Some(input) = find_cause::<InputRequired>(error) {
        return input_required(input, tool);
    }
    if find_cause::<LoginRequired>(error).is_some() {
        return ToolError::new("login-required");
    }
    if find_cause::<OutsideEnvironment>(error).is_some() {
        return ToolError::new("outside-environment");
    }
    if find_cause::<ProfileMissing>(error).is_some() {
        return ToolError::new("profile-not-found");
    }
    if let Some(service) = find_cause::<ServiceError>(error)
        && let Some(code) = service.error_code()
    {
        let mut tool_error = ToolError::new(platform_error_id(code));
        tool_error
            .details
            .insert("service".to_string(), json!(service.service_name));
        if let ServiceErrorKind::ErrorResponse(response) = &service.kind {
            tool_error
                .details
                .insert("status".to_string(), json!(response.status_code));
            if let Some(agent_error) = response.agent_error() {
                tool_error.details.insert(
                    "agent_error".to_string(),
                    json!({"cause": agent_error.cause, "stderr": agent_error.stderr}),
                );
            }
        }
        return tool_error;
    }
    if let Some(failure) = find_cause::<InvocationFailure>(error) {
        let mut tool_error = ToolError::new(failure.code.as_str());
        if let Some(stderr) = &failure.stderr {
            tool_error
                .details
                .insert("stderr".to_string(), json!(stderr));
        }
        return tool_error;
    }
    if let Some(HintError::EnvironmentHasNoDeployment) = find_cause::<HintError>(error) {
        return ToolError::new(platform_error_id(error_code::ENVIRONMENT_NOT_DEPLOYED));
    }
    ToolError::new("command-failed")
}

/// A question that the command asked: the question, and the tool argument that answers it on the
/// next call, when the tool has one. A typed tool, `None`, has no argument that answers one.
fn input_required(input: &InputRequired, tool: Option<&ManagementTool>) -> ToolError {
    let confirmation = input.answer == Some(PromptAnswer::Yes);
    let answer_with = tool.and_then(|tool| match input.answer {
        Some(PromptAnswer::Yes) => tool.row.destructive.then_some((tool, CONFIRM_ARGUMENT)),
        Some(PromptAnswer::Argument(id)) => tool
            .arguments
            .iter()
            .any(|argument| argument.id == id)
            .then_some((tool, id)),
        None => None,
    });
    let how = match answer_with {
        Some((tool, CONFIRM_ARGUMENT)) => format!(
            "To answer yes, call `{}` again with `{CONFIRM_ARGUMENT}: true`.",
            tool.name()
        ),
        Some((tool, argument)) => format!(
            "To answer, call `{}` again with the argument `{argument}`.",
            tool.name()
        ),
        None => "No argument of this tool answers it; run the command in a terminal with the \
                 `golem` CLI."
            .to_string(),
    };
    let mut prompt = json!({"kind": input.kind.id(), "question": input.question});
    if !input.options.is_empty() {
        prompt["options"] = json!(input.options);
    }
    let mut tool_error = ToolError::new(if confirmation {
        "confirmation-required"
    } else {
        "input-required"
    });
    tool_error.message = Some(format!("{input}. {how}"));
    tool_error.details.insert("prompt".to_string(), prompt);
    if let Some((_, answer_with)) = answer_with {
        tool_error
            .details
            .insert("answer_with".to_string(), json!(answer_with));
    }
    tool_error
}

/// Whether a call of `tool` may run: a destructive tool runs only with `confirm: true`, also when
/// the CLI would not ask.
fn confirmed(tool: &ManagementTool, arguments: &JsonObject) -> bool {
    !tool.row.destructive || arguments.get(CONFIRM_ARGUMENT) == Some(&Value::Bool(true))
}

/// What a call runs.
enum CallWork {
    /// The command of the command tool at this index, with this argv.
    Command { tool: usize, argv: Vec<String> },
    /// A bounded oplog report of the `agent_oplog_export` tool.
    OplogExport(OplogExportArguments),
}

struct ToolCall {
    work: CallWork,
    /// The request's token: the client cancels it, and the server cancels it once it answered.
    cancellation: CancellationToken,
    reply: oneshot::Sender<CallToolResult>,
}

/// How long a cancelled call that holds a [`CancellationCleanup`] has to end its remote work,
/// such as to cancel the streams of an invocation on the server, before the server drops it.
///
/// [`CancellationCleanup`]: crate::context::CancellationCleanup
const CANCELLED_CALL_GRACE: Duration = Duration::from_secs(5);

/// How long the server waits, when its service ends, for its active call to take its
/// cancellation path, which [`CANCELLED_CALL_GRACE`] already bounds.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(CANCELLED_CALL_GRACE.as_secs() + 1);

/// How long the server waits, once its client went away and its calls ended, for rmcp to end the
/// service. rmcp drains the responses for up to five seconds and then closes stdout; a client that
/// closes stdin but does not read stdout blocks that close forever.
const SERVICE_END_GRACE: Duration = Duration::from_secs(10);

/// The error of a call that printed more than [`OUTPUT_CAPTURE_LIMIT`]. The server stopped the
/// command as if the client had cancelled the call.
fn output_too_large() -> CallToolResult {
    error_envelope(
        "output-too-large",
        format!(
            "The command printed more than {} MiB of JSON, the limit of a tool result, and was \
             stopped; the streams of an invocation are cancelled. Narrow the call, such as with a \
             filter, or run the command in a terminal with the `golem` CLI.",
            OUTPUT_CAPTURE_LIMIT / (1024 * 1024)
        ),
        Map::from_iter([("limit_bytes".to_string(), json!(OUTPUT_CAPTURE_LIMIT))]),
    )
}

/// Runs `run` until it ends or `cancellation` is cancelled. A cancelled run is dropped at once,
/// so a command starts no more work, such as the request that its reads prepared. Only a run that
/// holds a cleanup goes on, for at most [`CANCELLED_CALL_GRACE`], to end its remote work.
async fn run_until_cancelled<F: Future>(run: F, cancellation: &Cancellation) -> Option<F::Output> {
    tokio::pin!(run);
    tokio::select! {
        biased;
        () = cancellation.token().cancelled() => {}
        result = &mut run => return Some(result),
    }
    if !cancellation.has_cleanup() {
        return None;
    }
    tokio::time::timeout(CANCELLED_CALL_GRACE, run).await.ok()
}

#[derive(Clone)]
struct ManagementMcpServer {
    tools: Arc<Vec<ManagementTool>>,
    /// The typed tools, which no command row has: `agent_oplog_export`.
    typed_tools: Arc<Vec<Tool>>,
    environment: String,
    calls: mpsc::Sender<ToolCall>,
}

impl ServerHandler for ManagementMcpServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("golem", crate::version()))
            .with_instructions(format!(
                "The Golem management server: each tool runs one `golem` CLI management command \
                 in the environment {}. It is not the application MCP server of a deployment.",
                self.environment
            ))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(
            self.tools
                .iter()
                .map(|tool| tool.tool.clone())
                .chain(self.typed_tools.iter().cloned())
                .collect(),
        ))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let arguments = request.arguments.unwrap_or_default();
        let work = if request.name == OPLOG_EXPORT_TOOL {
            match OplogExportArguments::parse(&arguments) {
                Ok(arguments) => CallWork::OplogExport(arguments),
                Err(message) => return Ok(error_result("invalid-arguments", message).into()),
            }
        } else {
            let Some(index) = self
                .tools
                .iter()
                .position(|tool| tool.name() == request.name)
            else {
                return Err(ErrorData::invalid_params(
                    format!("Unknown tool: {}", request.name),
                    None,
                ));
            };
            let tool = &self.tools[index];
            let argv = match tool_call_argv(tool, &arguments) {
                Ok(argv) => argv,
                Err(message) => return Ok(error_result("invalid-arguments", message).into()),
            };
            if !confirmed(tool, &arguments) {
                return Ok(error_envelope(
                    "confirmation-required",
                    format!(
                        "`{}` is destructive and runs only with `{CONFIRM_ARGUMENT}: true`",
                        tool.name()
                    ),
                    Map::from_iter([("answer_with".to_string(), json!(CONFIRM_ARGUMENT))]),
                )
                .into());
            }
            CallWork::Command { tool: index, argv }
        };
        let (reply, result) = oneshot::channel();
        let call = ToolCall {
            work,
            cancellation: context.ct.clone(),
            reply,
        };
        // The server sends no answer to a cancelled request; returning drops the reply, so that
        // a queued call does not run.
        tokio::select! {
            () = context.ct.cancelled() => {
                Err(ErrorData::internal_error("The tool call was cancelled", None))
            }
            result = async {
                self.calls
                    .send(call)
                    .await
                    .map_err(|_| ErrorData::internal_error("The server is stopping", None))?;
                result
                    .await
                    .map(Into::into)
                    .map_err(|_| ErrorData::internal_error("The tool call did not finish", None))
            } => result,
        }
    }
}

/// Serves the tools over stdio until the client closes stdin.
pub(super) async fn serve<Hooks: CommandHandlerHooks + 'static>(
    lazy: &mut LazyContext<Hooks>,
) -> anyhow::Result<()> {
    let tools = Arc::new(management_tools()?);
    let typed_tools = Arc::new(vec![oplog_export::tool()?]);
    let ctx = lazy.get_or_init().await?;
    // The questions about the manifest are answered here, once, and every call keeps the answer.
    drop(ctx.app_context_lock_mut().await?);
    ctx.finish_application_operation();
    let environment = ctx
        .environment_handler()
        .resolve_environment(EnvironmentResolveMode::Any)
        .await?;
    let environment_name = format!(
        "{}/{}",
        environment.application_name.0, environment.environment_name.0
    );
    log_action(
        "Serving",
        format!(
            "{} management tools over stdio in environment {}",
            tools.len() + typed_tools.len(),
            environment_name.log_color_highlight()
        ),
    );

    let capture = OutputCapture::new();
    let (calls, mut call_receiver) = mpsc::channel::<ToolCall>(1);
    let server = ManagementMcpServer {
        tools: tools.clone(),
        typed_tools,
        environment: environment_name,
        calls,
    };
    let disconnected = CancellationToken::new();
    let (stdin, stdout) = rmcp::transport::stdio();
    let stdin = ClientInput {
        inner: stdin,
        closed: disconnected.clone(),
    };
    let service = server
        .serve((stdin, stdout))
        .await
        .map_err(|error| anyhow!("Failed to start the MCP server: {error}"))?;

    let global_flags = &lazy.global_flags.clone();
    let hooks = &lazy.hooks.clone();
    let (ctx, environment, capture, tools) = (&ctx, &environment, &capture, &tools);
    let quit = execute_calls(
        &mut call_receiver,
        disconnected,
        service.waiting(),
        SERVICE_END_GRACE,
        |call, cancellation| async move {
            // Drops what was printed between calls, such as a late log line or a late document of
            // a cancelled call.
            capture.start_call(cancellation.token().clone());
            let run = async {
                match call.work {
                    CallWork::Command { tool, argv } => {
                        run_call(
                            hooks.clone(),
                            global_flags,
                            ctx,
                            environment,
                            capture,
                            &tools[tool],
                            argv,
                            cancellation.clone(),
                        )
                        .await
                    }
                    CallWork::OplogExport(arguments) => {
                        run_oplog_export_call(
                            ctx,
                            environment,
                            capture,
                            arguments,
                            cancellation.clone(),
                        )
                        .await
                    }
                }
            };
            let result = run_until_cancelled(run, &cancellation).await;
            if capture.overflowed() {
                discard_held_log();
                let _ = call.reply.send(output_too_large());
                return;
            }
            match result {
                Some(result) => {
                    let _ = call.reply.send(result);
                }
                // The next call's capture drops what the dropped call printed, also later.
                None => discard_held_log(),
            }
        },
    )
    .await;
    // The server ends the process here. A client that closed stdin but does not read stdout
    // keeps a response write blocked in Tokio's blocking pool, also after rmcp gave the write up
    // and ended the service, and the runtime would wait for that write forever when it shuts
    // down. Nobody is left to read a response.
    std::process::exit(match quit {
        Some(Ok(_)) => 0,
        Some(Err(error)) => {
            log_error(format!("The MCP server failed: {error}"));
            1
        }
        None => {
            log_warn("The MCP client went away without reading the responses; exiting");
            0
        }
    })
}

/// Runs the queued calls one at a time with `execute` and returns the output of `quit`, the end
/// of the service, or `None` when the service does not end within `service_end_grace` after the
/// calls ended. The client goes away, which cancels `disconnected`, before the service ends:
/// rmcp first drains the responses of the calls that run, for up to five seconds. From then on no
/// queued call runs, and the active call is cancelled, as the client would cancel it, and has
/// [`SHUTDOWN_GRACE`] to end its remote work, such as the streams of an invocation, before the
/// server exits. Dropping it would only detach those streams, and the agent would keep producing
/// them. A service that ends for another reason stops the calls in the same way.
async fn execute_calls<Q, Run: Future<Output = ()>>(
    calls: &mut mpsc::Receiver<ToolCall>,
    disconnected: CancellationToken,
    quit: impl Future<Output = Q>,
    service_end_grace: Duration,
    mut execute: impl FnMut(ToolCall, Cancellation) -> Run,
) -> Option<Q> {
    let executor = async {
        loop {
            let call = tokio::select! {
                biased;
                () = disconnected.cancelled() => break,
                call = calls.recv() => match call {
                    Some(call) => call,
                    None => break,
                },
            };
            // The client cancelled the call while it waited for the one before it.
            if call.cancellation.is_cancelled() || call.reply.is_closed() {
                continue;
            }
            // The call's own token: the client cancels it with the request, and the capture
            // cancels it when the call prints too much.
            let cancellation = Cancellation::new(call.cancellation.child_token());
            let run = execute(call, cancellation.clone());
            tokio::pin!(run);
            tokio::select! {
                () = &mut run => {}
                () = disconnected.cancelled() => {
                    cancellation.token().cancel();
                    run.await;
                }
            }
        }
    };
    tokio::pin!(executor);
    tokio::pin!(quit);
    tokio::select! {
        quit = &mut quit => {
            disconnected.cancel();
            let _ = tokio::time::timeout(SHUTDOWN_GRACE, &mut executor).await;
            Some(quit)
        }
        () = &mut executor => tokio::time::timeout(service_end_grace, quit).await.ok(),
    }
}

/// The server's stdin. It cancels `closed` when the client closes it or it fails: the client went
/// away. rmcp sees the same end, but ends its service only after it drained the responses of the
/// calls that run.
struct ClientInput<R> {
    inner: R,
    closed: CancellationToken,
}

impl<R: AsyncRead + Unpin> AsyncRead for ClientInput<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let wants_bytes = buf.remaining() > 0;
        let filled = buf.filled().len();
        let poll = Pin::new(&mut self.inner).poll_read(cx, buf);
        if let Poll::Ready(result) = &poll
            && wants_bytes
            && (result.is_err() || buf.filled().len() == filled)
        {
            self.closed.cancel();
        }
        poll
    }
}

/// Runs one tool call through the CLI's parser and handler, as `argv` would on the CLI.
#[allow(clippy::too_many_arguments)]
async fn run_call<Hooks: CommandHandlerHooks + 'static>(
    hooks: Arc<Hooks>,
    global_flags: &GolemCliGlobalFlags,
    server_ctx: &Context,
    environment: &ResolvedEnvironmentIdentity,
    capture: &OutputCapture,
    tool: &ManagementTool,
    argv: Vec<String>,
    cancellation: Cancellation,
) -> CallToolResult {
    let command = match GolemCliCommand::try_parse_from_lenient(&argv, false) {
        GolemCliCommandParseResult::FullMatch(command) => command,
        GolemCliCommandParseResult::ErrorWithPartialMatch { error, .. }
        | GolemCliCommandParseResult::Error { error, .. } => {
            return error_result(
                "invalid-arguments",
                message_for_output(error.render()).trim_end(),
            );
        }
    };
    let confirmed = command.global_flags.yes;

    let ctx = match server_ctx
        .for_tool_call(confirmed, environment.clone(), cancellation)
        .await
    {
        Ok(ctx) => Arc::new(ctx),
        Err(error) => {
            let tool_error = classify(&error, Some(tool));
            let message = tool_error
                .message
                .unwrap_or_else(|| error_message_for_output(&error));
            return error_envelope(&tool_error.code, message, tool_error.details);
        }
    };
    let mut lazy = LazyContext::with_context(global_flags.clone(), hooks, ctx.clone());
    let result = CommandHandler::<Hooks>::handle_subcommand(&mut lazy, command.subcommand).await;
    match result {
        Ok(()) => {
            discard_held_log();
            let (stdout, _) = capture.take();
            success(tool, &stdout)
        }
        Err(error) => failure(error, &ctx, global_flags, capture, Some(tool)),
    }
}

/// Runs one call of `agent_oplog_export` with the call's own [`Context`]. The report is printed
/// as one document, so that the output capture checks its size as for every other call.
async fn run_oplog_export_call(
    server_ctx: &Context,
    environment: &ResolvedEnvironmentIdentity,
    capture: &OutputCapture,
    arguments: OplogExportArguments,
    cancellation: Cancellation,
) -> CallToolResult {
    // The call logs nothing: the log and the tracing of a resolution or a read can quote the
    // agent ID or data of the agent.
    let result = {
        let _silent = (LogOutput::new(Output::None), TracingSuppression::new());
        match server_ctx
            .for_tool_call(false, environment.clone(), cancellation)
            .await
        {
            Ok(ctx) => oplog_export::run(Arc::new(ctx), arguments, OUTPUT_CAPTURE_LIMIT).await,
            Err(error) => Err(error),
        }
    };
    match result {
        Ok(document) => {
            print_line(document);
            discard_held_log();
            let (stdout, _) = capture.take();
            match json_documents(&stdout) {
                Ok(mut documents) if documents.len() == 1 => {
                    CallToolResult::structured(documents.remove(0))
                }
                _ => error_result(
                    "command-failed",
                    "The report is not one JSON document".to_string(),
                ),
            }
        }
        Err(error) => {
            discard_held_log();
            let _ = capture.take();
            oplog_export_failure(&error)
        }
    }
}

/// The platform error codes that a failed `agent_oplog_export` call keeps: those of a failed
/// resolution of the agent or a failed read. The code of a server response is any string.
const OPLOG_EXPORT_PLATFORM_ERRORS: &[&str] = &[
    error_code::ACCOUNT_NOT_FOUND,
    error_code::AGENT_NOT_FOUND,
    error_code::AGENT_TYPE_NOT_FOUND,
    error_code::APPLICATION_NOT_FOUND,
    error_code::AUTH_FORBIDDEN,
    error_code::AUTH_UNAUTHORIZED,
    error_code::CLI_UPDATE_REQUIRED,
    error_code::COMPONENT_NOT_FOUND,
    error_code::ENVIRONMENT_NOT_DEPLOYED,
    error_code::ENVIRONMENT_NOT_FOUND,
    error_code::LIMIT_EXCEEDED,
    error_code::SERVER_STARTING,
];

/// The error of a failed `agent_oplog_export` call. An error of the export itself has its own
/// code and message. Another error, such as when the CLI could not make the call's [`Context`] or
/// resolve the agent, keeps only its code, and the service and the HTTP status of a service error:
/// its message, its log and its other details can quote the agent ID or data of the agent. A
/// platform error code that is not in [`OPLOG_EXPORT_PLATFORM_ERRORS`] becomes `command-failed`.
fn oplog_export_failure(error: &anyhow::Error) -> CallToolResult {
    if let Some((code, message, details)) = oplog_export::export_failure(error) {
        return error_envelope(code, message, details);
    }
    let mut tool_error = classify(error, None);
    if let Some(code) = find_cause::<ServiceError>(error).and_then(ServiceError::error_code)
        && tool_error.code == platform_error_id(code)
        && !OPLOG_EXPORT_PLATFORM_ERRORS.contains(&code)
    {
        tool_error.code = "command-failed".to_string();
    }
    let details = tool_error
        .details
        .into_iter()
        .filter(|(key, _)| key == "service" || key == "status")
        .collect();
    let message = format!(
        "`{OPLOG_EXPORT_TOOL}` failed with `{}`. The message of the error is not shown, because \
         it can contain the agent ID or data of the agent.",
        tool_error.code
    );
    error_envelope(&tool_error.code, message, details)
}

/// The JSON documents that a command printed, one per line.
fn json_documents(stdout: &[u8]) -> serde_json::Result<Vec<Value>> {
    String::from_utf8_lossy(stdout)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(serde_json::from_str::<Value>)
        .collect()
}

fn success(tool: &ManagementTool, stdout: &[u8]) -> CallToolResult {
    let mut documents = match json_documents(stdout) {
        Ok(documents) => documents,
        Err(error) => {
            return error_result(
                "command-failed",
                format!("The command printed output that is not JSON: {error}"),
            );
        }
    };
    if let Some(other) = documents.iter().find_map(|document| {
        let output_type = document[CLI_OUTPUT_TYPE_FIELD].as_str().unwrap_or_default();
        (!tool.output_types.iter().any(|known| known == output_type)).then_some(output_type)
    }) {
        return error_result(
            "command-failed",
            format!("The command printed a `{other}` document, which this tool does not return"),
        );
    }
    match tool.shape {
        ResultShape::Document if documents.len() == 1 => {
            CallToolResult::structured(documents.remove(0))
        }
        ResultShape::Document => error_result(
            "command-failed",
            format!(
                "The command printed {} JSON documents instead of one",
                documents.len()
            ),
        ),
        ResultShape::Documents => CallToolResult::structured(json!({ "items": documents })),
    }
}

fn failure(
    error: anyhow::Error,
    ctx: &Arc<Context>,
    global_flags: &GolemCliGlobalFlags,
    capture: &OutputCapture,
    tool: Option<&ManagementTool>,
) -> CallToolResult {
    set_log_output(Output::Stderr);
    // A hint error and a context error are explained by the messages that their handlers log, as
    // on the CLI; the log is the message.
    let logged = if let Some(hint) = error.downcast_ref::<HintError>() {
        let _ = ctx.error_handler().handle_hint_errors(hint);
        true
    } else if let Some(hint) = error.downcast_ref::<ContextInitHintError>() {
        let _ = ErrorHandler::handle_context_init_hint_errors(global_flags, hint);
        true
    } else {
        false
    };
    let (stdout, stderr) = capture.take();
    failure_envelope(&error, tool, logged, &stdout, &stderr)
}

/// The error result of a failed call. A command that only ends with an exit status has already
/// reported its failure: on stdout, such as the failure of a native tool, and in its log. The
/// documents that it printed are the payload's `output`; they are not the tool's result.
fn failure_envelope(
    error: &anyhow::Error,
    tool: Option<&ManagementTool>,
    logged: bool,
    stdout: &[u8],
    stderr: &[u8],
) -> CallToolResult {
    let mut tool_error = classify(error, tool);
    let status_only = error.is::<NonSuccessfulExit>() || error.is::<PipedExitCode>();
    let output = json_documents(stdout)
        .ok()
        .filter(|documents| status_only && !documents.is_empty());
    let log = message_for_output(String::from_utf8_lossy(stderr))
        .trim()
        .to_string();
    let message = match tool_error.message {
        Some(message) => message,
        None => match find_cause::<InvocationFailure>(error) {
            Some(failure) if status_only => message_for_output(failure).trim().to_string(),
            _ if (logged || status_only) && !log.is_empty() => log,
            _ if output.is_some() => "The command failed; `output` has what it printed".to_string(),
            _ if status_only => "The command failed without a message".to_string(),
            _ => error_message_for_output(error),
        },
    };
    if let Some(output) = output {
        tool_error
            .details
            .insert("output".to_string(), Value::Array(output));
    }
    error_envelope(&tool_error.code, message, tool_error.details)
}

#[cfg(test)]
mod tests {
    use super::{
        ArgumentKind, CONFIRM_ARGUMENT, CallWork, ClientInput, ManagementTool, ResultShape,
        SERVICE_END_GRACE, ToolCall, classify, error_envelope, execute_calls, failure_envelope,
        management_tools, oplog_export_failure, run_until_cancelled, success, tool_call_argv,
    };
    use crate::command::{GolemCliCommand, GolemCliCommandParseResult};
    use crate::command_handler::agent::invocation_session::InvocationFailure;
    use crate::command_handler::interactive::{self, Ask, InteractiveHandler, refuse_prompts};
    use crate::command_table::CommandRow;
    use crate::context::Cancellation;
    use crate::error::service::{ServiceError, ServiceErrorKind, ServiceErrorResponse};
    use crate::error::{
        LoginRequired, NonSuccessfulExit, OutsideEnvironment, PipedExitCode, ProfileMissing,
        PromptAnswer,
    };
    use crate::model::format::Format;
    use golem_common::model::invocation_session_public::PublicErrorCode;
    use inquire::{Select, Text};
    use serde_json::{Value, json};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;
    use test_r::test;

    fn tool(name: &str) -> ManagementTool {
        management_tools()
            .unwrap()
            .into_iter()
            .find(|tool| tool.name() == name)
            .unwrap_or_else(|| panic!("no tool {name}"))
    }

    fn argv(name: &str, arguments: Value) -> Result<Vec<String>, String> {
        tool_call_argv(&tool(name), arguments.as_object().unwrap())
    }

    fn parse(argv: Vec<String>) -> GolemCliCommand {
        match GolemCliCommand::try_parse_from_lenient(&argv, false) {
            GolemCliCommandParseResult::FullMatch(command) => command,
            _ => panic!("{argv:?} does not parse"),
        }
    }

    #[test]
    fn values_that_look_like_flags_stay_values() {
        let command = parse(
            argv(
                "agent_revert",
                json!({"agent_id": "--yes", "last_oplog_index": 1}),
            )
            .unwrap(),
        );
        assert!(!command.global_flags.yes);
        assert_eq!(command.global_flags.format, Some(Format::Json));
        assert!(command.global_flags.environment.is_none());

        let command = parse(
            argv(
                "agent_list",
                json!({"filter": ["--environment=other/env", "-Y"]}),
            )
            .unwrap(),
        );
        assert!(!command.global_flags.yes);
        assert!(command.global_flags.environment.is_none());
    }

    #[test]
    fn only_confirm_confirms() {
        let unconfirmed = parse(
            argv(
                "agent_revert",
                json!({"agent_id": "A(\"a\")", "last_oplog_index": 1, CONFIRM_ARGUMENT: false}),
            )
            .unwrap(),
        );
        assert!(!unconfirmed.global_flags.yes);
        let confirmed = parse(
            argv(
                "agent_revert",
                json!({"agent_id": "A(\"a\")", "last_oplog_index": 1, CONFIRM_ARGUMENT: true}),
            )
            .unwrap(),
        );
        assert!(confirmed.global_flags.yes);
        assert!(
            argv(
                "agent_get",
                json!({"agent_id": "A(\"a\")", CONFIRM_ARGUMENT: true})
            )
            .is_err()
        );
    }

    #[test]
    fn arguments_outside_the_schema_are_refused() {
        assert!(argv("agent_list", json!({"filter": ["status == Idle"]})).is_ok());
        for arguments in [
            json!({"filter": ["status == Idle"], "refresh": 400}),
            json!({"filter": ["status == Idle"], "environment": "app/prod"}),
            json!({"filter": ["status == Idle"], "yes": true}),
            json!({"filter": ["status == Idle"], "unknown": "x"}),
        ] {
            assert!(
                argv("agent_list", arguments.clone()).is_err(),
                "{arguments}"
            );
        }
        assert!(
            argv(
                "agent_invoke",
                json!({"agent_id": "A(\"a\")", "function_name": "f"})
            )
            .is_ok()
        );
        for arguments in [
            json!({"agent_id": "A(\"a\")", "function_name": "f", "stdin_format": "raw"}),
            json!({"agent_id": "A(\"a\")", "function_name": "f", "no_stream": true}),
            json!({"agent_id": "A(\"a\")", "function_name": "f", "arguments": ["1", "-"]}),
        ] {
            assert!(
                argv("agent_invoke", arguments.clone()).is_err(),
                "{arguments}"
            );
        }
    }

    fn envelope(result: &rmcp::model::CallToolResult) -> Value {
        assert_eq!(result.is_error, Some(true));
        assert!(result.structured_content.is_none());
        let text = &result.content[0].as_text().unwrap().text;
        serde_json::from_str(text).unwrap()
    }

    fn reported(error: anyhow::Error, tool_name: &str) -> Value {
        let tool = tool(tool_name);
        let tool_error = classify(&error, Some(&tool));
        envelope(&error_envelope(
            &tool_error.code,
            tool_error.message.unwrap_or_else(|| error.to_string()),
            tool_error.details,
        ))
    }

    #[test]
    fn a_prompt_under_the_server_is_a_structured_error() {
        refuse_prompts();

        let asked = Select::new("Select a component:", vec!["shop:cart", "shop:order"]).ask();
        let report = reported(asked.unwrap_err().into(), "agent_list");
        assert_eq!(report["error"], "input-required");
        assert_eq!(
            report["payload"]["prompt"],
            json!({
                "kind": "select",
                "question": "Select a component:",
                "options": ["shop:cart", "shop:order"],
            })
        );
        assert!(report["payload"].get("answer_with").is_none());
        assert!(
            report["payload"]["message"]
                .as_str()
                .unwrap()
                .contains("Select a component:")
        );

        let asked = Text::new("Component name:")
            .ask_answered_by(Some(PromptAnswer::Argument("component_name")));
        let report = reported(asked.unwrap_err().into(), "agent_list");
        assert_eq!(report["payload"]["prompt"]["kind"], "text");
        assert_eq!(report["payload"]["answer_with"], "component_name");

        let read = interactive::stdin("the secret value").unwrap_err();
        let report = reported(read, "agent_list");
        assert_eq!(report["error"], "input-required");
        assert_eq!(report["payload"]["prompt"]["kind"], "stdin");

        let confirmed = InteractiveHandler::confirm_manifest_profile_warning(false).unwrap_err();
        let report = reported(confirmed, "agent_revert");
        assert_eq!(report["error"], "confirmation-required");
        assert_eq!(report["payload"]["prompt"]["kind"], "confirm");
        assert_eq!(report["payload"]["answer_with"], CONFIRM_ARGUMENT);
    }

    #[test]
    fn platform_errors_keep_their_identifier() {
        let not_found = ServiceError {
            service_name: "Worker",
            kind: ServiceErrorKind::ErrorResponse(ServiceErrorResponse {
                status_code: 404,
                errors: vec!["Agent not found".to_string()],
                code: Some("AGENT_NOT_FOUND".to_string()),
                additional_fields: Default::default(),
            }),
        };
        let shared = std::sync::Arc::new(anyhow::Error::from(not_found));
        // As `get_component_revision_by_id` reports a failure of its cache, also with a context.
        for wrapped in [
            anyhow::anyhow!(shared.clone()),
            anyhow::anyhow!(shared).context("Failed to get the component"),
        ] {
            let report = reported(wrapped, "agent_get");
            assert_eq!(report["error"], "agent-not-found");
            assert_eq!(report["payload"]["status"], 404);
            assert_eq!(report["payload"]["service"], "Worker");
        }

        // As `agent invoke` reports a failure of its invocation session, under the revert hint.
        let failed = anyhow::Error::new(InvocationFailure {
            code: PublicErrorCode::PreviousInvocationFailed,
            message: "rejected".to_string(),
            stderr: Some("value is too large".to_string()),
        })
        .context("The previous invocation failed");
        let report = reported(failed, "agent_invoke");
        assert_eq!(report["error"], "previous-invocation-failed");
        assert_eq!(report["payload"]["stderr"], "value is too large");

        let login = anyhow::Error::new(LoginRequired {
            login: "profile cloud".to_string(),
        });
        assert_eq!(reported(login, "agent_get")["error"], "login-required");
        let missing = anyhow::Error::new(ProfileMissing {
            profile: crate::config::ProfileName("gone".to_string()),
        });
        assert_eq!(reported(missing, "agent_get")["error"], "profile-not-found");
        assert_eq!(
            reported(anyhow::anyhow!("anything else"), "agent_get")["error"],
            "command-failed"
        );
    }

    #[test]
    fn an_oplog_export_failure_outside_the_export_keeps_only_its_code() {
        let marker = "CUSTOMER-MARKER";
        let mut additional_fields = serde_json::Map::new();
        additional_fields.insert(
            "workerError".to_string(),
            json!({"cause": marker, "stderr": marker}),
        );
        let service = ServiceError {
            service_name: "Worker",
            kind: ServiceErrorKind::ErrorResponse(ServiceErrorResponse {
                status_code: 404,
                errors: vec![format!("agent {marker} does not exist")],
                code: Some("AGENT_NOT_FOUND".to_string()),
                additional_fields,
            }),
        };
        let report = envelope(&oplog_export_failure(
            &anyhow::Error::from(service).context(format!("resolving {marker}")),
        ));
        assert_eq!(report["error"], "agent-not-found");
        assert_eq!(
            report["payload"]
                .as_object()
                .unwrap()
                .keys()
                .collect::<Vec<_>>(),
            vec!["message", "service", "status"],
            "{report}"
        );
        assert_eq!(report["payload"]["status"], 404);
        assert!(!report.to_string().contains(marker), "{report}");

        let outside = anyhow::Error::new(OutsideEnvironment {
            requested: marker.to_string(),
            environment: "local".to_string(),
        });
        // A server response can have any code; the error code is lowercase.
        let unknown_code = ServiceError {
            service_name: "Worker",
            kind: ServiceErrorKind::ErrorResponse(ServiceErrorResponse {
                status_code: 400,
                errors: vec![],
                code: Some(marker.to_string()),
                additional_fields: Default::default(),
            }),
        };
        let failures = [
            (outside, "outside-environment"),
            (anyhow::Error::from(unknown_code), "command-failed"),
            (anyhow::anyhow!("{marker}"), "command-failed"),
            (
                anyhow::Error::new(NonSuccessfulExit).context(marker),
                "command-failed",
            ),
        ];
        for (error, code) in failures {
            let report = envelope(&oplog_export_failure(&error));
            assert_eq!(report["error"], code, "{report}");
            assert!(
                !report
                    .to_string()
                    .to_lowercase()
                    .contains(&marker.to_lowercase()),
                "{report}"
            );
        }
    }

    #[test]
    fn a_failure_that_only_ends_with_a_status_keeps_what_the_command_printed() {
        let tool_invoke = tool("tool_invoke");
        // As `tool invoke` reports a native tool's failure on stdout, then ends with status 1.
        let report = json!({"$type": "tool.invoke", "response": {"result": {"failure": {
            "error": {"customError": {"code": "disk-full", "message": "the disk is full"}}}}}});
        let printed = format!("{report}\n");
        let quiet = envelope(&failure_envelope(
            &anyhow::Error::new(PipedExitCode(1)),
            Some(&tool_invoke),
            false,
            printed.as_bytes(),
            b"",
        ));
        assert_eq!(quiet["error"], "command-failed");
        assert_eq!(quiet["payload"]["output"], json!([report]));
        assert!(
            !quiet["payload"]["message"].as_str().unwrap().is_empty(),
            "{quiet}"
        );

        let logged = envelope(&failure_envelope(
            &anyhow::Error::new(NonSuccessfulExit),
            Some(&tool_invoke),
            false,
            printed.as_bytes(),
            b"Invoking bash\n",
        ));
        assert_eq!(logged["payload"]["message"], "Invoking bash");
        assert_eq!(logged["payload"]["output"], json!([report]));

        // An error with its own message is not a report on stdout.
        let failed = envelope(&failure_envelope(
            &anyhow::anyhow!("connection refused"),
            Some(&tool_invoke),
            false,
            printed.as_bytes(),
            b"",
        ));
        assert_eq!(failed["payload"]["message"], "connection refused");
        assert!(failed["payload"].get("output").is_none(), "{failed}");

        // As a streaming invocation reports its failure in its events, then ends with a status.
        let event = json!({"$type": "agent.invoke-session-event", "kind": "finished",
            "outcome": "invocation-failed"});
        let streamed = envelope(&failure_envelope(
            &anyhow::Error::new(InvocationFailure {
                code: PublicErrorCode::InvocationFailed,
                message: "intentional failure".to_string(),
                stderr: Some("failure stderr".to_string()),
            })
            .context(NonSuccessfulExit),
            Some(&tool("agent_invoke")),
            false,
            format!("{event}\n").as_bytes(),
            b"",
        ));
        assert_eq!(streamed["error"], "invocation-failed");
        assert_eq!(streamed["payload"]["stderr"], "failure stderr");
        assert_eq!(streamed["payload"]["output"], json!([event]));
        let message = streamed["payload"]["message"].as_str().unwrap();
        assert!(message.contains("intentional failure"), "{message}");
    }

    #[test]
    fn the_stdin_marker_value_is_refused_for_every_listed_argument() {
        let mut checked = 0;
        for tool in management_tools().unwrap() {
            for argument in tool
                .arguments
                .iter()
                .filter(|argument| tool.row.stdin_marker_args.contains(&argument.id.as_str()))
            {
                let value = match argument.kind {
                    ArgumentKind::Values => json!(["x", "-"]),
                    _ => json!("-"),
                };
                let arguments = json!({ argument.id.clone(): value });
                let refused = tool_call_argv(&tool, arguments.as_object().unwrap())
                    .expect_err(&format!("{} {}", tool.name(), argument.id));
                assert!(refused.contains("binds stdin"), "{refused}");
                checked += 1;
            }
        }
        assert!(checked > 0);
    }

    #[test]
    fn the_stdin_marker_value_is_refused_for_the_marked_argument_of_any_row() {
        let mut tool = tool("agent_list");
        tool.row = Box::leak(Box::new(CommandRow {
            stdin_marker_args: &["filter"],
            ..*tool.row
        }));
        let refused = tool_call_argv(&tool, json!({"filter": ["-"]}).as_object().unwrap())
            .expect_err("`-` for the marked `filter`");
        assert!(refused.contains("binds stdin"), "{refused}");
        assert!(
            tool_call_argv(
                &tool,
                json!({"component_name": "-", "filter": ["status == Idle"]})
                    .as_object()
                    .unwrap()
            )
            .is_ok(),
            "`-` for the unmarked `component_name`"
        );
    }

    #[test]
    fn agent_invoke_passes_its_fixed_flags_and_keeps_hyphen_arguments() {
        let command = argv(
            "agent_invoke",
            json!({"agent_id": "A(\"a\")", "function_name": "f", "arguments": ["-5", "x"]}),
        )
        .unwrap();
        assert!(command.contains(&"--no-stream".to_string()));
        let crate::command::GolemCliSubcommand::Agent {
            subcommand:
                crate::command::worker::AgentSubcommand::Invoke {
                    arguments,
                    no_stream,
                    ..
                },
        } = parse(command).subcommand
        else {
            panic!("not agent invoke");
        };
        assert!(no_stream);
        assert_eq!(arguments, vec!["-5".to_string(), "x".to_string()]);
    }

    #[test]
    fn tool_invoke_returns_the_help_of_a_tool_as_its_result() {
        let tool = tool("tool_invoke");
        let help = crate::model::cli_output::to_structured_output_value(
            crate::model::tool_invoke::ToolInvokeHelpView {
                help: "Usage: bash run <script>".to_string(),
            },
        )
        .unwrap();
        let result = success(&tool, format!("{help}\n").as_bytes());
        assert_ne!(result.is_error, Some(true), "{result:?}");
        let structured = result.structured_content.unwrap();
        let documents = match tool.shape {
            ResultShape::Document => vec![structured.clone()],
            ResultShape::Documents => structured["items"].as_array().unwrap().clone(),
        };
        assert_eq!(documents, vec![help]);

        let schema = Value::Object((*tool.tool.output_schema.clone().unwrap()).clone());
        let validator = jsonschema::options().build(&schema).unwrap();
        assert!(
            validator.is_valid(&structured),
            "{:?}",
            validator
                .iter_errors(&structured)
                .map(|error| error.to_string())
                .collect::<Vec<_>>()
        );
        let mut without_help = structured.clone();
        match tool.shape {
            ResultShape::Document => without_help.as_object_mut().unwrap().remove("help"),
            ResultShape::Documents => without_help["items"][0]
                .as_object_mut()
                .unwrap()
                .remove("help"),
        };
        assert!(!validator.is_valid(&without_help));
    }

    /// Two queued calls, as `execute_calls` receives them, and the receivers of their replies,
    /// which the client holds while it waits.
    async fn two_queued_calls() -> (
        tokio::sync::mpsc::Receiver<ToolCall>,
        Vec<tokio::sync::oneshot::Receiver<rmcp::model::CallToolResult>>,
    ) {
        let (calls, receiver) = tokio::sync::mpsc::channel(2);
        let mut replies = Vec::new();
        for tool in 0..2 {
            let (reply, waiting) = tokio::sync::oneshot::channel();
            replies.push(waiting);
            calls
                .send(ToolCall {
                    work: CallWork::Command {
                        tool,
                        argv: Vec::new(),
                    },
                    cancellation: tokio_util::sync::CancellationToken::new(),
                    reply,
                })
                .await
                .unwrap();
        }
        (receiver, replies)
    }

    #[test]
    async fn a_client_that_goes_away_lets_its_active_call_end_its_held_work() {
        let (mut calls, _replies) = two_queued_calls().await;
        // The service drains the responses after the client went away; it ends once the active
        // call ended its held work, so only the client going away can have cancelled the call.
        let disconnected = tokio_util::sync::CancellationToken::new();
        let (held_work_ended, service_end) = tokio::sync::oneshot::channel::<()>();
        let mut held_work_ended = Some(held_work_ended);
        let queued_call_ran = AtomicBool::new(false);

        let quit = execute_calls(
            &mut calls,
            disconnected.clone(),
            async { service_end.await.is_ok() },
            SERVICE_END_GRACE,
            |call, cancellation| {
                let held_work_ended = held_work_ended.take();
                let (disconnected, queued_call_ran) = (&disconnected, &queued_call_ran);
                async move {
                    if matches!(call.work, CallWork::Command { tool: 1, .. }) {
                        queued_call_ran.store(true, Ordering::SeqCst);
                        return;
                    }
                    // An invocation whose streams are open when the client goes away.
                    let command = async {
                        let _cleanup = cancellation.hold_cleanup();
                        disconnected.cancel();
                        cancellation.token().cancelled().await;
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        held_work_ended.unwrap().send(()).unwrap();
                    };
                    run_until_cancelled(command, &cancellation).await;
                }
            },
        )
        .await;

        assert_eq!(
            quit,
            Some(true),
            "the active call did not end its held work"
        );
        assert!(!queued_call_ran.load(Ordering::SeqCst));
    }

    #[test]
    async fn no_queued_call_runs_after_the_client_went_away() {
        let (mut calls, _replies) = two_queued_calls().await;
        let disconnected = tokio_util::sync::CancellationToken::new();
        let first_call_ran = AtomicBool::new(false);
        let queued_call_ran = AtomicBool::new(false);

        // The active call ends by itself after the client went away, while the service still
        // drains its responses.
        execute_calls(
            &mut calls,
            disconnected.clone(),
            tokio::time::sleep(Duration::from_millis(200)),
            SERVICE_END_GRACE,
            |call, _| {
                let (disconnected, first_call_ran, queued_call_ran) =
                    (&disconnected, &first_call_ran, &queued_call_ran);
                async move {
                    if matches!(call.work, CallWork::Command { tool: 1, .. }) {
                        queued_call_ran.store(true, Ordering::SeqCst);
                    } else {
                        first_call_ran.store(true, Ordering::SeqCst);
                        disconnected.cancel();
                    }
                }
            },
        )
        .await;

        assert!(first_call_ran.load(Ordering::SeqCst));
        assert!(!queued_call_ran.load(Ordering::SeqCst));
    }

    #[test]
    async fn a_service_that_does_not_end_after_the_client_went_away_is_given_up() {
        let (mut calls, _replies) = two_queued_calls().await;
        let disconnected = tokio_util::sync::CancellationToken::new();
        let grace = Duration::from_millis(100);
        let started = std::time::Instant::now();
        // A blocked write of a response keeps rmcp from ending the service.
        let quit = execute_calls(
            &mut calls,
            disconnected.clone(),
            std::future::pending::<()>(),
            grace,
            |_, _| {
                disconnected.cancel();
                std::future::ready(())
            },
        )
        .await;
        assert_eq!(quit, None);
        assert!(started.elapsed() >= grace);
    }

    #[test]
    async fn the_client_input_reports_when_the_client_closes_it() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (mut client, server) = tokio::io::duplex(64);
        let closed = tokio_util::sync::CancellationToken::new();
        let mut input = ClientInput {
            inner: server,
            closed: closed.clone(),
        };
        let mut buffer = [0u8; 16];
        client.write_all(b"{}\n").await.unwrap();
        assert_eq!(input.read(&mut buffer).await.unwrap(), 3);
        assert!(!closed.is_cancelled());
        drop(client);
        assert_eq!(input.read(&mut buffer).await.unwrap(), 0);
        assert!(closed.is_cancelled());
    }

    #[test]
    async fn a_cancelled_call_starts_no_more_work_unless_it_ends_held_work() {
        // A command whose read is still running when its call is cancelled.
        let cancellation = Cancellation::default();
        let (read_done, read) = tokio::sync::oneshot::channel::<()>();
        let mutated = AtomicBool::new(false);
        let command = async {
            read.await.unwrap();
            mutated.store(true, Ordering::SeqCst);
        };
        let client = async {
            tokio::task::yield_now().await;
            cancellation.token().cancel();
            read_done.send(()).unwrap();
        };
        let (result, ()) = tokio::join!(run_until_cancelled(command, &cancellation), client);
        assert!(result.is_none());
        assert!(!mutated.load(Ordering::SeqCst));

        // A command that holds a cleanup ends its held work after the cancellation.
        let cancellation = Cancellation::default();
        let cleaned = AtomicBool::new(false);
        let command = async {
            let _cleanup = cancellation.hold_cleanup();
            cancellation.token().cancelled().await;
            tokio::task::yield_now().await;
            cleaned.store(true, Ordering::SeqCst);
        };
        let client = async {
            tokio::task::yield_now().await;
            cancellation.token().cancel();
        };
        let (result, ()) = tokio::join!(run_until_cancelled(command, &cancellation), client);
        assert!(result.is_some());
        assert!(cleaned.load(Ordering::SeqCst));
    }

    #[test]
    fn agent_oplog_and_agent_list_read_one_page() {
        let oplog = argv(
            "agent_oplog",
            json!({"agent_id": "A(\"a\")", "cursor": "12-0-", "omit_payloads": true}),
        )
        .unwrap();
        let crate::command::GolemCliSubcommand::Agent {
            subcommand:
                crate::command::worker::AgentSubcommand::Oplog {
                    page,
                    cursor,
                    omit_payloads,
                    ..
                },
        } = parse(oplog).subcommand
        else {
            panic!("not agent oplog");
        };
        assert!(page);
        assert!(omit_payloads);
        assert_eq!(cursor.unwrap().next_oplog_index, 12);

        let properties = &tool("agent_oplog").tool.input_schema["properties"];
        for export in [
            "obfuscate",
            "keep_scalars",
            "keep_shape",
            "keep_error_text",
            "max_entry_bytes",
        ] {
            assert!(
                properties.get(export).is_none(),
                "agent_oplog offers `{export}`"
            );
            assert!(
                argv("agent_oplog", json!({"agent_id": "A(\"a\")", export: true})).is_err(),
                "agent_oplog takes `{export}`"
            );
        }

        let list = argv("agent_list", json!({"max_count": 50})).unwrap();
        let crate::command::GolemCliSubcommand::Agent {
            subcommand:
                crate::command::worker::AgentSubcommand::List {
                    page, max_count, ..
                },
        } = parse(list).subcommand
        else {
            panic!("not agent list");
        };
        assert!(page);
        assert_eq!(max_count, Some(50));

        for name in ["agent_oplog", "agent_list"] {
            assert!(
                argv(name, json!({"page": false})).is_err(),
                "{name} takes `page`"
            );
        }
        let max_count = &tool("agent_list").tool.input_schema["properties"]["max_count"];
        assert_eq!(max_count["maximum"], 50);
        let refused = argv("agent_list", json!({"max_count": 51})).unwrap_err();
        assert!(refused.contains("at most 50"), "{refused}");
    }

    /// The oplog tool shows the verified claims of the users among the callers only on request,
    /// as `agent oplog --show-claims` does. In the result, the claims are a JSON object.
    #[test]
    fn agent_oplog_tool_shows_the_claims_only_on_request() {
        use crate::command_handler::log::render_command_output_document;
        use crate::model::agent::oplog::AgentOplogPageView;
        use crate::model::agent::oplog::test_support::{
            CLAIMS, CLAIMS_POINTER, entry_of_a_signed_in_user,
        };
        use crate::model::format::Format;

        let oplog = tool("agent_oplog");
        assert_eq!(
            oplog.tool.input_schema["properties"]["show_claims"]["type"],
            "boolean"
        );

        // The argument becomes the flag of the command, and it is off when the call leaves it out.
        let show_claims = |arguments: Value| {
            let crate::command::GolemCliSubcommand::Agent {
                subcommand: crate::command::worker::AgentSubcommand::Oplog { show_claims, .. },
            } = parse(argv("agent_oplog", arguments).unwrap()).subcommand
            else {
                panic!("not agent oplog");
            };
            show_claims
        };
        assert!(show_claims(
            json!({"agent_id": "A(\"a\")", "show_claims": true})
        ));
        assert!(!show_claims(json!({"agent_id": "A(\"a\")"})));
        assert!(!show_claims(
            json!({"agent_id": "A(\"a\")", "show_claims": false})
        ));

        // The result of the tool is the page that the command prints.
        let result_of = |with_claims: bool| {
            let page = render_command_output_document(
                Format::Json,
                false,
                AgentOplogPageView {
                    entries: vec![entry_of_a_signed_in_user(with_claims)],
                    cursor: None,
                },
            )
            .unwrap();
            success(&oplog, format!("{page}\n").as_bytes())
                .structured_content
                .expect("the tool gives a structured result")
        };
        let claims_pointer = format!("/entries/0{CLAIMS_POINTER}");
        let shown = result_of(true);
        let claims = shown.pointer(&claims_pointer).expect("the claims");
        assert!(claims.is_object(), "{claims}");
        assert_eq!(claims, &serde_json::from_str::<Value>(CLAIMS).unwrap());
        assert!(result_of(false).pointer(&claims_pointer).is_none());
    }

    #[test]
    fn a_tool_returns_only_its_own_output_types() {
        let oplog = tool("agent_oplog");
        assert_eq!(oplog.shape, ResultShape::Document);
        let page = json!({"$type": "agent.oplog-page", "entries": [], "cursor": "1-0-"});
        let result = success(&oplog, format!("{page}\n").as_bytes());
        assert_eq!(result.structured_content, Some(page));

        for other in ["agent.oplog", "agent.oplog-export-header"] {
            let document = json!({"$type": other, "index": 1});
            let refused = envelope(&success(&oplog, format!("{document}\n").as_bytes()));
            assert_eq!(refused["error"], "command-failed");
            assert!(
                refused["payload"]["message"]
                    .as_str()
                    .unwrap()
                    .contains(other),
                "{other}"
            );
        }
    }

    #[test]
    fn every_tool_owned_output_type_is_the_result_of_a_typed_tool() {
        let schema = crate::model::cli_output::command_output_schema_value().unwrap();
        let typed = super::oplog_export::tool().unwrap();
        let mut owned = Vec::new();
        for (output_type, definition) in schema["definitions"].as_object().unwrap() {
            if let Some(owner) = definition.get("x-golem-mcp-tool") {
                assert_eq!(owner, &*typed.name, "{output_type}");
                owned.push(output_type.clone());
            }
        }
        assert_eq!(owned, vec!["agent.oplog-export".to_string()]);
        let output_schema = Value::Object(typed.output_schema.as_deref().unwrap().clone());
        assert_eq!(
            output_schema["oneOf"],
            json!([{"$ref": "#/definitions/agent.oplog-export"}])
        );
        assert_eq!(output_schema["type"], "object");
        for tool in management_tools().unwrap() {
            assert_ne!(tool.name(), &*typed.name);
            assert!(
                !tool
                    .output_types
                    .contains(&"agent.oplog-export".to_string()),
                "{}",
                tool.name()
            );
        }
    }

    #[test]
    fn oplog_export_takes_a_closed_range_and_no_continuation() {
        use super::oplog_export::OplogExportArguments;
        let parse = |arguments: Value| {
            let Value::Object(arguments) = arguments else {
                unreachable!()
            };
            OplogExportArguments::parse(&arguments)
        };
        let parsed = parse(
            json!({"agent_id": "A()", "from": 3, "to": 5, "keep_shape": true,
            "max_entry_bytes": null}),
        )
        .unwrap();
        assert_eq!((parsed.from, parsed.to), (3, 5));
        assert_eq!(parsed.keep_shape, Some(true));
        assert_eq!(parsed.max_entry_bytes, None);

        assert!(parse(json!({"agent_id": "A()", "from": 3})).is_err());
        assert!(parse(json!({"agent_id": "A()", "to": 3})).is_err());
        assert!(parse(json!({"agent_id": "A()", "from": -1, "to": 3})).is_err());
        for continuation in [
            "cursor",
            "page",
            "exportId",
            "tokenScope",
            "afterIndex",
            "obfuscate",
        ] {
            assert!(
                parse(json!({"agent_id": "A()", "from": 1, "to": 2, continuation: "x"})).is_err(),
                "{continuation}"
            );
        }
        // The error never quotes a value of the call: a value can be customer data.
        for malformed in [
            json!({"agent_id": "A()", "from": "SECRET-MARKER", "to": 3}),
            json!({"agent_id": ["SECRET-MARKER"], "from": 1, "to": 3}),
            json!({"agent_id": "A()", "from": 1, "to": 3, "keep_shape": "SECRET-MARKER"}),
            json!({"agent_id": "A()", "from": 1, "to": 3, "cursor": "SECRET-MARKER"}),
            json!({"agent_id": "A()", "from": 1, "to": 3, "SECRET-MARKER": 1}),
        ] {
            let refused = parse(malformed.clone()).unwrap_err();
            assert!(!refused.contains("SECRET-MARKER"), "{malformed}: {refused}");
            assert!(refused.contains("`agent_id`"), "{refused}");
        }
        let schema = super::oplog_export::tool().unwrap().input_schema;
        assert_eq!(schema["additionalProperties"], json!(false));
        assert_eq!(schema["required"], json!(["agent_id", "from", "to"]));
    }

    #[test]
    fn oplog_export_failures_have_their_own_codes_and_no_report() {
        use crate::oplog_export::export::OplogExportError;
        let failure = |error: OplogExportError| {
            let (code, message, details) = super::oplog_export::export_failure(
                &anyhow::Error::new(error).context("reading the range"),
            )
            .unwrap();
            (code, message, Value::Object(details))
        };
        let (code, message, details) =
            failure(OplogExportError::ScanBudgetExceeded { from: 2, to: 9 });
        assert_eq!(code, "scan-budget-exceeded");
        assert!(message.contains("from 2 to 9"), "{message}");
        assert!(message.contains("--obfuscate"), "{message}");
        assert_eq!(details, json!({}));

        let (code, _, details) = failure(OplogExportError::OutputTooLarge {
            limit_bytes: 4096,
            matched_entries: None,
        });
        assert_eq!(code, "output-too-large");
        assert_eq!(details, json!({"limit_bytes": 4096}));
        let (_, _, details) = failure(OplogExportError::OutputTooLarge {
            limit_bytes: 4096,
            matched_entries: Some(7),
        });
        assert_eq!(details, json!({"limit_bytes": 4096, "matched_entries": 7}));

        assert_eq!(
            failure(OplogExportError::RangePastEnd {
                to: 9,
                last_index: 4
            })
            .0,
            "invalid-arguments"
        );
        assert_eq!(failure(OplogExportError::StaleCursor).0, "stale-cursor");
        assert_eq!(failure(OplogExportError::RootChanged).0, "command-failed");
        assert!(super::oplog_export::export_failure(&anyhow::anyhow!("other")).is_none());
    }

    #[test]
    fn an_oplog_report_fits_up_to_the_last_byte_of_the_limit() {
        use super::oplog_export::fitted_document;
        use crate::oplog_export::export::OplogExportError;
        use crate::oplog_export::model::{
            OplogExportCoverage, OplogExportHeader, OplogExportReport, OplogReportHeader,
            ReportAgentRef,
        };
        // Escaped and non-ASCII text, so that bytes, characters and the text differ in length.
        let report = OplogExportReport {
            header: OplogReportHeader {
                export: OplogExportHeader {
                    obfuscate: true,
                    keep_scalars: false,
                    keep_shape: false,
                    keep_error_text: false,
                    filtered: false,
                    root: ReportAgentRef {
                        agent_id: golem_common::model::AgentId::from_parts(7, 1),
                        agent_type: Some(golem_common::model::agent::AgentTypeName(
                            "Zähler \"quoted\"".to_string(),
                        )),
                    },
                    cli_version: "1.0 \"quoted\"\n\u{1F600}".to_string(),
                    generated_at: "2026-11-18T00:00:00+00:00 Zürich".to_string(),
                },
                from: 2,
                to: 3,
            },
            entries: vec![],
            coverage: OplogExportCoverage::new(vec!["tab\there".to_string()]),
        };
        let document = fitted_document(report.clone(), usize::MAX).unwrap();
        assert!(document.len() > document.chars().count(), "{document}");
        let parsed: Value = serde_json::from_str(&document).unwrap();
        assert_eq!(parsed["$type"], "agent.oplog-export");
        assert_eq!(parsed["header"]["from"], 2);
        assert_eq!(parsed["coverage"]["notes"], json!(["tab\there"]));
        assert_eq!(
            fitted_document(report.clone(), document.len() + 1).unwrap(),
            document
        );
        let refused = fitted_document(report, document.len())
            .unwrap_err()
            .downcast::<OplogExportError>()
            .unwrap();
        assert_eq!(
            refused,
            OplogExportError::OutputTooLarge {
                limit_bytes: document.len(),
                matched_entries: Some(0),
            }
        );
    }

    #[test]
    fn tool_invoke_takes_its_tool_arguments_after_the_separator() {
        let command = parse(
            argv(
                "tool_invoke",
                json!({"agent": "A(\"a\")", "tool_name": "bash", "tool_args": ["run", "--yes"]}),
            )
            .unwrap(),
        );
        assert!(!command.global_flags.yes);
        let crate::command::GolemCliSubcommand::Tool {
            subcommand: crate::command::tool::ToolSubcommand::Invoke(args),
        } = command.subcommand
        else {
            panic!("not tool invoke");
        };
        assert_eq!(args.tool_name.to_string(), "bash");
        assert_eq!(args.tool_args, vec!["run".to_string(), "--yes".to_string()]);
        assert!(
            argv(
                "tool_invoke",
                json!({"agent": "A(\"a\")", "tool_name": "--yes"})
            )
            .is_err()
        );
    }
}
