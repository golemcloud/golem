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

//! The `run` contract a bound tool must offer for `golem ssh`, and the pure parts of a session:
//! building a submission, decoding its result, local commands, the prompt and exit statuses.

use crate::context::GlobalEnvironmentSelector;
use golem_client::model::{NativeToolFailure, NativeToolResult, NativeToolSuccess};
use golem_common::model::tool::{SerializableToolError, SerializableToolRpcError};
use golem_common::schema::tool::{OptionShape, Tool};
use golem_common::schema::{SchemaGraph, SchemaType, TypedSchemaValue};
use golem_schema::render::json_value::to_json_value;
use golem_schema::tool::argv::{self, ParsedToolArguments};
use serde::Deserialize;
use std::path::Path;

/// The command every submission calls.
pub const RUN: &str = "run";

/// The process status when no command ran, following OpenSSH.
pub const NOT_RUN_EXIT: u8 = 255;

/// The process status after Ctrl+C stopped waiting for a command.
pub const INTERRUPTED_EXIT: u8 = 130;

/// The result of one `run` call. Fields beyond these four are ignored.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct BashResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: u8,
    pub cwd: String,
}

/// Why a submission produced no [`BashResult`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallFailure {
    /// The tool refused the call with one of its declared errors, before running anything.
    Named { name: String, detail: String },
    /// Golem refused the call: authorization, middleware or filesystem policy.
    Denied(String),
    /// The call failed without a result.
    Rpc(String),
    /// A result came back, but it is not a `run` result, so the final directory is unknown.
    Undecodable(String),
    /// The request failed without a definite answer, so the command may or may not have run.
    Unknown(String),
}

/// How a submission ended, for the process exit status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The script ran and exited with this status.
    Ran(u8),
    /// Nothing ran: the call was refused, failed, or returned no usable result.
    NotRun,
    /// Ctrl+C stopped waiting; the command may still be running on the agent.
    Interrupted,
}

/// A line the session handles itself instead of sending it to the tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalCommand {
    /// `exit` leaves with the last command's status; `exit N` leaves with `N`.
    Exit(Option<u8>),
}

/// Checks that `tool` offers the `run` command `golem ssh` submits to, before anything is
/// submitted: a `--cwd` string option, a `script` string positional, a `--timeout` integer option
/// when one will be passed, and a record result with `stdout`, `stderr` and `cwd` strings and an
/// integer `exit_code`. Every other input must have a default.
pub fn check_run_contract(tool: &Tool, with_timeout: bool) -> Result<(), String> {
    let schema = &tool.schema;
    let nodes = &tool.commands.nodes;
    let run = nodes
        .first()
        .into_iter()
        .flat_map(|root| &root.subcommands)
        .filter_map(|index| {
            usize::try_from(index.0)
                .ok()
                .and_then(|index| nodes.get(index))
        })
        .find(|node| node.name == RUN)
        .and_then(|node| node.body.as_ref())
        .ok_or_else(|| format!("it has no `{RUN}` command"))?;
    let scalar_option = |long: &str| {
        run.options
            .iter()
            .find(|option| option.long == long)
            .and_then(|option| match &option.shape {
                OptionShape::Scalar(ty) | OptionShape::OptionalScalar(ty) => Some(ty),
                _ => None,
            })
    };
    if !scalar_option("cwd").is_some_and(|ty| resolves_to(schema, ty, is_string)) {
        return Err(format!("`{RUN}` has no `--cwd` string option"));
    }
    if !run.positionals.fixed.iter().any(|positional| {
        positional.name == "script" && resolves_to(schema, &positional.type_, is_string)
    }) {
        return Err(format!("`{RUN}` has no `script` string positional"));
    }
    if with_timeout
        && !scalar_option("timeout").is_some_and(|ty| resolves_to(schema, ty, is_integer))
    {
        return Err(format!("`{RUN}` has no `--timeout` integer option"));
    }
    let result = run
        .result
        .as_ref()
        .ok_or_else(|| format!("`{RUN}` declares no result"))?;
    let Ok(SchemaType::Record { fields, .. }) = schema.resolve_ref(&result.type_) else {
        return Err(format!("the `{RUN}` result is not a record"));
    };
    for (name, kind, accept) in RESULT_FIELDS {
        if !fields
            .iter()
            .any(|field| field.name == name && resolves_to(schema, &field.body, accept))
        {
            return Err(format!("the `{RUN}` result has no `{name}` {kind} field"));
        }
    }
    match argv::parse(tool, &run_argv("/", with_timeout.then_some(1), "true")) {
        Ok(ParsedToolArguments::Invoke { .. }) => Ok(()),
        Ok(ParsedToolArguments::Help(_)) => Err(format!("`{RUN}` shows help instead of running")),
        Err(error) => Err(format!(
            "`{RUN}` needs more than a directory and a script: {error}"
        )),
    }
}

