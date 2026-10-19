//! Project owner bindings into shell commands without copying the CLI argument grammar.

#[cfg(target_arch = "wasm32")]
use bash_shell::commands::CommandDescriptor;
use bash_shell::commands::{CommandFuture, CommandInvoker, CommandOutput, PreparedCommand};
#[cfg(target_arch = "wasm32")]
use bash_shell::session::Session;
use golem_schema::tool::{
    ProcessExitStatus, Tool,
    argv::{self, ParsedToolArguments},
};
use golem_schema::{IntoSchema, SchemaGraph, SchemaType, TypedSchemaValue};
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
        let parsed = argv::parse_with_variables(tool, args, variables).map_err(|message| {
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
                    stderr: body.stderr.is_some(),
                    result_is_status: body
                            .result
                            .as_ref()
                            .is_some_and(|result| matches!(&result.type_, SchemaType::Ref { id, .. } if *id == ProcessExitStatus::type_id())),
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

fn process_status_type(graph: &SchemaGraph, root: &SchemaType) -> Result<bool, String> {
    if !matches!(root, SchemaType::Ref { id, .. } if *id == ProcessExitStatus::type_id()) {
        return Ok(false);
    }
    let definition = graph.lookup(&ProcessExitStatus::type_id());
    if let Some(definition) = definition
        && let SchemaType::Record { fields, .. } = &definition.body
        && fields.len() == 1
        && fields[0].name == "exit_code"
        && matches!(
            fields[0].body,
            SchemaType::U8 {
                restrictions: None,
                ..
            }
        )
    {
        return Ok(true);
    }
    Err("ProcessExitStatus must define exactly one exit_code: u8 field".into())
}

/// ProcessExitStatus controls shell status; all other results retain their rendering behavior.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn apply_result(
    output: &mut CommandOutput,
    stdout: bool,
    result_is_status: bool,
    wire: Option<golem_rust::schema::wit::wire::TypedSchemaValue>,
) {
    let result = (|| -> Result<(), String> {
        if stdout && !result_is_status {
            return Ok(());
        }
        let Some(wire) = wire else {
            return if result_is_status {
                Err("result: missing ProcessExitStatus".into())
            } else {
                Ok(())
            };
        };
        let typed = golem_rust::decode_typed_schema_value(&wire)
            .map_err(|error| format!("result: {error:?}"))?;
        if result_is_status {
            if !process_status_type(typed.graph(), typed.root_type())? {
                return Err("result: expected named ProcessExitStatus".into());
            }
            let golem_schema::SchemaValue::Record { fields } = typed.value() else {
                return Err("result: expected ProcessExitStatus record".into());
            };
            let [golem_schema::SchemaValue::U8(status)] = fields.as_slice() else {
                return Err("result: expected exactly one u8 exit status value".into());
            };
            if output.exit_code == 0 {
                output.exit_code = *status;
            }
        } else {
            let json = golem_rust::schema::render::json_value::to_json_value_redacted(
                typed.graph(),
                typed.root_type(),
                typed.value(),
            )
            .map_err(|error| format!("result: {error:?}"))?;
            output.stdout = match json {
                serde_json::Value::String(text) => text.into_bytes(),
                value => format!("{value}\n").into_bytes(),
            };
        }
        Ok(())
    })();
    if let Err(error) = result {
        output.exit_code = 1;
        output.stderr.extend(format!("{error}\n").bytes());
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
