//! Takes the content of a file from the bound `read-file` tool for a local command.
//!
//! `read-file` returns a text file in pages. This module reads each page and joins them, so the
//! command gets all bytes of the file or nothing.

use golem_rust::{FromSchema, IntoSchema};
use golem_schema::tool::Tool;
#[cfg(target_arch = "wasm32")]
use std::collections::BTreeMap;
use std::path::{Component, Path};

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
#[derive(IntoSchema)]
#[schema(rename_all = "kebab-case")]
#[cfg_attr(
    not(target_arch = "wasm32"),
    allow(dead_code, reason = "only a Golem host takes a call")
)]
struct Arguments {
    path: String,
    start_line: Option<u64>,
    end_line: Option<u64>,
    cursor: Option<Cursor>,
}

/// One result of `read-file`.
#[derive(Debug, PartialEq, Eq, IntoSchema, FromSchema)]
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

/// Reads files through the bound `read-file` tool.
#[cfg(target_arch = "wasm32")]
pub(super) struct ReadFileTool(super::Bound);

#[cfg(target_arch = "wasm32")]
impl ReadFileTool {
    /// A reader for a caller that has `tools` bound, if the built-in `read-file` is one of them.
    pub(super) fn bound(tools: &BTreeMap<String, Tool>) -> Option<Self> {
        tools
            .get(TOOL)
            .is_some_and(is_read_file)
            .then(|| Self(super::Bound::new(TOOL)))
    }

    /// Makes one call of the tool for the page at `cursor`.
    fn fetch(&self, path: &str, cursor: Option<Cursor>) -> Result<Page, Refusal> {
        self.0
            .call(Arguments {
                path: path.to_owned(),
                start_line: None,
                end_line: None,
                cursor,
            })
            .map_err(|failure| match failure.error.as_deref() {
                Some(NOT_TEXT) => Refusal::NotText,
                _ => Refusal::Other(failure.reason),
            })
    }
}

#[cfg(target_arch = "wasm32")]
impl bash_shell::commands::FileReader for ReadFileTool {
    fn read(&self, path: &Path, len: u64) -> Option<Vec<u8>> {
        if !self.0.usable() {
            return None;
        }
        let path = request(path, len)?;
        match read_all(len, |cursor| self.fetch(path, cursor)) {
            Ok(bytes) => bytes,
            Err(reason) => {
                self.0.give_up(&reason);
                None
            }
        }
    }
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

/// Whether `tool` has the command, the arguments and the result of the built-in `read-file`.
fn is_read_file(tool: &Tool) -> bool {
    super::takes::<Arguments, Page>(tool, true)
}

#[cfg(test)]
mod tests;
