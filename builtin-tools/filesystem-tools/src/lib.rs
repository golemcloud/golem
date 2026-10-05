use golem_rust::ToolError;
use std::path::{Component, Path};

mod bounded_stream;
mod discovery;
mod edit_file;
mod grep;
mod ls;
mod path_policy;
mod read_file;
mod write_file;

pub use discovery::*;
pub use edit_file::*;
pub use grep::*;
pub use ls::*;
pub use read_file::*;
pub use write_file::*;

#[derive(Debug, Clone, PartialEq, Eq, ToolError)]
pub enum FilesystemToolError {
    /// The path is empty, contains NUL or a parent component, or does not identify a file.
    #[tool_error(kind = "usage-error", exit_code = 2)]
    UnsafePath(String),
    /// A line bound or continuation cursor is invalid.
    #[tool_error(kind = "usage-error", exit_code = 2)]
    InvalidRange(String),
    /// A discovery option or bound is invalid.
    #[tool_error(kind = "usage-error", exit_code = 2)]
    InvalidOptions(String),
    /// A grep expression is empty, malformed, or exceeds its compile limits.
    #[tool_error(kind = "usage-error", exit_code = 2)]
    InvalidPattern(String),
    /// A path filter is malformed or exceeds its limits.
    #[tool_error(kind = "usage-error", exit_code = 2)]
    InvalidGlob(String),
    /// A continuation does not match the query or no longer identifies valid state.
    #[tool_error(kind = "usage-error", exit_code = 2)]
    InvalidCursor(String),
    /// `old_text` must contain at least one character.
    #[tool_error(kind = "usage-error", exit_code = 2)]
    EmptyOldText,
    /// `old_text` was not found, so no edit was made.
    #[tool_error(kind = "runtime-error", exit_code = 1)]
    StaleEdit(String),
    /// More than one occurrence matched, so no edit was made.
    #[tool_error(kind = "usage-error", exit_code = 2)]
    AmbiguousEdit(String),
    /// The caller-supplied path does not exist.
    #[tool_error(kind = "runtime-error", exit_code = 1)]
    NotFound(String),
    /// The caller-supplied path exists but is not a regular file.
    #[tool_error(kind = "runtime-error", exit_code = 1)]
    NotAFile(String),
    /// The caller-supplied path exists but is not a directory.
    #[tool_error(kind = "runtime-error", exit_code = 1)]
    NotADirectory(String),
    /// The selected discovery root is neither a regular file nor a directory.
    #[tool_error(kind = "runtime-error", exit_code = 1)]
    UnsupportedRoot(String),
    /// A selected root or ancestor is a symbolic link; discovery never follows links.
    #[tool_error(kind = "runtime-error", exit_code = 1)]
    SymlinkPath(String),
    /// The traversed file bytes contain NUL or invalid UTF-8.
    #[tool_error(kind = "runtime-error", exit_code = 1)]
    BinaryFile(String),
    /// The underlying filesystem operation failed.
    #[tool_error(kind = "runtime-error", exit_code = 1)]
    Io(String),
}

fn validate_path(path: &str) -> Result<(), FilesystemToolError> {
    if path.is_empty() {
        return Err(invalid_path(path, "path must not be empty"));
    }
    if Path::new(path) == Path::new("/") {
        return Err(invalid_path(path, "must identify a file"));
    }
    if path.contains('\0') {
        return Err(invalid_path(path, "path must not contain NUL bytes"));
    }
    if Path::new(path)
        .components()
        .any(|component| component == Component::ParentDir)
    {
        return Err(invalid_path(
            path,
            "parent (`..`) components are not allowed",
        ));
    }
    Ok(())
}

fn invalid_path(path: &str, reason: &str) -> FilesystemToolError {
    FilesystemToolError::UnsafePath(format!("path '{path}' {reason}"))
}

fn io_error(path: &str, error: std::io::Error) -> FilesystemToolError {
    FilesystemToolError::Io(format!("filesystem operation on '{path}' failed: {error}"))
}

fn decode_text(path: &str, bytes: Vec<u8>) -> Result<String, FilesystemToolError> {
    if bytes.contains(&0) {
        return Err(FilesystemToolError::BinaryFile(path.to_string()));
    }
    String::from_utf8(bytes).map_err(|_| {
        FilesystemToolError::BinaryFile(format!("file '{path}' contains invalid UTF-8"))
    })
}