/// A result field `golem ssh` reads: its name, the kind named in diagnostics, and its type check.
type ResultField = (&'static str, &'static str, fn(&SchemaType) -> bool);

const RESULT_FIELDS: [ResultField; 4] = [
    ("stdout", "string", is_string),
    ("stderr", "string", is_string),
    ("exit_code", "integer", is_integer),
    ("cwd", "string", is_string),
];

fn resolves_to(schema: &SchemaGraph, ty: &SchemaType, accept: fn(&SchemaType) -> bool) -> bool {
    schema.resolve_ref(ty).is_ok_and(accept)
}

fn is_string(ty: &SchemaType) -> bool {
    matches!(ty, SchemaType::String { .. })
}

fn is_integer(ty: &SchemaType) -> bool {
    matches!(
        ty,
        SchemaType::S8 { .. }
            | SchemaType::S16 { .. }
            | SchemaType::S32 { .. }
            | SchemaType::S64 { .. }
            | SchemaType::U8 { .. }
            | SchemaType::U16 { .. }
            | SchemaType::U32 { .. }
            | SchemaType::U64 { .. }
    )
}

/// Builds the argument list of one submission. The options use the attached `--name=value`
/// form, which every scalar option shape reads as its value, and `--` keeps a script that starts
/// with `-` a positional.
pub fn run_argv(cwd: &str, timeout: Option<u32>, script: &str) -> Vec<String> {
    let mut argv = vec![RUN.to_string()];
    if !cwd.is_empty() {
        argv.push(format!("--cwd={cwd}"));
    }
    if let Some(timeout) = timeout {
        argv.push(format!("--timeout={timeout}"));
    }
    argv.push("--".to_string());
    argv.push(script.to_string());
    argv
}

/// Decodes the result of a scalar `run` call.
pub fn decode_result(result: Option<NativeToolResult>) -> Result<BashResult, CallFailure> {
    match result {
        Some(NativeToolResult::Success(NativeToolSuccess {
            result: Some(value),
        })) => {
            let typed = value.as_inner();
            let json = to_json_value(typed.graph(), &typed.graph().root, typed.value())
                .map_err(|error| CallFailure::Undecodable(error.to_string()))?;
            serde_json::from_value(json)
                .map_err(|error| CallFailure::Undecodable(format!("not a `{RUN}` result: {error}")))
        }
        Some(NativeToolResult::Success(NativeToolSuccess { result: None })) => Err(
            CallFailure::Undecodable("the call succeeded without a result".to_string()),
        ),
        Some(NativeToolResult::Failure(NativeToolFailure { error })) => Err(match error {
            SerializableToolRpcError::RemoteToolError(error) => match *error {
                SerializableToolError::CustomError(error) => CallFailure::Named {
                    detail: error_detail(&error.payload),
                    name: error.name,
                },
                SerializableToolError::InvalidToolName(name) => {
                    CallFailure::Rpc(format!("invalid tool name: {name}"))
                }
                SerializableToolError::InvalidCommandPath(path) => {
                    CallFailure::Rpc(format!("invalid command path: {}", path.join(" ")))
                }
                SerializableToolError::InvalidInput(message) => {
                    CallFailure::Rpc(format!("invalid input: {message}"))
                }
                SerializableToolError::ConstraintViolation(message) => {
                    CallFailure::Rpc(format!("constraint violation: {message}"))
                }
                SerializableToolError::InvalidResult(message) => {
                    CallFailure::Rpc(format!("invalid result: {message}"))
                }
            },
            SerializableToolRpcError::Denied(message) => CallFailure::Denied(message),
            SerializableToolRpcError::ProtocolError(message) => {
                CallFailure::Rpc(format!("protocol error: {message}"))
            }
            SerializableToolRpcError::NotFound(message) => {
                CallFailure::Rpc(format!("not found: {message}"))
            }
            SerializableToolRpcError::RemoteInternalError(message) => {
                CallFailure::Rpc(format!("internal error: {message}"))
            }
            SerializableToolRpcError::Cancelled => CallFailure::Rpc("cancelled".to_string()),
            SerializableToolRpcError::ResourceExhausted(message) => {
                CallFailure::Rpc(format!("resource exhausted: {message}"))
            }
        }),
        None => Err(CallFailure::Undecodable(
            "the response carried no result".to_string(),
        )),
    }
}

