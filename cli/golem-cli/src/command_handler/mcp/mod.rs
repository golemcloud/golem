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
//! arguments; its output schema comes from the command's structured output types. A tool call
//! becomes the command's argv with the row's fixed flags and `--format json`, which the CLI's own
//! parser reads and the CLI's own handler runs. The JSON documents that the command prints are the
//! tool's result.
//!
//! The server works in the one environment that it resolves at start. Every tool call gets a new
//! [`Context`], made from the server's context, that is bound to that environment. A destructive
//! tool runs only when the call passes `confirm: true`. Calls run one at a time, because the CLI's
//! log state and output are global.

use super::partial_match::ErrorHandler;
use super::{CommandHandler, CommandHandlerHooks, Handlers, LazyContext};
use crate::command::{GolemCliCommand, GolemCliCommandParseResult, GolemCliGlobalFlags};
use crate::command_name;
use crate::command_table::{COMMAND_TABLE, CommandClass, CommandRow};
use crate::context::Context;
use crate::error::service::{ServiceError, ServiceErrorKind};
use crate::error::{
    ContextInitHintError, HintError, InputRequired, LoginRequired, NonSuccessfulExit,
    OutsideEnvironment, PipedExitCode, PromptAnswer,
};
use crate::log::{
    LogColorize, Output, OutputCapture, discard_held_log, error_message_for_output, log_action,
    message_for_output, set_log_output,
};
use crate::model::cli_output::{
    command_output_schema_value, command_output_types, focused_command_output_schema,
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
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

/// The tool argument that confirms a destructive command, as `--yes` does on the CLI.
pub const CONFIRM_ARGUMENT: &str = "confirm";

/// What the tool returns as `structuredContent`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultShape {
    /// Every output type of the command emits one document per run; it is the result.
    Document,
    /// An output type of the command emits a sequence of documents per run (a finite stream, or
    /// conditional documents); the result is `{"items": [...]}` with every document in order.
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
}

/// One MCP tool: a non-interactive row of the command table.
#[derive(Debug, Clone)]
pub struct ManagementTool {
    pub row: &'static CommandRow,
    pub shape: ResultShape,
    pub tool: Tool,
    arguments: Vec<ToolArgument>,
}

impl ManagementTool {
    pub fn name(&self) -> &str {
        &self.tool.name
    }
}

/// The MCP tool name of a command path: `agent cancel-invocation` is `agent_cancel_invocation`.
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
        let argument = tool_argument(arg).map_err(|error| anyhow!("`{path}` `{id}`: {error}"))?;
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

    let output_types = command_output_types(&path)?;
    if output_types.is_empty() {
        bail!("`{path}` has no JSON output type");
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
    let tool = Tool::new_with_raw(
        tool_name(row.path),
        Some(format!("`golem {path}`: {about}").into()),
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
        arguments,
    })
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
    })
}

fn argument_schema(arg: &Arg, argument: &ToolArgument) -> Value {
    let value_schema = match &argument.value_type {
        ValueType::String => json!({"type": "string"}),
        ValueType::Integer => json!({"type": "integer"}),
        ValueType::OneOf(values) => json!({"type": "string", "enum": values}),
    };
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

fn classify(error: &anyhow::Error, tool: &ManagementTool) -> ToolError {
    if let Some(input) = find_cause::<InputRequired>(error) {
        return input_required(input, tool);
    }
    if find_cause::<LoginRequired>(error).is_some() {
        return ToolError::new("login-required");
    }
    if find_cause::<OutsideEnvironment>(error).is_some() {
        return ToolError::new("outside-environment");
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
    if let Some(HintError::EnvironmentHasNoDeployment) = find_cause::<HintError>(error) {
        return ToolError::new(platform_error_id(error_code::ENVIRONMENT_NOT_DEPLOYED));
    }
    ToolError::new("command-failed")
}

/// A question that the command asked: the question, and the tool argument that answers it on the
/// next call, when the tool has one.
fn input_required(input: &InputRequired, tool: &ManagementTool) -> ToolError {
    let confirmation = input.answer == Some(PromptAnswer::Yes);
    let answer_with = match input.answer {
        Some(PromptAnswer::Yes) => tool.row.destructive.then_some(CONFIRM_ARGUMENT),
        Some(PromptAnswer::Argument(id)) => tool
            .arguments
            .iter()
            .any(|argument| argument.id == id)
            .then_some(id),
        None => None,
    };
    let how = match answer_with {
        Some(CONFIRM_ARGUMENT) => format!(
            "To answer yes, call `{}` again with `{CONFIRM_ARGUMENT}: true`.",
            tool.name()
        ),
        Some(argument) => format!(
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
    if let Some(answer_with) = answer_with {
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

struct ToolCall {
    tool: usize,
    argv: Vec<String>,
    reply: oneshot::Sender<CallToolResult>,
}

#[derive(Clone)]
struct ManagementMcpServer {
    tools: Arc<Vec<ManagementTool>>,
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
            self.tools.iter().map(|tool| tool.tool.clone()).collect(),
        ))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
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
        let arguments = request.arguments.unwrap_or_default();
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
        let (reply, result) = oneshot::channel();
        self.calls
            .send(ToolCall {
                tool: index,
                argv,
                reply,
            })
            .await
            .map_err(|_| ErrorData::internal_error("The server is stopping", None))?;
        result
            .await
            .map(Into::into)
            .map_err(|_| ErrorData::internal_error("The tool call did not finish", None))
    }
}

/// Serves the tools over stdio until the client closes stdin.
pub(super) async fn serve<Hooks: CommandHandlerHooks + 'static>(
    lazy: &mut LazyContext<Hooks>,
) -> anyhow::Result<()> {
    let tools = Arc::new(management_tools()?);
    let ctx = lazy.get_or_init().await?;
    // The questions about the manifest are answered here, once, and every call keeps the answer.
    drop(ctx.app_context_lock_mut().await?);
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
            tools.len(),
            environment_name.log_color_highlight()
        ),
    );

    let capture = OutputCapture::new();
    let (calls, mut call_receiver) = mpsc::channel::<ToolCall>(1);
    let server = ManagementMcpServer {
        tools: tools.clone(),
        environment: environment_name,
        calls,
    };
    let service = server
        .serve(rmcp::transport::stdio())
        .await
        .map_err(|error| anyhow!("Failed to start the MCP server: {error}"))?;

    let global_flags = lazy.global_flags.clone();
    let hooks = lazy.hooks.clone();
    let executor = async {
        while let Some(call) = call_receiver.recv().await {
            let result = run_call(
                hooks.clone(),
                &global_flags,
                &ctx,
                &environment,
                &capture,
                &tools[call.tool],
                call.argv,
            )
            .await;
            let _ = call.reply.send(result);
        }
    };
    tokio::select! {
        quit = service.waiting() => {
            quit.map_err(|error| anyhow!("The MCP server failed: {error}"))?;
        }
        () = executor => {}
    }
    Ok(())
}

