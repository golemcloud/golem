//! Project owner bindings into shell commands without copying the CLI argument grammar.

#[cfg(target_arch = "wasm32")]
use bash_shell::commands::CommandDescriptor;
use bash_shell::commands::{CommandFuture, CommandInvoker, CommandOutput, PreparedCommand};
#[cfg(target_arch = "wasm32")]
use bash_shell::session::Session;
use golem_rust::bindings::golem::tool::streams::{
    ByteStreamCloseCause, ByteStreamFailure, StreamWriteError,
};
use golem_schema::TypedSchemaValue;
use golem_schema::tool::{
    Tool,
    process::{
        ParsedProcessCommand, parse_command, process_status_type, render_custom_error,
        render_result,
    },
};
use std::collections::{BTreeMap, HashMap};
#[cfg(target_arch = "wasm32")]
use std::sync::Arc;

#[cfg(target_arch = "wasm32")]
pub(crate) const MAX_ATTACHMENT_BYTES: usize = 16 * 1024 * 1024;

pub(crate) struct Catalog {
    tools: BTreeMap<String, Tool>,
}

impl Catalog {
    pub(crate) fn new(tools: BTreeMap<String, Tool>) -> Result<Self, String> {
        for (name, tool) in &tools {
            golem_schema::tool::validation::validate_tool(tool)
                .map_err(|errors| format!("invalid tool {name:?}: {errors:?}"))?;
            for command in &tool.commands.nodes {
                if let Some(result) = command.body.as_ref().and_then(|body| body.result.as_ref()) {
                    process_status_type(&tool.schema, &result.type_)
                        .map_err(|error| format!("invalid tool {name:?}: {error}"))?;
                }
            }
        }
        Ok(Self { tools })
    }
}

impl Catalog {
    fn prepare_invocation(
        &self,
        name: &str,
        args: &[String],
        variables: &HashMap<String, String>,
    ) -> Result<Invocation, CommandOutput> {
        let tool = self
            .tools
            .get(name)
            .ok_or_else(|| failure(127, "tool is not bound"))?;
        let parsed = parse_command(tool, args, variables).map_err(|message| {
            // The shared parser forbids constructing opaque capabilities from text.
            let message = if message.contains("capabilit") || message.contains("secret") {
                format!("secret and other capability arguments are unsupported: {message}")
            } else {
                message
            };
            failure(2, message)
        })?;
        match parsed {
            ParsedProcessCommand::Help(text) => Err(CommandOutput {
                stdout: text.into_bytes(),
                stderr: vec![],
                exit_code: 0,
            }),
            ParsedProcessCommand::Invoke {
                command_path,
                input,
                stdin,
                stdout,
                stderr,
                result_is_status,
                errors,
            } => Ok(Invocation {
                name: name.into(),
                path: command_path,
                input: *input,
                stdin,
                stdout,
                stderr,
                result_is_status,
                errors,
            }),
        }
    }
}

impl CommandInvoker for Catalog {
    fn prepare(
        &self,
        name: &str,
        args: &[String],
        variables: &HashMap<String, String>,
    ) -> Result<Box<dyn PreparedCommand>, CommandOutput> {
        self.prepare_invocation(name, args, variables)
            .map(|invocation| Box::new(invocation) as Box<dyn PreparedCommand>)
    }
}

// RPC-only fields are still exercised by native argument-parity tests.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
struct Invocation {
    name: String,
    path: Vec<String>,
    input: TypedSchemaValue,
    stdin: bool,
    stdout: bool,
    stderr: bool,
    result_is_status: bool,
    errors: BTreeMap<String, u8>,
}

impl PreparedCommand for Invocation {
    fn takes_stdin(&self) -> bool {
        self.stdin
    }

    fn invoke(&self, stdin: Option<Vec<u8>>) -> CommandFuture<'_> {
        Box::pin(async move {
            #[cfg(target_arch = "wasm32")]
            {
                transport::invoke(self, stdin).await
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                let _ = stdin;
                failure(1, "bound tools require a Golem host")
            }
        })
    }
}

/// The exit status and stderr line for a failed tool call: denied 3, not found 127, cancelled
/// 130, invalid input 2, a declared error's own status (1 when undeclared), anything else 1.
/// A custom error's payload is rendered redacted and never debug-printed: it may carry
/// capabilities.
fn rpc_failure(
    error: golem_rust::schema::wit::wire::ToolRpcError,
    declared: &BTreeMap<String, u8>,
) -> (u8, String) {
    use golem_rust::schema::wit::wire::{ToolError, ToolRpcError};
    match error {
        ToolRpcError::Denied(message) => (3, message),
        ToolRpcError::NotFound(message) => (127, message),
        ToolRpcError::Cancelled => (130, "Cancelled".into()),
        ToolRpcError::RemoteToolError(
            ToolError::InvalidInput(message) | ToolError::ConstraintViolation(message),
        ) => (2, message),
        ToolRpcError::RemoteToolError(ToolError::CustomError(custom)) => {
            let payload = golem_rust::decode_typed_schema_value(&custom.payload).ok();
            render_custom_error(&custom.name, payload.as_ref(), declared)
        }
        ToolRpcError::ProtocolError(message)
        | ToolRpcError::RemoteInternalError(message)
        | ToolRpcError::ResourceExhausted(message) => (1, message),
        other => (1, format!("{other:?}")),
    }
}

