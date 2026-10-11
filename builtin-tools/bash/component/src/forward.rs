//! Takes the content of a file from the bound `read-file` tool for a local command.
//!
//! `read-file` returns a text file in pages. This module reads each page and joins them, so the
//! command gets all bytes of the file or nothing. When it gets nothing, it reads the file itself,
//! so a script sees the same output with and without the tool.

use golem_rust::{FromSchema, IntoSchema};
use golem_schema::{SchemaGraph, SchemaType, tool::Tool};
#[cfg(target_arch = "wasm32")]
use std::collections::BTreeMap;
use std::path::{Component, Path};
#[cfg(target_arch = "wasm32")]
use std::sync::atomic::{AtomicBool, Ordering};

/// The name under which the built-in tool is bound.
#[cfg(target_arch = "wasm32")]
const TOOL: &str = "read-file";
/// The error of the tool for a file that has a NUL byte or is not UTF-8.
#[cfg(target_arch = "wasm32")]
const NOT_TEXT: &str = "binary-file";

/// The largest file that is read through the tool. The content of each page goes into the oplog
/// of the agent, and all pages are held in memory until the last one has arrived.
const MAX_BYTES: u64 = 1024 * 1024;
/// The most calls of the tool for one file. One call returns at most 200 lines, and the script
/// stands still until the last call has returned.
const MAX_CALLS: u64 = 64;

/// The position of the next page, as `read-file` returns it and takes it back.
#[derive(Debug, PartialEq, Eq, IntoSchema, FromSchema)]
#[schema(named = "golem_filesystem_tools.read_file.ReadFileCursor")]
struct Cursor {
    byte_offset: u64,
    line: u64,
}

/// The arguments of `read-file`.
#[cfg(target_arch = "wasm32")]
#[derive(IntoSchema)]
#[schema(rename_all = "kebab-case")]
struct Arguments {
    path: String,
    start_line: Option<u64>,
    end_line: Option<u64>,
    cursor: Option<Cursor>,
}

/// One result of `read-file`.
#[derive(Debug, PartialEq, Eq, FromSchema)]
struct Page {
    content: String,
    #[allow(dead_code, reason = "the record is decoded as a whole")]
    start_line: Option<u64>,
    #[allow(dead_code, reason = "the record is decoded as a whole")]
    end_line: Option<u64>,
    next_cursor: Option<Cursor>,
}

/// Why a call of the tool gave no page.
#[derive(Debug, PartialEq, Eq)]
enum Refusal {
    /// The file is not UTF-8 text. The tool reads other files.
    NotText,
    /// Any other failure, with its text for the log.
    Other(String),
}

/// Reads files through the bound `read-file` tool, until the tool fails for a reason that is not
/// about one file.
#[cfg(target_arch = "wasm32")]
pub(crate) struct ReadFileTool {
    /// Set by the first such failure, so that each later file does not pay for a failed call.
    unusable: AtomicBool,
}

#[cfg(target_arch = "wasm32")]
impl ReadFileTool {
    /// A reader for a caller that has `tools` bound, if the built-in `read-file` is one of them.
    pub(crate) fn bound(tools: &BTreeMap<String, Tool>) -> Option<Self> {
        tools.get(TOOL).is_some_and(is_read_file).then(|| Self {
            unusable: AtomicBool::new(false),
        })
    }
}

#[cfg(target_arch = "wasm32")]
impl bash_shell::commands::FileReader for ReadFileTool {
    fn read(&self, path: &Path, len: u64) -> Option<Vec<u8>> {
        // The body of a tool call in flight can wait for this script, to take its output for
        // example, and the body of `read-file` waits for that body when both need the
        // filesystem. A read that blocks the script would then never end.
        if self.unusable.load(Ordering::Relaxed) || crate::adapter::tool_call_in_flight() {
            return None;
        }
        let path = request(path, len)?;
        match read_all(len, |cursor| fetch(path, cursor)) {
            Ok(bytes) => bytes,
            Err(reason) => {
                self.unusable.store(true, Ordering::Relaxed);
                log::warn!("{TOOL} is not used for the rest of this call: {reason}");
                None
            }
        }
    }
}

/// Makes one call of the tool. The script stands still until the tool has answered, as it does
/// while a command reads a file itself.
#[cfg(target_arch = "wasm32")]
fn fetch(path: &str, cursor: Option<Cursor>) -> Result<Page, Refusal> {
    use golem_rust::bindings::golem::tool::host;
    use golem_rust::schema::wit::wire::{ToolError, ToolRpcError};
    use golem_rust::{IntoTypedSchemaValue, decode_typed_schema_value_owned};

    let failed = |error: ToolRpcError| {
        Refusal::Other(crate::adapter::rpc_failure(error, &BTreeMap::new()).1)
    };
    let arguments = Arguments {
        path: path.to_owned(),
        start_line: None,
        end_line: None,
        cursor,
    }
    .into_typed_schema_value()
    .map_err(|error| Refusal::Other(format!("arguments: {error:?}")))?;
    let arguments = golem_rust::encode_typed_schema_value(&arguments)
        .map_err(|error| Refusal::Other(format!("arguments: {error:?}")))?;
    // `create` gives an error where `new` traps, and a trap ends the whole call.
    let rpc = host::ToolRpc::create(TOOL).map_err(failed)?;
    let result = crate::execution::block_on(async move {
        rpc.invoke_and_await(Vec::new(), arguments, None, None, None)
            .await
    });
    let result = match result {
        Ok(result) => result,
        Err(ToolRpcError::RemoteToolError(ToolError::CustomError(error)))
            if error.name == NOT_TEXT =>
        {
            return Err(Refusal::NotText);
        }
        Err(error) => return Err(failed(error)),
    };
    let value = result
        .result
        .ok_or_else(|| Refusal::Other("no result".into()))?;
    let value = decode_typed_schema_value_owned(value)
        .map_err(|error| Refusal::Other(format!("result: {error}")))?;
    Page::from_value(value.value()).map_err(|error| Refusal::Other(format!("result: {error}")))
}

