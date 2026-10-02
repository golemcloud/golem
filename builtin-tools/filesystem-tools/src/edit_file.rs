use crate::{FilesystemToolError, decode_text, io_error, validate_path};
use golem_rust::{
    FromSchema, FromWire, IntoSchema, IntoWire, WireSchema, tool_definition, tool_implementation,
};
use std::fs;

#[derive(Debug, Clone, PartialEq, Eq, IntoSchema, FromSchema, FromWire, IntoWire, WireSchema)]
pub struct EditFileResult {
    /// Number of replacements made; successful edits always report one.
    pub replacements: u64,
    /// File size in bytes before replacement.
    pub bytes_before: u64,
    /// File size in bytes after replacement.
    pub bytes_after: u64,
}

#[tool_definition(version = "0.4.0", requires_filesystem = true)]
pub trait EditFile {
    /// Replaces exactly one occurrence of `old_text` in a known UTF-8 text file. The result reports
    /// replacement and byte counts. Missing text is stale, repeated text is ambiguous, and binary,
    /// missing, unsafe, or non-file paths are errors. File discovery is not performed.
    #[command(annotations(
        read_only = false,
        destructive = true,
        idempotent = false,
        open_world = false
    ))]
    fn edit_file(
        &self,
        path: String,
        old_text: String,
        new_text: String,
    ) -> Result<EditFileResult, FilesystemToolError>;
}

struct EditFileImpl;

#[tool_implementation]
impl EditFile for EditFileImpl {
    fn edit_file(
        &self,
        path: String,
        old_text: String,
        new_text: String,
    ) -> Result<EditFileResult, FilesystemToolError> {
        validate_path(&path)?;
        if old_text.is_empty() {
            return Err(FilesystemToolError::EmptyOldText);
        }
        let content = read_text_file(&path)?;
        let updated = replace_exactly_once(&path, &content, &old_text, &new_text)?;
        let result = EditFileResult {
            replacements: 1,
            bytes_before: content.len() as u64,
            bytes_after: updated.len() as u64,
        };
        fs::write(&path, updated.as_bytes()).map_err(|error| io_error(&path, error))?;
        Ok(result)
    }
}

fn read_text_file(path: &str) -> Result<String, FilesystemToolError> {
    let metadata = fs::metadata(path).map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => FilesystemToolError::NotFound(path.to_string()),
        _ => io_error(path, error),
    })?;
    if !metadata.is_file() {
        return Err(FilesystemToolError::NotAFile(path.to_string()));
    }
    let bytes = fs::read(path).map_err(|error| io_error(path, error))?;
    decode_text(path, bytes)
}

fn replace_exactly_once(
    path: &str,
    content: &str,
    old_text: &str,
    new_text: &str,
) -> Result<String, FilesystemToolError> {
    let mut matches = content
        .char_indices()
        .filter_map(|(offset, _)| content[offset..].starts_with(old_text).then_some(offset));
    let Some(offset) = matches.next() else {
        return Err(FilesystemToolError::StaleEdit(format!(
            "text to replace was not found in '{path}'"
        )));
    };
    let match_count = 1 + matches.count();
    if match_count > 1 {
        return Err(FilesystemToolError::AmbiguousEdit(format!(
            "text to replace occurs {} times in '{path}'",
            match_count
        )));
    }
    let mut result = String::with_capacity(content.len() - old_text.len() + new_text.len());
    result.push_str(&content[..offset]);
    result.push_str(new_text);
    result.push_str(&content[offset + old_text.len()..]);
    Ok(result)
}
