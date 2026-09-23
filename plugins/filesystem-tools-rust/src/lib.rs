use golem_rust::{
    FromSchema, IntoSchema, IntoWire, ToolError, WireSchema, tool_definition, tool_implementation,
};
use std::fs;
use std::path::{Component, Path};

#[derive(Debug, Clone, PartialEq, Eq, IntoSchema, FromSchema, IntoWire, WireSchema)]
pub struct ReadFileResult {
    pub content: String,
    pub start_line: Option<u64>,
    pub end_line: Option<u64>,
    pub total_lines: u64,
    pub truncated_before: bool,
    pub truncated_after: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, IntoSchema, FromSchema, IntoWire, WireSchema)]
pub enum WriteDisposition {
    Created,
    Replaced,
}

#[derive(Debug, Clone, PartialEq, Eq, IntoSchema, FromSchema, IntoWire, WireSchema)]
pub struct WriteFileResult {
    pub disposition: WriteDisposition,
    pub bytes_written: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, IntoSchema, FromSchema, IntoWire, WireSchema)]
pub struct EditFileResult {
    pub replacements: u64,
    pub bytes_before: u64,
    pub bytes_after: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, ToolError)]
pub enum FilesystemToolError {
    #[tool_error(kind = "usage-error", exit_code = 2)]
    UnsafePath(String),
    #[tool_error(kind = "usage-error", exit_code = 2)]
    InvalidRange(String),
    #[tool_error(kind = "usage-error", exit_code = 2)]
    EmptyOldText,
    #[tool_error(kind = "runtime-error", exit_code = 1)]
    StaleEdit(String),
    #[tool_error(kind = "usage-error", exit_code = 2)]
    AmbiguousEdit(String),
    #[tool_error(kind = "runtime-error", exit_code = 1)]
    NotFound(String),
    #[tool_error(kind = "runtime-error", exit_code = 1)]
    NotAFile(String),
    #[tool_error(kind = "runtime-error", exit_code = 1)]
    BinaryFile(String),
    #[tool_error(kind = "runtime-error", exit_code = 1)]
    Io(String),
}

#[tool_definition(version = "0.1.0-rust")]
pub trait ReadFileRust {
    #[command(annotations(
        read_only = true,
        destructive = false,
        idempotent = true,
        open_world = true
    ))]
    fn read_file(
        &self,
        path: String,
        range: Vec<u64>,
    ) -> Result<ReadFileResult, FilesystemToolError>;
}

#[tool_definition(version = "0.1.0-rust")]
pub trait WriteFileRust {
    #[command(annotations(
        read_only = false,
        destructive = true,
        idempotent = true,
        open_world = true
    ))]
    fn write_file(
        &self,
        path: String,
        content: String,
        create_parent_directories: bool,
    ) -> Result<WriteFileResult, FilesystemToolError>;
}

#[tool_definition(version = "0.1.0-rust")]
pub trait EditFileRust {
    #[command(annotations(
        read_only = false,
        destructive = true,
        idempotent = false,
        open_world = true
    ))]
    fn edit_file(
        &self,
        path: String,
        old_text: String,
        new_text: String,
    ) -> Result<EditFileResult, FilesystemToolError>;
}

struct ReadFileRustImpl;
struct WriteFileRustImpl;
struct EditFileRustImpl;

#[tool_implementation]
impl ReadFileRust for ReadFileRustImpl {
    fn read_file(
        &self,
        path: String,
        range: Vec<u64>,
    ) -> Result<ReadFileResult, FilesystemToolError> {
        validate_path(&path)?;
        let content = read_text_file(&path)?;
        let (start_line, end_line) = match range.as_slice() {
            [] => (None, None),
            [start] => (Some(*start), None),
            [start, end] => (Some(*start), Some(*end)),
            _ => {
                return Err(invalid_range(
                    &path,
                    "range must contain zero, one, or two line numbers",
                ));
            }
        };
        slice_lines(&path, &content, start_line, end_line)
    }
}

#[tool_implementation]
impl WriteFileRust for WriteFileRustImpl {
    fn write_file(
        &self,
        path: String,
        content: String,
        create_parent_directories: bool,
    ) -> Result<WriteFileResult, FilesystemToolError> {
        validate_path(&path)?;
        let file_path = Path::new(&path);
        let disposition = match fs::metadata(file_path) {
            Ok(metadata) if metadata.is_file() => WriteDisposition::Replaced,
            Ok(_) => return Err(FilesystemToolError::NotAFile(path)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => WriteDisposition::Created,
            Err(error) => return Err(io_error(&path, error)),
        };

        if create_parent_directories
            && let Some(parent) = file_path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent).map_err(|error| io_error(&path, error))?;
        }
        fs::write(file_path, content.as_bytes()).map_err(|error| io_error(&path, error))?;
        Ok(WriteFileResult {
            disposition,
            bytes_written: content.len() as u64,
        })
    }
}