/// The path to give to the tool for a file of `len` bytes, or `None` for a file that the tool is
/// not asked for: one above the size limit, and one whose path the tool refuses (a path that is
/// not UTF-8 or has a parent component).
fn request(path: &Path, len: u64) -> Option<&str> {
    if len > MAX_BYTES || path.components().any(|part| part == Component::ParentDir) {
        return None;
    }
    path.to_str()
}

/// Reads a file of `len` bytes page by page. `fetch` makes one call of the tool with the cursor
/// of the page before, and with no cursor for the first page.
///
/// - `Ok(Some(bytes))`: all bytes of the file.
/// - `Ok(None)`: the tool does not give this file: it is not text, or it needs more than
///   [`MAX_CALLS`] calls. The first page tells how many calls the file needs, so such a file
///   costs one call.
/// - `Err(reason)`: the tool cannot be used: a call failed, or the pages are not the bytes of
///   the file.
fn read_all(
    len: u64,
    mut fetch: impl FnMut(Option<Cursor>) -> Result<Page, Refusal>,
) -> Result<Option<Vec<u8>>, String> {
    let mut bytes = Vec::new();
    let mut cursor = None;
    for call in 1..=MAX_CALLS {
        let page = match fetch(cursor.take()) {
            Ok(page) => page,
            Err(Refusal::NotText) => return Ok(None),
            Err(Refusal::Other(reason)) => return Err(reason),
        };
        let before = bytes.len();
        bytes.extend_from_slice(page.content.as_bytes());
        let total = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        if total > len {
            return Err(format!("more than the {len} bytes of the file"));
        }
        let Some(next) = page.next_cursor else {
            return if total == len {
                Ok(Some(bytes))
            } else {
                Err(format!("{total} of the {len} bytes of the file"))
            };
        };
        if next.byte_offset != total || (bytes.len() == before && total < len) {
            return Err(format!(
                "a cursor at byte {} after {total} bytes",
                next.byte_offset
            ));
        }
        if call == 1 && len > total.saturating_mul(MAX_CALLS) {
            return Ok(None);
        }
        cursor = Some(next);
    }
    Ok(None)
}

/// Whether `tool` has the commands, arguments and result of the built-in `read-file`, so that a
/// call with [`Cursor`] and no streams is a call that the tool expects, and its result is a
/// [`Page`]. Any tool can be bound under the name `read-file`.
fn is_read_file(tool: &Tool) -> bool {
    let [command] = tool.commands.nodes.as_slice() else {
        return false;
    };
    let Some(body) = &command.body else {
        return false;
    };
    let is_cursor = |type_: &SchemaType| {
        record(type_, &tool.schema).is_some_and(|fields| {
            matches!(fields.as_slice(), [("byte_offset", offset), ("line", line)]
                if is_u64(offset) && is_u64(line))
        })
    };
    let arguments = matches!(body.positionals.fixed.as_slice(), [path, start, end, cursor]
        if path.name == "path" && path.required && is_string(&path.type_)
            && start.name == "start-line" && !start.required && is_u64(&start.type_)
            && end.name == "end-line" && !end.required && is_u64(&end.type_)
            && cursor.name == "cursor" && !cursor.required && is_cursor(&cursor.type_));
    let result = body
        .result
        .as_ref()
        .and_then(|result| record(&result.type_, &tool.schema))
        .is_some_and(|fields| {
            matches!(fields.as_slice(),
                [("content", content), ("start_line", start), ("end_line", end), ("next_cursor", next)]
                if is_string(content)
                    && optional(start).is_some_and(is_u64)
                    && optional(end).is_some_and(is_u64)
                    && optional(next).is_some_and(is_cursor))
        });
    tool.requires_filesystem
        && command.subcommands.is_empty()
        && body
            .annotations
            .as_ref()
            .is_some_and(|notes| notes.read_only)
        && body.stdin.is_none()
        && body.stdout.is_none()
        && body.stderr.is_none()
        && body.options.is_empty()
        && body.flags.is_empty()
        && body.positionals.tail.is_none()
        && arguments
        && result
}

fn is_string(type_: &SchemaType) -> bool {
    matches!(type_, SchemaType::String { .. })
}

fn is_u64(type_: &SchemaType) -> bool {
    matches!(type_, SchemaType::U64 { .. })
}

/// The type inside an option type.
fn optional(type_: &SchemaType) -> Option<&SchemaType> {
    match type_ {
        SchemaType::Option { inner, .. } => Some(inner),
        _ => None,
    }
}

/// The name and type of each field of a record type, which can be a reference to a definition
/// of `graph`.
fn record<'a>(
    type_: &'a SchemaType,
    graph: &'a SchemaGraph,
) -> Option<Vec<(&'a str, &'a SchemaType)>> {
    let type_ = match type_ {
        SchemaType::Ref { id, .. } => &graph.lookup(id)?.body,
        other => other,
    };
    match type_ {
        SchemaType::Record { fields, .. } => Some(
            fields
                .iter()
                .map(|field| (field.name.as_str(), &field.body))
                .collect(),
        ),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
