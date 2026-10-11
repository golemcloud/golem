//! Gives a change of the content of a file to the bound `edit-file` tool for a local command.
//!
//! `edit-file` puts one text in the place of another text that is in the file one time. This
//! module finds such a pair of texts for the content before and after the change.

use golem_rust::{FromSchema, IntoSchema};
use golem_schema::tool::Tool;
#[cfg(target_arch = "wasm32")]
use std::collections::BTreeMap;
use std::path::{Component, Path};

/// The name under which the built-in tool is bound.
#[cfg(target_arch = "wasm32")]
const TOOL: &str = "edit-file";

/// The largest content of a file, before the change and after it, that goes to the tool. The
/// command holds both in memory, and the two texts of the call go into the oplog of the agent.
const MAX_BYTES: usize = 1024 * 1024;

/// The arguments of `edit-file`.
#[derive(IntoSchema)]
#[schema(rename_all = "kebab-case")]
#[cfg_attr(
    not(target_arch = "wasm32"),
    allow(dead_code, reason = "only a Golem host takes a call")
)]
struct Arguments {
    path: String,
    old_text: String,
    new_text: String,
}

/// The result of `edit-file`.
#[derive(IntoSchema, FromSchema)]
#[cfg_attr(
    not(target_arch = "wasm32"),
    allow(dead_code, reason = "only a Golem host takes a call")
)]
struct Edited {
    replacements: u64,
    bytes_before: u64,
    bytes_after: u64,
}

/// One call of the tool: the path of the file, the text to replace and the text that replaces
/// it.
#[derive(Debug, PartialEq, Eq)]
struct Edit<'a> {
    path: &'a str,
    old_text: &'a str,
    new_text: &'a str,
}

/// Changes files through the bound `edit-file` tool.
#[cfg(target_arch = "wasm32")]
pub(super) struct EditFileTool(super::Bound);

#[cfg(target_arch = "wasm32")]
impl EditFileTool {
    /// An editor for a caller that has `tools` bound, if the built-in `edit-file` is one of
    /// them.
    pub(super) fn bound(tools: &BTreeMap<String, Tool>) -> Option<Self> {
        tools
            .get(TOOL)
            .is_some_and(is_edit_file)
            .then(|| Self(super::Bound::new(TOOL)))
    }
}

#[cfg(target_arch = "wasm32")]
impl bash_shell::commands::FileEditor for EditFileTool {
    fn max_len(&self) -> usize {
        MAX_BYTES
    }

    fn edit(&self, path: &Path, old: &[u8], new: &[u8]) -> bool {
        if !self.0.usable() {
            return false;
        }
        let Some(edit) = request(path, old, new) else {
            return false;
        };
        let edited = self.0.call::<_, Edited>(Arguments {
            path: edit.path.to_owned(),
            old_text: edit.old_text.to_owned(),
            new_text: edit.new_text.to_owned(),
        });
        let length = |bytes: &[u8]| u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        match edited {
            Ok(edited)
                if edited.replacements == 1
                    && edited.bytes_before == length(old)
                    && edited.bytes_after == length(new) =>
            {
                true
            }
            Ok(edited) => {
                self.0.give_up(&format!(
                    "{} replacements, from {} to {} bytes",
                    edited.replacements, edited.bytes_before, edited.bytes_after
                ));
                false
            }
            Err(failure) => {
                self.0.give_up(&failure.reason);
                false
            }
        }
    }
}

/// The call that changes the content of the file at `path` from `old` to `new`, or `None` for a
/// change that the tool is not asked to make: content that is above the size limit or is not
/// UTF-8, a file that the tool does not read as text (it has a NUL byte) or that has no content
/// to replace, and a path that the tool refuses (a path that is not UTF-8 or has a parent
/// component).
fn request<'a>(path: &'a Path, old: &'a [u8], new: &'a [u8]) -> Option<Edit<'a>> {
    if old.is_empty()
        || old.len() > MAX_BYTES
        || new.len() > MAX_BYTES
        || old.contains(&0)
        || path.components().any(|part| part == Component::ParentDir)
    {
        return None;
    }
    let (old, new) = (
        std::str::from_utf8(old).ok()?,
        std::str::from_utf8(new).ok()?,
    );
    let (old_text, new_text) = replacement(old, new);
    Some(Edit {
        path: path.to_str()?,
        old_text,
        new_text,
    })
}

/// How many times the text that changes is made longer by one line on each side, to get a text
/// that is in the content one time, before the whole content is sent.
const WIDENINGS: usize = 3;

/// A text of `old`, which is not empty, and the text that replaces it to give `new`. The text
/// starts in `old` at one character and at no other, as the tool needs it.
///
/// The text is what is between the first and the last difference of `old` and `new`. When that
/// text is empty or is also in another place, it is made longer by whole lines.
fn replacement<'a>(old: &'a str, new: &'a str) -> (&'a str, &'a str) {
    let shorter = old.len().min(new.len());
    let mut start = old
        .bytes()
        .zip(new.bytes())
        .take_while(|(old, new)| old == new)
        .count();
    while !old.is_char_boundary(start) {
        start -= 1;
    }
    let mut after = old
        .bytes()
        .rev()
        .zip(new.bytes().rev())
        .take(shorter - start)
        .take_while(|(old, new)| old == new)
        .count();
    while !old.is_char_boundary(old.len() - after) {
        after -= 1;
    }
    let mut end = old.len() - after;
    for _ in 0..=WIDENINGS {
        let text = &old[start..end];
        if is_in_one_place(old, text) {
            return (text, &new[start..new.len() - (old.len() - end)]);
        }
        // The start of the line before, and the end of the line after. The byte before `start`
        // can be inside a character, so the line start is looked for in the bytes.
        start = old.as_bytes()[..start.saturating_sub(1)]
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |at| at + 1);
        end = old[end..].find('\n').map_or(old.len(), |at| end + at + 1);
    }
    (old, new)
}

/// Whether `text` is not empty and starts at one character of `content` and at no other.
fn is_in_one_place(content: &str, text: &str) -> bool {
    let Some(first) = content.find(text).filter(|_| !text.is_empty()) else {
        return false;
    };
    // The next place can start inside the first one.
    let next = first + content[first..].chars().next().map_or(1, char::len_utf8);
    !content[next..].contains(text)
}

/// Whether `tool` has the command, the arguments and the result of the built-in `edit-file`.
fn is_edit_file(tool: &Tool) -> bool {
    super::takes::<Arguments, Edited>(tool, false)
}

#[cfg(test)]
mod tests;
