//! Project owner bindings into shell commands without copying the CLI argument grammar.

#[cfg(target_arch = "wasm32")]
use bash_shell::commands::CommandDescriptor;
use bash_shell::commands::{CommandFuture, CommandInvoker, CommandOutput, PreparedCommand};
#[cfg(target_arch = "wasm32")]
use bash_shell::session::Session;
use golem_schema::TypedSchemaValue;
use golem_schema::tool::{
    Tool,
    argv::{self, ParsedToolArguments},
};
use std::collections::BTreeMap;
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
        }
        Ok(Self { tools })
    }
}

impl Catalog {
    fn prepare_invocation(&self, name: &str, args: &[String]) -> Result<Invocation, CommandOutput> {
        let tool = self
            .tools
            .get(name)
            .ok_or_else(|| failure(127, "tool is not bound"))?;
        let parsed = argv::parse(tool, args).map_err(|message| {
            // The shared parser forbids constructing opaque capabilities from text.
            let message = if message.contains("capabilit") || message.contains("secret") {
                format!("secret and other capability arguments are unsupported: {message}")
            } else {
                message
            };
            failure(2, message)
        })?;
        match parsed {
            ParsedToolArguments::Help(text) => Err(CommandOutput {
                stdout: text.into_bytes(),
                stderr: vec![],
                exit_code: 0,
            }),
            ParsedToolArguments::Invoke {
                command_path,
                input,
            } => {
                let index = tool
                    .command_index_by_path(&command_path)
                    .ok_or_else(|| failure(2, "unknown command path"))?;
                let body = tool.commands.nodes[index]
                    .body
                    .as_ref()
                    .ok_or_else(|| failure(2, "command has no body"))?;
                Ok(Invocation {
                    name: name.into(),
                    path: command_path,
                    input: *input,
                    stdin: body.stdin.is_some(),
                    stdout: body.stdout.is_some(),
                    errors: body
                        .errors
                        .iter()
                        .map(|error| (error.name.clone(), error.exit_code))
                        .collect(),
                })
            }
        }
    }
}

impl CommandInvoker for Catalog {
    fn prepare(
        &self,
        name: &str,
        args: &[String],
    ) -> Result<Box<dyn PreparedCommand>, CommandOutput> {
        self.prepare_invocation(name, args)
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
    use golem_rust::schema::{
        render::json_value::to_json_value_redacted,
        wit::wire::{ToolError, ToolRpcError},
    };
    match error {
        ToolRpcError::Denied(message) => (3, message),
        ToolRpcError::NotFound(message) => (127, message),
        ToolRpcError::Cancelled => (130, "Cancelled".into()),
        ToolRpcError::RemoteToolError(
            ToolError::InvalidInput(message) | ToolError::ConstraintViolation(message),
        ) => (2, message),
        ToolRpcError::RemoteToolError(ToolError::CustomError(custom)) => {
            let code = declared.get(&custom.name).copied().unwrap_or(1);
            let payload = golem_rust::decode_typed_schema_value(&custom.payload)
                .ok()
                .and_then(|typed| {
                    to_json_value_redacted(typed.graph(), &typed.graph().root, typed.value()).ok()
                });
            let message = match payload {
                Some(payload) => format!("tool error: {}: {payload}", custom.name),
                None => format!("tool error: {}", custom.name),
            };
            (code, message)
        }
        ToolRpcError::ProtocolError(message)
        | ToolRpcError::RemoteInternalError(message)
        | ToolRpcError::ResourceExhausted(message) => (1, message),
        other => (1, format!("{other:?}")),
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
