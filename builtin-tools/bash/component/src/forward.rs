//! Gives part of the work of a local command to a built-in file tool that is bound to the agent:
//! `cat` reads a file through `read-file`, `tee` writes a file through `write-file`, and `sed -i`
//! changes a file through `edit-file`.
//!
//! A tool does all of that work or nothing. When it does nothing, the command does the work
//! itself, so a script sees the same result with and without the tool.

use golem_rust::IntoSchema;
use golem_schema::{SchemaGraph, SchemaType, tool::Tool};
#[cfg(target_arch = "wasm32")]
use std::collections::BTreeMap;
#[cfg(target_arch = "wasm32")]
use std::sync::atomic::{AtomicBool, Ordering};

mod edit_file;
mod read_file;
mod write_file;

/// Installs each built-in file tool of `tools` in `session`, for the local command that uses it.
#[cfg(target_arch = "wasm32")]
pub(crate) fn install(session: &mut bash_shell::session::Session, tools: &BTreeMap<String, Tool>) {
    use std::sync::Arc;
    if let Some(reader) = read_file::ReadFileTool::bound(tools) {
        session.forward_file_reads(Arc::new(reader));
    }
    if let Some(writer) = write_file::WriteFileTool::bound(tools) {
        session.forward_file_writes(Arc::new(writer));
    }
    if let Some(editor) = edit_file::EditFileTool::bound(tools) {
        session.forward_file_edits(Arc::new(editor));
    }
}

/// Whether `tool` is one command with no stream that needs the filesystem, takes a value of `A`
/// and returns a value of `R`. A call with an `A` is then a call that the tool expects, and its
/// result is an `R`. Any tool can be bound under the name of a built-in tool.
fn takes<A: IntoSchema, R: IntoSchema>(tool: &Tool, read_only: bool) -> bool {
    let [command] = tool.commands.nodes.as_slice() else {
        return false;
    };
    let Some(body) = &command.body else {
        return false;
    };
    let Some(result) = &body.result else {
        return false;
    };
    let (Ok(input), Ok(arguments), Ok(returned)) = (
        tool.canonical_input_record_schema(0),
        golem_schema::try_into_schema_graph::<A>(),
        golem_schema::try_into_schema_graph::<R>(),
    ) else {
        return false;
    };
    tool.requires_filesystem
        && command.subcommands.is_empty()
        && body
            .annotations
            .as_ref()
            .is_some_and(|notes| notes.read_only == read_only)
        && body.stdin.is_none()
        && body.stdout.is_none()
        && body.stderr.is_none()
        && same_shape((&input.root, &input), (&arguments.root, &arguments))
        && same_shape((&result.type_, &tool.schema), (&returned.root, &returned))
}

/// Whether two types, each with the graph that has its definitions, have the same structure: the
/// same fields and cases with the same names, in the same order. The names and the ids of the
/// definitions are not compared, because the input record of a tool is not a definition, and a
/// type of this crate gets its name and its id from its Rust path by default.
fn same_shape(left: (&SchemaType, &SchemaGraph), right: (&SchemaType, &SchemaGraph)) -> bool {
    let (Some(left_type), Some(right_type)) = (resolved(left), resolved(right)) else {
        return false;
    };
    let same = |left_type, right_type| same_shape((left_type, left.1), (right_type, right.1));
    match (left_type, right_type) {
        (SchemaType::Bool { .. }, SchemaType::Bool { .. })
        | (SchemaType::U32 { .. }, SchemaType::U32 { .. })
        | (SchemaType::U64 { .. }, SchemaType::U64 { .. })
        | (SchemaType::String { .. }, SchemaType::String { .. }) => true,
        (SchemaType::Option { inner: left, .. }, SchemaType::Option { inner: right, .. }) => {
            same(left, right)
        }
        (SchemaType::List { element: left, .. }, SchemaType::List { element: right, .. }) => {
            same(left, right)
        }
        (SchemaType::Enum { cases: left, .. }, SchemaType::Enum { cases: right, .. }) => {
            left == right
        }
        (SchemaType::Record { fields: left, .. }, SchemaType::Record { fields: right, .. }) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|(left, right)| left.name == right.name && same(&left.body, &right.body))
        }
        (SchemaType::Variant { cases: left, .. }, SchemaType::Variant { cases: right, .. }) => {
            left.len() == right.len()
                && left.iter().zip(right).all(|(left, right)| {
                    left.name == right.name
                        && match (&left.payload, &right.payload) {
                            (None, None) => true,
                            (Some(left), Some(right)) => same(left, right),
                            _ => false,
                        }
                })
        }
        _ => false,
    }
}