/// A declared error's payload as text: the string itself, a record's `reason`, or its JSON.
fn error_detail(payload: &TypedSchemaValue) -> String {
    match to_json_value(payload.graph(), &payload.graph().root, payload.value()) {
        Ok(serde_json::Value::String(text)) => text,
        Ok(serde_json::Value::Object(fields))
            if fields.get("reason").is_some_and(|r| r.is_string()) =>
        {
            fields["reason"].as_str().unwrap_or_default().to_string()
        }
        Ok(value) => value.to_string(),
        Err(error) => error.to_string(),
    }
}

/// Recognises a line that is exactly `exit` or `exit N`, ignoring surrounding whitespace. As in
/// bash, `N` is taken modulo 256.
pub fn local_command(line: &str) -> Option<LocalCommand> {
    let mut words = line.split_whitespace();
    if words.next() != Some("exit") {
        return None;
    }
    match (words.next(), words.next()) {
        (None, _) => Some(LocalCommand::Exit(None)),
        (Some(status), None) if status.bytes().all(|byte| byte.is_ascii_digit()) => Some(
            LocalCommand::Exit(Some(status.bytes().fold(0u8, |status, digit| {
                status.wrapping_mul(10).wrapping_add(digit - b'0')
            }))),
        ),
        _ => None,
    }
}

/// The parts of the prompt, so a terminal can style each one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptPart {
    Agent,
    Cwd,
    Status,
    /// The trailing marker; `ok` is whether the last command exited 0.
    Marker {
        ok: bool,
    },
}

/// The interactive prompt: the agent, the remembered directory once known, the last status when
/// it was not zero (because `$?` does not carry to the next command), and a marker. `paint`
/// styles each part.
pub fn prompt(
    agent: &str,
    cwd: &str,
    last_status: u8,
    paint: impl Fn(PromptPart, &str) -> String,
) -> String {
    let mut prompt = paint(PromptPart::Agent, agent);
    if !cwd.is_empty() {
        prompt.push(' ');
        prompt.push_str(&paint(PromptPart::Cwd, cwd));
    }
    if last_status != 0 {
        prompt.push(' ');
        prompt.push_str(&paint(PromptPart::Status, &format!("[{last_status}]")));
    }
    prompt.push(' ');
    prompt.push_str(&paint(
        PromptPart::Marker {
            ok: last_status == 0,
        },
        "\u{276f}",
    ));
    prompt.push(' ');
    prompt
}