/// How a bound command is named in its diagnostics: the tool, then its subcommand path.
fn command_label(name: &str, path: &[String]) -> String {
    std::iter::once(name)
        .chain(path.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Why an output channel of a bound command ended before all of its bytes arrived.
enum OutputFailure {
    /// The stream ended with a failure item.
    Stream(ByteStreamFailure),
    /// The channel passed the 16 MiB limit, and the shell stopped reading it.
    LimitExceeded,
}

/// The stderr line for an output channel that did not end cleanly. A crashed tool's streams and
/// its call error carry the same text, and the call error follows on stderr, so the line leaves
/// out a text that `call_error` repeats.
fn output_failure_line(
    command: &str,
    channel: &str,
    failure: &OutputFailure,
    call_error: Option<&str>,
) -> String {
    match failure {
        OutputFailure::LimitExceeded => format!("{command}: {channel} attachment limit exceeded"),
        OutputFailure::Stream(failure) => {
            format!(
                "{command}: {channel}: {}",
                stream_failure_text(failure, call_error)
            )
        }
    }
}

/// The stderr line for a write to a bound command's stdin that failed.
fn stdin_failure_line(command: &str, error: &StreamWriteError) -> String {
    let reason = match error {
        StreamWriteError::Closed(ByteStreamCloseCause::Finished) => "closed".to_string(),
        StreamWriteError::Closed(ByteStreamCloseCause::ConsumerCancelled) => {
            "closed by the tool".to_string()
        }
        StreamWriteError::Closed(ByteStreamCloseCause::Failed(failure)) => {
            stream_failure_text(failure, None)
        }
        StreamWriteError::ConcurrentOperation => "concurrent write".to_string(),
    };
    format!("{command}: stdin: {reason}")
}

/// A stream failure in plain words, never debug-printed. `repeated` is a text that the caller
/// writes anyway; a failure text equal to it is left out.
fn stream_failure_text(failure: &ByteStreamFailure, repeated: Option<&str>) -> String {
    match failure {
        ByteStreamFailure::Cancelled => "cancelled".to_string(),
        ByteStreamFailure::Abandoned => "abandoned".to_string(),
        ByteStreamFailure::ResourceExhausted => "resource exhausted".to_string(),
        ByteStreamFailure::Failed(details) if repeated == Some(details.as_str()) => {
            "failed".to_string()
        }
        ByteStreamFailure::Failed(details) => format!("failed: {details}"),
    }
}

/// ProcessExitStatus controls shell status; all other results retain their rendering behavior.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn apply_result(
    output: &mut CommandOutput,
    stdout: bool,
    result_is_status: bool,
    wire: Option<golem_rust::schema::wit::wire::TypedSchemaValue>,
) {
    if stdout && !result_is_status {
        return;
    }
    let result = wire
        .as_ref()
        .map(golem_rust::decode_typed_schema_value)
        .transpose()
        .map_err(|error| format!("result: {error:?}"))
        .and_then(|typed| render_result(stdout, result_is_status, typed.as_ref()));
    match result {
        Ok(result) => {
            if output.exit_code == 0 {
                output.exit_code = result.exit_code;
            }
            if let Some(stdout) = result.stdout {
                output.stdout = stdout;
            }
        }
        Err(error) => {
            output.exit_code = 1;
            output.stderr.extend(format!("{error}\n").bytes());
        }
    }
}

fn failure(exit_code: u8, message: impl std::fmt::Display) -> CommandOutput {
    CommandOutput {
        stdout: vec![],
        stderr: format!("{message}\n").into_bytes(),
        exit_code,
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn install(session: &mut Session) -> Result<Vec<String>, String> {
    use golem_rust::bindings::golem::tool::host;
    use golem_schema::tool::wit::decode_tool;
    let mut tools = BTreeMap::new();
    for registered in host::get_all_tools() {
        let tool = decode_tool(registered.definition).map_err(|error| {
            log::error!("tool discovery failed: {error:?}");
            format!("tool discovery failed: {error:?}")
        })?;
        if tools.insert(registered.lookup_name, tool).is_some() {
            return Err("duplicate bound tool name".into());
        }
    }
    install_catalog(session, Catalog::new(tools)?)
}

#[cfg(target_arch = "wasm32")]
fn install_catalog(session: &mut Session, catalog: Catalog) -> Result<Vec<String>, String> {
    let descriptors = catalog
        .tools
        .keys()
        .map(|name| CommandDescriptor {
            name: name.clone(),
            help: format!("Invoke the bound {name} tool; use --help for its command metadata."),
        })
        .collect();
    session.register_commands(descriptors, Arc::new(catalog))
}

#[cfg(target_arch = "wasm32")]
#[path = "transport.rs"]
mod transport;

#[cfg(test)]
mod tests;