#[tool_implementation]
impl EditFileRust for EditFileRustImpl {
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

fn decode_text(path: &str, bytes: Vec<u8>) -> Result<String, FilesystemToolError> {
    if bytes.contains(&0) {
        return Err(FilesystemToolError::BinaryFile(path.to_string()));
    }
    String::from_utf8(bytes).map_err(|_| {
        FilesystemToolError::BinaryFile(format!("file '{path}' contains invalid UTF-8"))
    })
}

fn slice_lines(
    path: &str,
    content: &str,
    requested_start: Option<u64>,
    requested_end: Option<u64>,
) -> Result<ReadFileResult, FilesystemToolError> {
    if requested_start == Some(0) || requested_end == Some(0) {
        return Err(invalid_range(path, "line numbers are 1-based"));
    }
    if let (Some(start), Some(end)) = (requested_start, requested_end)
        && start > end
    {
        return Err(invalid_range(path, "start_line must not exceed end_line"));
    }

    let lines: Vec<&str> = content.split_inclusive('\n').collect();
    let total_lines = lines.len() as u64;
    if let Some(start) = requested_start
        && start > total_lines
    {
        return Err(invalid_range(
            path,
            &format!("start_line {start} exceeds total line count {total_lines}"),
        ));
    }

    let effective_start = requested_start.unwrap_or(1);
    let effective_end = requested_end.unwrap_or(total_lines).min(total_lines);
    let selected = if total_lines == 0 || effective_start > effective_end {
        String::new()
    } else {
        lines[(effective_start - 1) as usize..effective_end as usize].concat()
    };

    Ok(ReadFileResult {
        content: selected,
        start_line: (total_lines > 0).then_some(effective_start),
        end_line: (total_lines > 0).then_some(effective_end),
        total_lines,
        truncated_before: total_lines > 0 && effective_start > 1,
        truncated_after: effective_end < total_lines,
    })
}

fn invalid_range(path: &str, reason: &str) -> FilesystemToolError {
    FilesystemToolError::InvalidRange(format!("invalid range for '{path}': {reason}"))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_ranges_are_inclusive_and_preserve_crlf() {
        let result = slice_lines("file", "one\r\ntwo\r\nthree", Some(2), Some(9)).unwrap();
        assert_eq!(result.content, "two\r\nthree");
        assert_eq!(result.start_line, Some(2));
        assert_eq!(result.end_line, Some(3));
        assert_eq!(result.total_lines, 3);
        assert!(result.truncated_before);
        assert!(!result.truncated_after);
    }

    #[test]
    fn empty_file_has_no_lines_and_out_of_range_start_is_rejected() {
        let result = slice_lines("file", "", None, None).unwrap();
        assert_eq!(result.total_lines, 0);
        assert_eq!(result.content, "");
        assert!(matches!(
            slice_lines("file", "a\n", Some(2), None),
            Err(FilesystemToolError::InvalidRange(_))
        ));
    }

    #[test]
    fn invalid_and_descending_ranges_are_rejected() {
        assert!(slice_lines("file", "a", Some(0), None).is_err());
        assert!(slice_lines("file", "a\nb", Some(2), Some(1)).is_err());
    }

    #[test]
    fn binary_and_invalid_utf8_are_rejected() {
        assert!(matches!(
            decode_text("file", b"a\0b".to_vec()),
            Err(FilesystemToolError::BinaryFile(_))
        ));
        assert!(matches!(
            decode_text("file", vec![0xff]),
            Err(FilesystemToolError::BinaryFile(_))
        ));
    }

    #[test]
    fn unsafe_paths_are_rejected_but_absolute_paths_are_allowed() {
        assert!(validate_path("").is_err());
        assert!(validate_path("/").is_err());
        assert!(validate_path("a/../b").is_err());
        assert!(validate_path("bad\0path").is_err());
        assert!(validate_path("/workspace/file.txt").is_ok());
        assert!(validate_path("workspace/file.txt").is_ok());
    }

    #[test]
    fn replacement_requires_exactly_one_match() {
        assert!(matches!(
            replace_exactly_once("file", "abc", "missing", "x"),
            Err(FilesystemToolError::StaleEdit(_))
        ));
        let ambiguous = replace_exactly_once("file", "abc abc", "abc", "x").unwrap_err();
        assert_eq!(
            ambiguous,
            FilesystemToolError::AmbiguousEdit(
                "text to replace occurs 2 times in 'file'".to_string()
            )
        );
        assert!(matches!(
            replace_exactly_once("file", "aaa", "aa", "x"),
            Err(FilesystemToolError::AmbiguousEdit(_))
        ));
        assert_eq!(
            replace_exactly_once("file", "a\r\nb\r\nc", "b", "B").unwrap(),
            "a\r\nB\r\nc"
        );
    }
}