/// The command that looks up a submission's result later under its idempotency key, without
/// running it again. `global_args` are the profile and environment flags of this session.
pub fn lookup_command(
    executable: &str,
    global_args: &[String],
    agent: &str,
    key: &str,
    tool: &str,
) -> String {
    let mut args = vec![executable.to_string()];
    args.extend(global_args.iter().cloned());
    args.extend(
        [
            "tool",
            "invoke",
            "--agent",
            agent,
            "--lookup",
            "--idempotency-key",
            key,
            tool,
            "--",
            RUN,
        ]
        .map(str::to_string),
    );
    args.iter()
        .map(|arg| shlex::try_quote(arg).map_or_else(|_| arg.clone(), |quoted| quoted.into_owned()))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The global options that select where the session's agent lives: the configuration directory
/// when it is not the default, the profile, and the environment selector.
pub fn global_args(
    config_dir: Option<&Path>,
    profile: Option<&str>,
    environment: Option<&GlobalEnvironmentSelector>,
) -> Vec<String> {
    let mut args = Vec::new();
    if let Some(config_dir) = config_dir {
        args.extend(["--config-dir".to_string(), config_dir.display().to_string()]);
    }
    if let Some(profile) = profile {
        args.extend(["--profile".to_string(), profile.to_string()]);
    }
    match environment {
        Some(GlobalEnvironmentSelector::Environment(environment)) => {
            args.extend(["--environment".to_string(), environment.to_string()]);
        }
        Some(GlobalEnvironmentSelector::Local) => args.push("--local".to_string()),
        Some(GlobalEnvironmentSelector::Cloud) => args.push("--cloud".to_string()),
        None => {}
    }
    args
}

/// Removes terminal cursor-position reports (`ESC [ row ; col R`) from a line read without the
/// line editor. A terminal that answers the editor's query too late delivers its answer as input.
pub fn strip_cursor_reports(line: &str) -> String {
    let mut rest = line;
    let mut stripped = String::with_capacity(line.len());
    while let Some(start) = rest.find("\x1b[") {
        stripped.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match cursor_report_len(after) {
            Some(len) => rest = &after[len..],
            None => {
                stripped.push_str("\x1b[");
                rest = after;
            }
        }
    }
    stripped.push_str(rest);
    stripped
}

/// The length of `row;colR` at the start of `text`, when it is one.
fn cursor_report_len(text: &str) -> Option<usize> {
    let row = text.bytes().take_while(u8::is_ascii_digit).count();
    let after_row = text[row..].strip_prefix(';')?;
    let col = after_row.bytes().take_while(u8::is_ascii_digit).count();
    (row > 0 && col > 0 && after_row[col..].starts_with('R')).then_some(row + 1 + col + 1)
}

/// Reads a failed request to the invoke endpoint. `status` is the HTTP status when the server
/// answered: a 4xx is a definite refusal, while no answer or a 5xx leaves the outcome unknown.
pub fn classify_invoke_error(status: Option<u16>, message: String) -> CallFailure {
    match status {
        Some(403) => CallFailure::Denied(message),
        Some(status) if (400..500).contains(&status) => CallFailure::Rpc(message),
        _ => CallFailure::Unknown(message),
    }
}

/// How the session reads its input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    /// The line editor, with history and hints.
    Editor,
    /// Plain lines after a prompt on stderr.
    PromptedLines,
    /// Plain lines with no prompt, for piped input.
    Lines,
}

/// The editor queries the cursor position on stdout, so it is used only when stdout is the
/// terminal as well.
pub fn input_mode(stdin_is_terminal: bool, stdout_is_terminal: bool) -> InputMode {
    match (stdin_is_terminal, stdout_is_terminal) {
        (true, true) => InputMode::Editor,
        (true, false) => InputMode::PromptedLines,
        (false, _) => InputMode::Lines,
    }
}