/// Runs one tool call through the CLI's parser and handler, as `argv` would on the CLI.
async fn run_call<Hooks: CommandHandlerHooks + 'static>(
    hooks: Arc<Hooks>,
    global_flags: &GolemCliGlobalFlags,
    server_ctx: &Context,
    environment: &ResolvedEnvironmentIdentity,
    capture: &OutputCapture,
    tool: &ManagementTool,
    argv: Vec<String>,
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

    // Drops what was printed between calls, such as a late log line of the previous call.
    let _ = capture.take();
    let ctx = match server_ctx.for_tool_call(confirmed, environment.clone()) {
        Ok(ctx) => Arc::new(ctx),
        Err(error) => return error_result("command-failed", error_message_for_output(&error)),
    };
    let mut lazy = LazyContext::with_context(global_flags.clone(), hooks, ctx.clone());
    let result = CommandHandler::<Hooks>::handle_subcommand(&mut lazy, command.subcommand).await;
    match result {
        Ok(()) => {
            discard_held_log();
            let (stdout, _) = capture.take();
            success(tool, &stdout)
        }
        Err(error) => failure(error, &ctx, global_flags, capture, tool),
    }
}

fn success(tool: &ManagementTool, stdout: &[u8]) -> CallToolResult {
    let documents = String::from_utf8_lossy(stdout)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(serde_json::from_str::<Value>)
        .collect::<Result<Vec<_>, _>>();
    let mut documents = match documents {
        Ok(documents) => documents,
        Err(error) => {
            return error_result(
                "command-failed",
                format!("The command printed output that is not JSON: {error}"),
            );
        }
    };
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
    tool: &ManagementTool,
) -> CallToolResult {
    set_log_output(Output::Stderr);
    let tool_error = classify(&error, tool);
    // A hint error and a context error are explained by the messages that their handlers log, as
    // on the CLI; the log is the message.
    let logged = if let Some(hint) = error.downcast_ref::<HintError>() {
        let _ = ctx.error_handler().handle_hint_errors(hint);
        true
    } else if let Some(hint) = error.downcast_ref::<ContextInitHintError>() {
        let _ = ErrorHandler::handle_context_init_hint_errors(global_flags, hint);
        true
    } else {
        error.is::<NonSuccessfulExit>() || error.is::<PipedExitCode>()
    };
    let (_, stderr) = capture.take();
    let message = match tool_error.message {
        Some(message) => message,
        None if logged => message_for_output(String::from_utf8_lossy(&stderr))
            .trim()
            .to_string(),
        None => error_message_for_output(&error),
    };
    error_envelope(&tool_error.code, message, tool_error.details)
}

#[cfg(test)]
mod tests {
    use super::{
        ArgumentKind, CONFIRM_ARGUMENT, ManagementTool, classify, error_envelope, management_tools,
        tool_call_argv,
    };
    use crate::command::{GolemCliCommand, GolemCliCommandParseResult};
    use crate::command_handler::interactive::{self, Ask, InteractiveHandler, refuse_prompts};
    use crate::error::service::{ServiceError, ServiceErrorKind, ServiceErrorResponse};
    use crate::error::{LoginRequired, PromptAnswer};
    use crate::model::format::Format;
    use inquire::{Select, Text};
    use serde_json::{Value, json};
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
        let tool_error = classify(&error, &tool);
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

        let login = anyhow::Error::new(LoginRequired {
            login: "profile cloud".to_string(),
        });
        assert_eq!(reported(login, "agent_get")["error"], "login-required");
        assert_eq!(
            reported(anyhow::anyhow!("anything else"), "agent_get")["error"],
            "command-failed"
        );
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
