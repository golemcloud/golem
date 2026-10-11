//! Gives the content of a file to the bound `write-file` tool for a local command.
//!
//! `write-file` makes a text the whole content of a file, in one call.

use golem_rust::{FromSchema, IntoSchema};
use golem_schema::tool::Tool;
#[cfg(target_arch = "wasm32")]
use std::collections::BTreeMap;
use std::path::{Component, Path};

/// The name under which the built-in tool is bound.
#[cfg(target_arch = "wasm32")]
const TOOL: &str = "write-file";

/// The largest content that goes to the tool. The content goes into the oplog of the agent, and
/// the command holds it in memory until its input ends.
const MAX_BYTES: usize = 1024 * 1024;

/// The arguments of `write-file`.
#[derive(IntoSchema)]
#[schema(rename_all = "kebab-case")]
#[cfg_attr(
    not(target_arch = "wasm32"),
    allow(dead_code, reason = "only a Golem host takes a call")
)]
struct Arguments {
    path: String,
    content: String,
    create_parent_directories: bool,
}

/// Whether `write-file` made the file or replaced its content.
#[derive(IntoSchema, FromSchema)]
enum Disposition {
    Created,
    Replaced,
}

/// The result of `write-file`.
#[derive(IntoSchema, FromSchema)]
struct Written {
    #[allow(dead_code, reason = "the record is decoded as a whole")]
    disposition: Disposition,
    #[cfg_attr(
        not(target_arch = "wasm32"),
        allow(dead_code, reason = "only a Golem host takes a call")
    )]
    bytes_written: u64,
}

/// Writes files through the bound `write-file` tool.
#[cfg(target_arch = "wasm32")]
pub(super) struct WriteFileTool(super::Bound);

#[cfg(target_arch = "wasm32")]
impl WriteFileTool {
    /// A writer for a caller that has `tools` bound, if the built-in `write-file` is one of
    /// them.
    pub(super) fn bound(tools: &BTreeMap<String, Tool>) -> Option<Self> {
        tools
            .get(TOOL)
            .is_some_and(is_write_file)
            .then(|| Self(super::Bound::new(TOOL)))
    }
}

#[cfg(target_arch = "wasm32")]
impl bash_shell::commands::FileWriter for WriteFileTool {
    fn max_len(&self) -> usize {
        MAX_BYTES
    }

    fn write(&self, path: &Path, content: &[u8]) -> bool {
        if !self.0.usable() {
            return false;
        }
        let Some((path, text)) = request(path, content) else {
            return false;
        };
        let written = self.0.call::<_, Written>(Arguments {
            path: path.to_owned(),
            content: text.to_owned(),
            create_parent_directories: false,
        });
        let length = u64::try_from(text.len()).unwrap_or(u64::MAX);
        match written {
            Ok(written) if written.bytes_written == length => true,
            Ok(written) => {
                let wrote = written.bytes_written;
                self.0
                    .give_up(&format!("{wrote} of {length} bytes written"));
                false
            }
            Err(failure) => {
                self.0.give_up(&failure.reason);
                false
            }
        }
    }
}

/// The path and the text to give to the tool, or `None` for content that the tool is not asked
/// to write: content that is not UTF-8 or is above the size limit, and a path that the tool
/// refuses (a path that is not UTF-8 or has a parent component).
fn request<'a>(path: &'a Path, content: &'a [u8]) -> Option<(&'a str, &'a str)> {
    if content.len() > MAX_BYTES || path.components().any(|part| part == Component::ParentDir) {
        return None;
    }
    Some((path.to_str()?, std::str::from_utf8(content).ok()?))
}

/// Whether `tool` has the command, the arguments and the result of the built-in `write-file`.
fn is_write_file(tool: &Tool) -> bool {
    super::takes::<Arguments, Written>(tool, false)
}

#[cfg(test)]
mod tests;