pub fn exit_code(outcome: Outcome) -> u8 {
    match outcome {
        Outcome::Ran(status) => status,
        Outcome::NotRun => NOT_RUN_EXIT,
        Outcome::Interrupted => INTERRUPTED_EXIT,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BashResult, CallFailure, INTERRUPTED_EXIT, InputMode, LocalCommand, NOT_RUN_EXIT, Outcome,
        PromptPart, check_run_contract, classify_invoke_error, decode_result, exit_code,
        global_args, input_mode, local_command, lookup_command, prompt, run_argv,
        strip_cursor_reports,
    };
    use crate::context::GlobalEnvironmentSelector;
    use golem_client::model::{NativeToolFailure, NativeToolResult, NativeToolSuccess};
    use golem_common::model::tool::{
        SerializableCustomToolError, SerializableToolError, SerializableToolRpcError,
    };
    use golem_common::schema::tool::Tool;
    use golem_common::schema::{
        NamedFieldType, SchemaGraph, SchemaType, SchemaValue, TypedSchemaValue,
    };
    use serde_json::{Value, json};
    use std::path::Path;
    use test_r::test;

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    /// The effective definition of the bash 0.2.0 release, as `describe_tool` returns it.
    fn pinned() -> Value {
        serde_json::from_str(include_str!("bash-0.2.0.json")).unwrap()
    }

    fn check(definition: Value, with_timeout: bool) -> Result<(), String> {
        let tool: Tool = serde_json::from_value(definition).unwrap();
        check_run_contract(&tool, with_timeout)
    }

    fn run_body(definition: &mut Value) -> &mut Value {
        &mut definition["commands"]["nodes"][1]["body"]
    }

    fn option<'a>(definition: &'a mut Value, long: &str) -> &'a mut Value {
        run_body(definition)["options"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|option| option["long"] == long)
            .unwrap()
    }

    fn remove_option(definition: &mut Value, long: &str) {
        run_body(definition)["options"]
            .as_array_mut()
            .unwrap()
            .retain(|option| option["long"] != long);
    }

    fn result_fields(definition: &mut Value) -> &mut Vec<Value> {
        definition["schema"]["defs"][0]["body"]["value"]["fields"]
            .as_array_mut()
            .unwrap()
    }

    fn rejected(definition: Value, with_timeout: bool, mentions: &str) {
        let reason = check(definition, with_timeout).unwrap_err();
        assert!(reason.contains(mentions), "{reason}");
    }

    #[test]
    fn the_bash_release_is_compatible() {
        assert_eq!(check(pinned(), false), Ok(()));
        assert_eq!(check(pinned(), true), Ok(()));
    }

    #[test]
    fn a_tool_without_run_is_rejected() {
        let mut definition = pinned();
        definition["commands"]["nodes"][1]["name"] = json!("exec");
        rejected(definition, false, "run");
    }

    #[test]
    fn a_missing_or_mistyped_cwd_is_rejected() {
        let mut definition = pinned();
        remove_option(&mut definition, "cwd");
        rejected(definition, false, "cwd");

        let mut definition = pinned();
        let cwd = option(&mut definition, "cwd");
        cwd["shape"]["value"] = json!({"kind": "u32", "value": {}});
        cwd["default"] = json!({"kind": "u32", "value": 0});
        rejected(definition, false, "cwd");
    }

    #[test]
    fn a_missing_or_mistyped_script_is_rejected() {
        let mut definition = pinned();
        run_body(&mut definition)["positionals"]["fixed"][0]["name"] = json!("command");
        rejected(definition, false, "script");

        let mut definition = pinned();
        run_body(&mut definition)["positionals"]["fixed"][0]["type_"] =
            json!({"kind": "u32", "value": {}});
        rejected(definition, false, "script");
    }

    #[test]
    fn a_result_missing_a_field_or_with_a_wrong_type_is_rejected() {
        let mut definition = pinned();
        result_fields(&mut definition).retain(|field| field["name"] != "cwd");
        rejected(definition, false, "cwd");

        let mut definition = pinned();
        result_fields(&mut definition)
            .iter_mut()
            .find(|field| field["name"] == "exit_code")
            .unwrap()["body"] = json!({"kind": "string", "value": {}});
        rejected(definition, false, "exit_code");

        let mut definition = pinned();
        run_body(&mut definition)["result"] = Value::Null;
        rejected(definition, false, "result");
    }

    #[test]
    fn a_timeout_is_needed_only_when_requested() {
        let mut definition = pinned();
        remove_option(&mut definition, "timeout");
        assert_eq!(check(definition.clone(), false), Ok(()));
        rejected(definition, true, "timeout");
    }

    #[test]
    fn another_required_input_is_rejected() {
        let mut definition = pinned();
        let mut extra = option(&mut definition, "cwd").clone();
        extra["long"] = json!("user");
        extra["default"] = Value::Null;
        extra["required"] = json!(true);
        run_body(&mut definition)["options"]
            .as_array_mut()
            .unwrap()
            .push(extra);
        rejected(definition, false, "user");
    }

    fn record(fields: Vec<(&str, SchemaType, SchemaValue)>) -> TypedSchemaValue {
        let (types, values): (Vec<_>, Vec<_>) = fields
            .into_iter()
            .map(|(name, body, value)| {
                (
                    NamedFieldType {
                        name: name.to_string(),
                        body,
                        metadata: Default::default(),
                    },
                    value,
                )
            })
            .unzip();
        TypedSchemaValue::new(
            SchemaGraph::anonymous(SchemaType::record(types)),
            SchemaValue::Record { fields: values },
        )
    }

    fn success(value: TypedSchemaValue) -> Option<NativeToolResult> {
        Some(NativeToolResult::Success(NativeToolSuccess {
            result: Some(value.try_into().unwrap()),
        }))
    }

    fn failure(error: SerializableToolRpcError) -> Option<NativeToolResult> {
        Some(NativeToolResult::Failure(NativeToolFailure { error }))
    }

    fn bash_result(cwd: &str, exit_code: u8) -> TypedSchemaValue {
        record(vec![
            (
                "stdout",
                SchemaType::string(),
                SchemaValue::String("out".to_string()),
            ),
            (
                "stderr",
                SchemaType::string(),
                SchemaValue::String("err".to_string()),
            ),
            ("exit_code", SchemaType::u8(), SchemaValue::U8(exit_code)),
            (
                "cwd",
                SchemaType::string(),
                SchemaValue::String(cwd.to_string()),
            ),
        ])
    }

    #[test]
    fn argv_omits_an_empty_cwd() {
        assert_eq!(run_argv("", None, "pwd"), strings(&["run", "--", "pwd"]));
    }

    #[test]
    fn argv_attaches_the_cwd_and_timeout() {
        assert_eq!(
            run_argv("/tmp", Some(5), "pwd"),
            strings(&["run", "--cwd=/tmp", "--timeout=5", "--", "pwd"])
        );
    }

    #[test]
    fn argv_keeps_a_dash_script_and_newlines_as_one_positional() {
        assert_eq!(
            run_argv("/", None, "-x"),
            strings(&["run", "--cwd=/", "--", "-x"])
        );
        assert_eq!(
            run_argv("", None, "echo a\necho b"),
            strings(&["run", "--", "echo a\necho b"])
        );
    }

    #[test]
    fn exit_alone_or_with_a_status_is_local() {
        assert_eq!(local_command("exit"), Some(LocalCommand::Exit(None)));
        assert_eq!(local_command("  exit  "), Some(LocalCommand::Exit(None)));
        assert_eq!(local_command("exit 3"), Some(LocalCommand::Exit(Some(3))));
        assert_eq!(
            local_command("exit 300"),
            Some(LocalCommand::Exit(Some(44)))
        );
    }

    #[test]
    fn anything_else_goes_to_the_tool() {
        for line in [
            "exit3",
            "echo exit",
            "exit 3; ls",
            "exit abc",
            "exit 1 2",
            "",
        ] {
            assert_eq!(local_command(line), None, "{line:?}");
        }
    }

    fn plain(_: PromptPart, text: &str) -> String {
        text.to_string()
    }

    #[test]
    fn prompt_shows_the_directory_once_known() {
        assert_eq!(prompt("A(\"m1\")", "", 0, plain), "A(\"m1\") \u{276f} ");
        assert_eq!(
            prompt("A(\"m1\")", "/tmp", 0, plain),
            "A(\"m1\") /tmp \u{276f} "
        );
    }

    #[test]
    fn prompt_shows_a_nonzero_status() {
        assert_eq!(prompt("a", "/tmp", 7, plain), "a /tmp [7] \u{276f} ");
        assert_eq!(prompt("a", "", 255, plain), "a [255] \u{276f} ");
    }

    #[test]
    fn prompt_paints_each_part() {
        let tagged = |part: PromptPart, text: &str| format!("<{part:?}:{text}>");
        assert_eq!(
            prompt("a", "/tmp", 7, tagged),
            "<Agent:a> <Cwd:/tmp> <Status:[7]> <Marker { ok: false }:\u{276f}> "
        );
        assert_eq!(
            prompt("a", "/", 0, tagged),
            "<Agent:a> <Cwd:/> <Marker { ok: true }:\u{276f}> "
        );
    }

    #[test]
    fn lookup_command_quotes_what_the_shell_would_split() {
        assert_eq!(
            lookup_command(
                "golem",
                &strings(&["--profile", "my profile"]),
                "A(\"m1\")",
                "k-1",
                "bash"
            ),
            "golem --profile 'my profile' tool invoke --agent 'A(\"m1\")' --lookup \
             --idempotency-key k-1 bash -- run"
        );
    }

    #[test]
    fn global_args_select_the_same_agent() {
        assert_eq!(global_args(None, None, None), Vec::<String>::new());
        assert_eq!(
            global_args(
                Some(Path::new("/etc/golem")),
                Some("staging"),
                Some(&GlobalEnvironmentSelector::Local)
            ),
            strings(&[
                "--config-dir",
                "/etc/golem",
                "--profile",
                "staging",
                "--local"
            ])
        );
        assert_eq!(
            global_args(None, None, Some(&GlobalEnvironmentSelector::Cloud)),
            strings(&["--cloud"])
        );
    }

    #[test]
    fn late_cursor_reports_are_removed_from_plain_lines() {
        assert_eq!(strip_cursor_reports("\x1b[12;1Rpwd"), "pwd");
        assert_eq!(strip_cursor_reports("ec\x1b[3;40Rho x"), "echo x");
        assert_eq!(strip_cursor_reports("ls\x1b[1;1R\x1b[1;1R"), "ls");
    }

    #[test]
    fn other_escapes_stay_in_plain_lines() {
        for line in [
            "printf '\x1b[1m'",
            "echo \x1b[12;R",
            "echo [3;4R",
            "pwd",
            "",
        ] {
            assert_eq!(strip_cursor_reports(line), line, "{line:?}");
        }
    }

    #[test]
    fn an_unanswered_or_failed_request_leaves_the_outcome_unknown() {
        for status in [None, Some(500), Some(502), Some(503)] {
            assert_eq!(
                classify_invoke_error(status, "lost".to_string()),
                CallFailure::Unknown("lost".to_string()),
                "{status:?}"
            );
        }
    }

    #[test]
    fn a_refused_request_is_a_definite_failure() {
        assert_eq!(
            classify_invoke_error(Some(403), "no".to_string()),
            CallFailure::Denied("no".to_string())
        );
        for status in [400, 404, 409, 422] {
            assert_eq!(
                classify_invoke_error(Some(status), "bad".to_string()),
                CallFailure::Rpc("bad".to_string()),
                "{status}"
            );
        }
    }

    #[test]
    fn the_editor_needs_both_stdin_and_stdout_on_a_terminal() {
        assert_eq!(input_mode(true, true), InputMode::Editor);
        assert_eq!(input_mode(true, false), InputMode::PromptedLines);
        assert_eq!(input_mode(false, true), InputMode::Lines);
        assert_eq!(input_mode(false, false), InputMode::Lines);
    }

    #[test]
    fn exit_status_follows_the_outcome() {
        assert_eq!(exit_code(Outcome::Ran(0)), 0);
        assert_eq!(exit_code(Outcome::Ran(7)), 7);
        assert_eq!(exit_code(Outcome::NotRun), NOT_RUN_EXIT);
        assert_eq!(exit_code(Outcome::Interrupted), INTERRUPTED_EXIT);
        assert_eq!((NOT_RUN_EXIT, INTERRUPTED_EXIT), (255, 130));
    }

    #[test]
    fn a_run_result_decodes_including_nonzero_status() {
        assert_eq!(
            decode_result(success(bash_result("/tmp", 7))),
            Ok(BashResult {
                stdout: "out".to_string(),
                stderr: "err".to_string(),
                exit_code: 7,
                cwd: "/tmp".to_string(),
            })
        );
    }

    #[test]
    fn extra_result_fields_are_ignored() {
        let mut value = bash_result("/", 0);
        let (graph, SchemaValue::Record { mut fields }) = value.clone().into_parts() else {
            unreachable!()
        };
        let SchemaType::Record {
            fields: mut types, ..
        } = graph.root.clone()
        else {
            unreachable!()
        };
        types.push(NamedFieldType {
            name: "extra".to_string(),
            body: SchemaType::bool(),
            metadata: Default::default(),
        });
        fields.push(SchemaValue::Bool(true));
        value = TypedSchemaValue::new(
            SchemaGraph::anonymous(SchemaType::record(types)),
            SchemaValue::Record { fields },
        );
        assert_eq!(decode_result(success(value)).unwrap().cwd, "/");
    }

    #[test]
    fn a_result_without_a_directory_is_undecodable() {
        let value = record(vec![
            (
                "stdout",
                SchemaType::string(),
                SchemaValue::String(String::new()),
            ),
            (
                "stderr",
                SchemaType::string(),
                SchemaValue::String(String::new()),
            ),
            ("exit_code", SchemaType::u8(), SchemaValue::U8(0)),
        ]);
        assert!(matches!(
            decode_result(success(value)),
            Err(CallFailure::Undecodable(_))
        ));
    }

    #[test]
    fn a_mistyped_or_missing_result_is_undecodable() {
        let mistyped = record(vec![("cwd", SchemaType::u32(), SchemaValue::U32(1))]);
        assert!(matches!(
            decode_result(success(mistyped)),
            Err(CallFailure::Undecodable(_))
        ));
        let empty = Some(NativeToolResult::Success(NativeToolSuccess {
            result: None,
        }));
        assert!(matches!(
            decode_result(empty),
            Err(CallFailure::Undecodable(_))
        ));
        assert!(matches!(
            decode_result(None),
            Err(CallFailure::Undecodable(_))
        ));
    }

    #[test]
    fn a_declared_tool_error_keeps_its_name_and_reason() {
        let error = SerializableToolRpcError::RemoteToolError(Box::new(
            SerializableToolError::CustomError(Box::new(SerializableCustomToolError {
                name: "invalid-cwd".to_string(),
                payload: TypedSchemaValue::new(
                    SchemaGraph::anonymous(SchemaType::string()),
                    SchemaValue::String("/gone: no such directory".to_string()),
                ),
            })),
        ));
        assert_eq!(
            decode_result(failure(error)),
            Err(CallFailure::Named {
                name: "invalid-cwd".to_string(),
                detail: "/gone: no such directory".to_string(),
            })
        );
    }

    #[test]
    fn denials_and_other_failures_stay_distinct() {
        assert_eq!(
            decode_result(failure(SerializableToolRpcError::Denied(
                "filesystem access denied".to_string()
            ))),
            Err(CallFailure::Denied("filesystem access denied".to_string()))
        );
        for error in [
            SerializableToolRpcError::RemoteInternalError("trap".to_string()),
            SerializableToolRpcError::NotFound("gone".to_string()),
            SerializableToolRpcError::Cancelled,
            SerializableToolRpcError::RemoteToolError(Box::new(
                SerializableToolError::InvalidInput("bad".to_string()),
            )),
        ] {
            assert!(
                matches!(
                    decode_result(failure(error.clone())),
                    Err(CallFailure::Rpc(_))
                ),
                "{error:?}"
            );
        }
    }
}