/// The type itself, or the body of the definition that it refers to.
fn resolved<'a>((type_, graph): (&'a SchemaType, &'a SchemaGraph)) -> Option<&'a SchemaType> {
    match type_ {
        SchemaType::Ref { id, .. } => Some(&graph.lookup(id)?.body),
        other => Some(other),
    }
}

/// Why a call of a bound tool gave no result.
#[cfg(target_arch = "wasm32")]
struct Failure {
    /// The name of the error, when the tool itself returned one.
    error: Option<String>,
    /// The text for the log.
    reason: String,
}

/// A bound tool that local commands call, until it fails for a reason that is not about one
/// file.
#[cfg(target_arch = "wasm32")]
struct Bound {
    /// The name under which the built-in tool is bound.
    name: &'static str,
    /// Set by the first such failure, so that each later file does not pay for a failed call.
    unusable: AtomicBool,
}

#[cfg(target_arch = "wasm32")]
impl Bound {
    fn new(name: &'static str) -> Self {
        Self {
            name,
            unusable: AtomicBool::new(false),
        }
    }

    /// Whether a local command can call the tool now.
    ///
    /// The body of a tool call in flight can wait for this script, to take its output for
    /// example, and the body of a file tool waits for that body when both need the filesystem.
    /// A call that blocks the script would then never end.
    fn usable(&self) -> bool {
        !self.unusable.load(Ordering::Relaxed) && !crate::adapter::tool_call_in_flight()
    }

    /// Stops the use of the tool for the rest of this call of the bash tool.
    fn give_up(&self, reason: &str) {
        self.unusable.store(true, Ordering::Relaxed);
        log::warn!(
            "{} is not used for the rest of this call: {reason}",
            self.name
        );
    }

    /// Makes one call of the tool. The script stands still until the tool has answered, as it
    /// does while a command works on a file itself.
    fn call<A, R>(&self, arguments: A) -> Result<R, Failure>
    where
        A: golem_rust::IntoTypedSchemaValue,
        R: golem_rust::FromSchema,
    {
        use golem_rust::bindings::golem::tool::host;
        use golem_rust::decode_typed_schema_value_owned;
        use golem_rust::schema::wit::wire::{ToolError, ToolRpcError};

        let other = |reason: String| Failure {
            error: None,
            reason,
        };
        let failed = |error: ToolRpcError| Failure {
            error: match &error {
                ToolRpcError::RemoteToolError(ToolError::CustomError(custom)) => {
                    Some(custom.name.clone())
                }
                _ => None,
            },
            reason: crate::adapter::rpc_failure(error, &BTreeMap::new()).1,
        };
        let arguments = arguments
            .into_typed_schema_value()
            .map_err(|error| other(format!("arguments: {error:?}")))?;
        let arguments = golem_rust::encode_typed_schema_value(&arguments)
            .map_err(|error| other(format!("arguments: {error:?}")))?;
        // `create` gives an error where `new` traps, and a trap ends the whole call.
        let rpc = host::ToolRpc::create(self.name).map_err(failed)?;
        let result = crate::execution::block_on(async move {
            rpc.invoke_and_await(Vec::new(), arguments, None, None, None)
                .await
        })
        .map_err(failed)?;
        let value = result.result.ok_or_else(|| other("no result".into()))?;
        let value = decode_typed_schema_value_owned(value)
            .map_err(|error| other(format!("result: {error}")))?;
        R::from_value(value.value()).map_err(|error| other(format!("result: {error}")))
    }
}

#[cfg(test)]
mod tests;
