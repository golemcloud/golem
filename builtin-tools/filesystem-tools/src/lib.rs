use golem_rust::{
    FromSchema, FromWire, IntoSchema, IntoWire, ToolError, WireSchema, tool_definition,
    tool_implementation,
};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Component, Path};

mod path_policy;

#[cfg(test)]
use path_policy::*;

const MAX_READ_BYTES: usize = 64 * 1024;
const MAX_READ_LINES: usize = 200;

/// Opaque continuation position returned by `read-file`. Pass it back unchanged to continue.
#[derive(Debug, Clone, PartialEq, Eq, IntoSchema, FromSchema, FromWire, IntoWire, WireSchema)]
pub struct ReadFileCursor {
    /// Zero-based byte position of the next bounded read.
    pub byte_offset: u64,
    /// One-based line containing `byte_offset`.
    pub line: u64,
}

/// A bounded page of UTF-8 file content and the position needed to request the next page.
#[derive(Debug, Clone, PartialEq, Eq, IntoSchema, FromSchema, FromWire, IntoWire, WireSchema)]
pub struct ReadFileResult {
    /// UTF-8 content traversed in the requested line range; it may contain part of a long line.
    pub content: String,
    /// First line represented in `content`, or none when no requested content was reached.
    pub start_line: Option<u64>,
    /// Last line represented in `content`, or none when no requested content was reached.
    pub end_line: Option<u64>,
    /// Continuation for the next bounded call, or none when EOF or `end_line` was reached.
    pub next_cursor: Option<ReadFileCursor>,
}

#[derive(Debug, Clone, PartialEq, Eq, IntoSchema, FromSchema, FromWire, IntoWire, WireSchema)]
pub enum WriteDisposition {
    Created,
    Replaced,
}

#[derive(Debug, Clone, PartialEq, Eq, IntoSchema, FromSchema, FromWire, IntoWire, WireSchema)]
pub struct WriteFileResult {
    /// Whether the call created a new file or replaced an existing file.
    pub disposition: WriteDisposition,
    /// Number of UTF-8 bytes written.
    pub bytes_written: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, IntoSchema, FromSchema, FromWire, IntoWire, WireSchema)]
pub struct EditFileResult {
    /// Number of replacements made; successful edits always report one.
    pub replacements: u64,
    /// File size in bytes before replacement.
    pub bytes_before: u64,
    /// File size in bytes after replacement.
    pub bytes_after: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, ToolError)]
pub enum FilesystemToolError {
    /// The path is empty, contains NUL or a parent component, or does not identify a file.
    #[tool_error(kind = "usage-error", exit_code = 2)]
    UnsafePath(String),
    /// A line bound or continuation cursor is invalid.
    #[tool_error(kind = "usage-error", exit_code = 2)]
    InvalidRange(String),
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
    /// The traversed file bytes contain NUL or invalid UTF-8.
    #[tool_error(kind = "runtime-error", exit_code = 1)]
    BinaryFile(String),
    /// The underlying filesystem operation failed.
    #[tool_error(kind = "runtime-error", exit_code = 1)]
    Io(String),
}

#[tool_definition(version = "0.3.0", requires_filesystem = true)]
pub trait ReadFile {
    /// Reads a page from a known text file. Lines are 1-based and `end_line` is inclusive. Omit
    /// both bounds to read from the beginning. A call examines at most 64 KiB and 200 lines, so a
    /// distant start or oversized line can produce an empty or partial page. When `next_cursor` is
    /// present, call again with the same bounds and that cursor. The path must be supplied by the
    /// caller; this tool does not discover or list files.
    #[command(annotations(
        read_only = true,
        destructive = false,
        idempotent = true,
        open_world = false
    ))]
    fn read_file(
        &self,
        path: String,
        start_line: Option<u64>,
        end_line: Option<u64>,
        cursor: Option<ReadFileCursor>,
    ) -> Result<ReadFileResult, FilesystemToolError>;
}

#[tool_definition(version = "0.3.0", requires_filesystem = true)]
pub trait WriteFile {
    /// Creates or replaces a known UTF-8 text file and reports which occurred and the byte count.
    /// The caller must supply the path; this tool does not discover files. Errors identify unsafe
    /// paths, non-file destinations, and filesystem failures.
    #[command(annotations(
        read_only = false,
        destructive = true,
        idempotent = true,
        open_world = false
    ))]
    fn write_file(
        &self,
        path: String,
        content: String,
        create_parent_directories: bool,
    ) -> Result<WriteFileResult, FilesystemToolError>;
}

#[tool_definition(version = "0.3.0", requires_filesystem = true)]
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

struct ReadFileImpl;
struct WriteFileImpl;
struct EditFileImpl;

#[tool_implementation]
impl ReadFile for ReadFileImpl {
    fn read_file(
        &self,
        path: String,
        start_line: Option<u64>,
        end_line: Option<u64>,
        cursor: Option<ReadFileCursor>,
    ) -> Result<ReadFileResult, FilesystemToolError> {
        validate_path(&path)?;
        read_file_page(&path, start_line, end_line, cursor)
    }
}

#[tool_implementation]
impl WriteFile for WriteFileImpl {
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

fn read_file_page(
    path: &str,
    requested_start: Option<u64>,
    requested_end: Option<u64>,
    cursor: Option<ReadFileCursor>,
) -> Result<ReadFileResult, FilesystemToolError> {
    if requested_start == Some(0) || requested_end == Some(0) {
        return Err(invalid_range(path, "line numbers are 1-based"));
    }
    if let (Some(start), Some(end)) = (requested_start, requested_end)
        && start > end
    {
        return Err(invalid_range(path, "start_line must not exceed end_line"));
    }

    let requested_start = requested_start.unwrap_or(1);
    let cursor = cursor.unwrap_or(ReadFileCursor {
        byte_offset: 0,
        line: 1,
    });
    if cursor.line == 0 {
        return Err(invalid_range(path, "cursor line must be 1-based"));
    }
    if cursor.byte_offset == 0 && cursor.line != 1 {
        return Err(invalid_range(
            path,
            "a cursor at byte offset zero must be on line one",
        ));
    }

    let metadata = fs::metadata(path).map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => FilesystemToolError::NotFound(path.to_string()),
        _ => io_error(path, error),
    })?;
    if !metadata.is_file() {
        return Err(FilesystemToolError::NotAFile(path.to_string()));
    }
    if cursor.byte_offset > metadata.len() {
        return Err(invalid_range(path, "cursor byte offset exceeds file size"));
    }

    let mut file = File::open(path).map_err(|error| io_error(path, error))?;
    file.seek(SeekFrom::Start(cursor.byte_offset))
        .map_err(|error| io_error(path, error))?;
    let remaining = (metadata.len() - cursor.byte_offset).min(MAX_READ_BYTES as u64) as usize;
    let mut bytes = vec![0; remaining];
    file.read_exact(&mut bytes)
        .map_err(|error| io_error(path, error))?;
    if bytes.contains(&0) {
        return Err(FilesystemToolError::BinaryFile(path.to_string()));
    }
    if bytes
        .first()
        .is_some_and(|byte| byte & 0b1100_0000 == 0b1000_0000)
    {
        return Err(invalid_range(
            path,
            "cursor byte offset is not on a UTF-8 character boundary",
        ));
    }

    let at_eof = cursor.byte_offset + bytes.len() as u64 == metadata.len();
    match std::str::from_utf8(&bytes) {
        Ok(_) => {}
        Err(error) if error.error_len().is_none() && !at_eof => {
            bytes.truncate(error.valid_up_to());
        }
        Err(_) => {
            return Err(FilesystemToolError::BinaryFile(format!(
                "file '{path}' contains invalid UTF-8"
            )));
        }
    }
    if !at_eof && bytes.last() == Some(&b'\r') {
        bytes.pop();
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| {
        FilesystemToolError::BinaryFile(format!("file '{path}' contains invalid UTF-8"))
    })?;

    let mut cut = text.len();
    let mut newline_count = 0;
    for (offset, byte) in text.bytes().enumerate() {
        if byte == b'\n' {
            newline_count += 1;
            if newline_count == MAX_READ_LINES {
                cut = offset + 1;
                break;
            }
        }
    }
    let traversed = &text[..cut];
    let mut line = cursor.line;
    let mut selected_start = None;
    let mut selected_end = None;
    let mut selected = String::new();
    for segment in traversed.split_inclusive('\n') {
        if line >= requested_start && requested_end.is_none_or(|end| line <= end) {
            selected_start.get_or_insert(line);
            selected_end = Some(line);
            selected.push_str(segment);
        }
        if segment.ends_with('\n') {
            line += 1;
        }
    }

    let consumed = traversed.len() as u64;
    let next_offset = cursor.byte_offset + consumed;
    if next_offset < metadata.len() && next_offset == cursor.byte_offset {
        return Err(FilesystemToolError::BinaryFile(format!(
            "file '{path}' cannot make progress while decoding UTF-8"
        )));
    }
    let range_finished = requested_end.is_some_and(|end| line > end);
    let next_cursor = (next_offset < metadata.len() && !range_finished).then_some(ReadFileCursor {
        byte_offset: next_offset,
        line,
    });

    Ok(ReadFileResult {
        content: selected,
        start_line: selected_start,
        end_line: selected_end,
        next_cursor,
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
    use golem_rust::SchemaValue;
    use golem_rust::agentic::ToolBuildCtx;
    use golem_rust::tool::{Tool, ToolInvokeError};
    use golem_rust::{IntoTypedSchemaValue, schema::FromSchema};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn path_policy_contract_classifies_supported_operations() {
        for (tool, operation) in [
            ("read-file", PathPolicyOperation::Read),
            ("ls", PathPolicyOperation::Read),
            ("write-file", PathPolicyOperation::Write),
            ("edit-file", PathPolicyOperation::Write),
            ("delete-file", PathPolicyOperation::Delete),
        ] {
            assert_eq!(
                protected_path_argument(tool, &[]),
                Ok(Some(ProtectedPathArgument {
                    operation,
                    argument: "path"
                }))
            );
        }
        assert_eq!(protected_path_argument("search", &[]), Ok(None));
        assert_eq!(
            protected_path_argument("read-file", &["nested".to_string()]),
            Err(vec!["nested".to_string()])
        );
    }

    #[test]
    fn path_policy_contract_rejects_ambiguous_configuration() {
        let valid = PathPolicyParameters {
            base: "/workspace".to_string(),
            allowed_roots: vec![PathPolicyRoot {
                path: "project".to_string(),
                operations: vec![PathPolicyOperation::Read],
            }],
        };
        assert_eq!(validate_path_policy_parameters(&valid), Ok(()));

        let mut invalid = valid.clone();
        invalid.base.clear();
        assert!(validate_path_policy_parameters(&invalid).is_err());
        invalid = valid.clone();
        invalid.base = "workspace".to_string();
        assert!(validate_path_policy_parameters(&invalid).is_err());
        invalid = valid.clone();
        invalid.base.push('\0');
        assert!(validate_path_policy_parameters(&invalid).is_err());
        invalid = valid.clone();
        invalid.base.push('\\');
        assert!(validate_path_policy_parameters(&invalid).is_err());
        invalid = valid.clone();
        invalid.allowed_roots.clear();
        assert!(validate_path_policy_parameters(&invalid).is_err());
        invalid = valid.clone();
        invalid.allowed_roots[0].path.clear();
        assert!(validate_path_policy_parameters(&invalid).is_err());
        invalid = valid.clone();
        invalid.allowed_roots[0].path.push('\0');
        assert!(validate_path_policy_parameters(&invalid).is_err());
        invalid = valid.clone();
        invalid.allowed_roots[0].path.push('\\');
        assert!(validate_path_policy_parameters(&invalid).is_err());
        invalid = valid;
        invalid.allowed_roots[0].operations.clear();
        assert!(validate_path_policy_parameters(&invalid).is_err());
    }

    #[test]
    fn path_policy_contract_accepts_absolute_and_relative_roots_and_round_trips() {
        let parameters = PathPolicyParameters {
            base: "/workspace".to_string(),
            allowed_roots: vec![
                PathPolicyRoot {
                    path: "readable".to_string(),
                    operations: vec![PathPolicyOperation::Read],
                },
                PathPolicyRoot {
                    path: "/workspace/writable".to_string(),
                    operations: vec![PathPolicyOperation::Write, PathPolicyOperation::Delete],
                },
            ],
        };
        assert_eq!(validate_path_policy_parameters(&parameters), Ok(()));
        let typed = parameters.clone().into_typed_schema_value().unwrap();
        assert_eq!(
            PathPolicyParameters::from_value(typed.value()).unwrap(),
            parameters
        );
    }

    fn policy(
        base: &Path,
        root: &str,
        operations: Vec<PathPolicyOperation>,
    ) -> PathPolicyParameters {
        PathPolicyParameters {
            base: base.to_string_lossy().into_owned(),
            allowed_roots: vec![PathPolicyRoot {
                path: root.to_string(),
                operations,
            }],
        }
    }

    fn with_temp_directory<T>(test: impl FnOnce(&Path) -> T) -> T {
        let path = std::env::temp_dir().join(format!(
            "golem-path-policy-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        let result = test(&path.canonicalize().unwrap());
        fs::remove_dir_all(path).unwrap();
        result
    }

    #[test]
    fn path_policy_uses_component_containment_after_normalization() {
        with_temp_directory(|base| {
            fs::create_dir(base.join("workspace")).unwrap();
            fs::create_dir(base.join("workspace-other")).unwrap();
            let parameters = policy(
                base,
                "workspace",
                vec![PathPolicyOperation::Read, PathPolicyOperation::Write],
            );

            assert_eq!(
                authorize_path(&parameters, PathPolicyOperation::Read, "workspace/file").unwrap(),
                base.join("workspace/file")
            );
            assert_eq!(
                authorize_path(
                    &parameters,
                    PathPolicyOperation::Write,
                    base.join("workspace/new/child").to_str().unwrap()
                )
                .unwrap(),
                base.join("workspace/new/child")
            );
            assert!(
                authorize_path(
                    &parameters,
                    PathPolicyOperation::Read,
                    "workspace-other/file"
                )
                .is_err()
            );
            assert!(
                authorize_path(
                    &parameters,
                    PathPolicyOperation::Read,
                    "workspace/../workspace-other/file"
                )
                .is_err()
            );
            assert!(
                authorize_path(
                    &parameters,
                    PathPolicyOperation::Read,
                    r"workspace\..\workspace-other/file"
                )
                .is_err()
            );
        });
    }

    #[test]
    fn path_policy_applies_independent_operation_grants() {
        with_temp_directory(|base| {
            fs::create_dir(base.join("root-a")).unwrap();
            fs::create_dir(base.join("root-b")).unwrap();
            for granted in [
                PathPolicyOperation::Read,
                PathPolicyOperation::Write,
                PathPolicyOperation::Delete,
            ] {
                let parameters = policy(base, "root-a", vec![granted]);
                for requested in [
                    PathPolicyOperation::Read,
                    PathPolicyOperation::Write,
                    PathPolicyOperation::Delete,
                ] {
                    assert_eq!(
                        authorize_path(&parameters, requested, "root-a/file").is_ok(),
                        requested == granted,
                        "granted {granted:?}, requested {requested:?}"
                    );
                }
            }

            let parameters = PathPolicyParameters {
                base: base.to_string_lossy().into_owned(),
                allowed_roots: vec![
                    PathPolicyRoot {
                        path: "root-a".to_string(),
                        operations: vec![PathPolicyOperation::Read],
                    },
                    PathPolicyRoot {
                        path: "root-b".to_string(),
                        operations: vec![PathPolicyOperation::Write],
                    },
                ],
            };
            assert!(
                authorize_path(&parameters, PathPolicyOperation::Write, "root-a/file").is_err()
            );
            assert!(authorize_path(&parameters, PathPolicyOperation::Read, "root-b/file").is_err());
        });
    }

    #[cfg(unix)]
    #[test]
    fn path_policy_rejects_existing_symlink_components_and_targets() {
        use std::os::unix::fs::symlink;

        with_temp_directory(|base| {
            fs::create_dir(base.join("allowed")).unwrap();
            fs::create_dir(base.join("outside")).unwrap();
            fs::write(base.join("outside/secret"), "secret").unwrap();
            symlink(base.join("outside"), base.join("allowed/escape")).unwrap();
            symlink(base.join("outside/secret"), base.join("allowed/final-link")).unwrap();
            symlink(base.join("outside/missing"), base.join("allowed/dangling")).unwrap();
            let parameters = policy(
                base,
                "allowed",
                vec![PathPolicyOperation::Read, PathPolicyOperation::Write],
            );

            for path in ["allowed/escape/secret", "allowed/final-link"] {
                let violation =
                    authorize_path(&parameters, PathPolicyOperation::Read, path).unwrap_err();
                assert!(violation.reason.contains("symbolic link"), "{violation:?}");
            }
            for path in ["allowed/escape/missing/target", "allowed/dangling"] {
                let violation =
                    authorize_path(&parameters, PathPolicyOperation::Write, path).unwrap_err();
                assert!(violation.reason.contains("symbolic link"), "{violation:?}");
            }
        });
    }

    #[test]
    fn path_policy_allows_nonexistent_write_suffix_below_real_ancestor() {
        with_temp_directory(|base| {
            fs::create_dir(base.join("allowed")).unwrap();
            let parameters = policy(base, "allowed", vec![PathPolicyOperation::Write]);
            assert_eq!(
                authorize_path(
                    &parameters,
                    PathPolicyOperation::Write,
                    "allowed/missing/target"
                )
                .unwrap(),
                base.join("allowed/missing/target")
            );
        });
    }

    #[derive(IntoSchema)]
    struct ReadFileInput {
        path: String,
        start_line: Option<u64>,
        end_line: Option<u64>,
        cursor: Option<ReadFileCursor>,
    }

    #[derive(IntoSchema)]
    struct WriteFileInput {
        path: String,
        content: String,
        create_parent_directories: bool,
    }

    fn read_file_tool() -> Tool {
        __golem_tool_descriptor_for_ReadFile(&mut ToolBuildCtx::new())
            .unwrap()
            .try_to_native_tool()
            .unwrap()
    }

    fn write_file_tool() -> Tool {
        __golem_tool_descriptor_for_WriteFile(&mut ToolBuildCtx::new())
            .unwrap()
            .try_to_native_tool()
            .unwrap()
    }

    #[test]
    fn path_policy_rewrites_only_the_authorized_path_field() {
        with_temp_directory(|base| {
            fs::create_dir(base.join("allowed")).unwrap();
            let parameters = policy(base, "allowed", vec![PathPolicyOperation::Read]);
            let input = ReadFileInput {
                path: "allowed/file".to_string(),
                start_line: Some(7),
                end_line: Some(11),
                cursor: Some(ReadFileCursor {
                    byte_offset: 13,
                    line: 3,
                }),
            }
            .into_typed_schema_value()
            .unwrap();
            let rewritten =
                apply_path_policy(&parameters, "read-file", &read_file_tool(), &[], input).unwrap();
            assert_eq!(
                rewritten,
                ReadFileInput {
                    path: base.join("allowed/file").to_string_lossy().into_owned(),
                    start_line: Some(7),
                    end_line: Some(11),
                    cursor: Some(ReadFileCursor {
                        byte_offset: 13,
                        line: 3,
                    }),
                }
                .into_typed_schema_value()
                .unwrap()
            );

            let write_input = WriteFileInput {
                path: "allowed/output".to_string(),
                content: "leave path-looking text allowed/unchanged".to_string(),
                create_parent_directories: true,
            }
            .into_typed_schema_value()
            .unwrap();
            assert_eq!(
                apply_path_policy(
                    &policy(base, "allowed", vec![PathPolicyOperation::Write]),
                    "write-file",
                    &write_file_tool(),
                    &[],
                    write_input,
                )
                .unwrap(),
                WriteFileInput {
                    path: base.join("allowed/output").to_string_lossy().into_owned(),
                    content: "leave path-looking text allowed/unchanged".to_string(),
                    create_parent_directories: true,
                }
                .into_typed_schema_value()
                .unwrap()
            );
        });
    }

    #[test]
    fn path_policy_applies_every_protected_operation_and_passes_unknown_tools_through() {
        with_temp_directory(|base| {
            fs::create_dir(base.join("allowed")).unwrap();
            let parameters = policy(
                base,
                "allowed",
                vec![
                    PathPolicyOperation::Read,
                    PathPolicyOperation::Write,
                    PathPolicyOperation::Delete,
                ],
            );
            let tool = read_file_tool();
            for tool_name in ["read-file", "ls", "write-file", "edit-file", "delete-file"] {
                let input = ReadFileInput {
                    path: "allowed/file".to_string(),
                    start_line: None,
                    end_line: None,
                    cursor: None,
                }
                .into_typed_schema_value()
                .unwrap();
                let rewritten =
                    apply_path_policy(&parameters, tool_name, &tool, &[], input).unwrap();
                let fields = tool
                    .decode_canonical_input_record(0, rewritten.into_parts().1)
                    .unwrap();
                assert_eq!(
                    fields
                        .iter()
                        .find(|field| field.name == "path")
                        .unwrap()
                        .value,
                    SchemaValue::String(base.join("allowed/file").to_string_lossy().into_owned()),
                    "{tool_name}"
                );
            }

            let input = ReadFileInput {
                path: "outside/file".to_string(),
                start_line: Some(3),
                end_line: None,
                cursor: None,
            }
            .into_typed_schema_value()
            .unwrap();
            assert_eq!(
                apply_path_policy(&parameters, "search", &tool, &[], input.clone()).unwrap(),
                input
            );
        });
    }

    #[test]
    fn path_policy_fails_closed_for_recognized_non_root_commands() {
        with_temp_directory(|base| {
            fs::create_dir(base.join("allowed")).unwrap();
            let parameters = policy(base, "allowed", vec![PathPolicyOperation::Read]);
            let input = ReadFileInput {
                path: "allowed/file".to_string(),
                start_line: None,
                end_line: None,
                cursor: None,
            }
            .into_typed_schema_value()
            .unwrap();

            assert!(matches!(
                apply_path_policy(
                    &parameters,
                    "read-file",
                    &read_file_tool(),
                    &["nested".to_string()],
                    input
                ),
                Err(ToolInvokeError::InvalidCommandPath(path)) if path == ["nested"]
            ));
        });
    }

    #[test]
    fn path_policy_returns_structured_denial_before_dispatch() {
        with_temp_directory(|base| {
            fs::create_dir(base.join("allowed")).unwrap();
            fs::create_dir(base.join("write-only")).unwrap();
            let parameters = PathPolicyParameters {
                base: base.to_string_lossy().into_owned(),
                allowed_roots: vec![
                    PathPolicyRoot {
                        path: "allowed".to_string(),
                        operations: vec![PathPolicyOperation::Read],
                    },
                    PathPolicyRoot {
                        path: "write-only".to_string(),
                        operations: vec![PathPolicyOperation::Write],
                    },
                ],
            };
            let input = ReadFileInput {
                path: "outside/file".to_string(),
                start_line: None,
                end_line: None,
                cursor: None,
            }
            .into_typed_schema_value()
            .unwrap();
            let error = apply_path_policy(&parameters, "read-file", &read_file_tool(), &[], input)
                .unwrap_err();
            let ToolInvokeError::UnknownCustomError(error) = error else {
                panic!("expected structured custom denial")
            };
            assert_eq!(error.name, "path-policy-denied");
            let denial = PathPolicyDenied::from_value(error.payload().unwrap().value()).unwrap();
            assert_eq!(denial.operation, PathPolicyOperation::Read);
            assert_eq!(denial.argument, "path");
            assert_eq!(denial.supplied_path, "outside/file");
            assert_eq!(denial.allowed_roots, ["allowed"]);
            assert!(denial.resolved_path.unwrap().ends_with("/outside/file"));
        });
    }

    fn with_file<T>(content: &[u8], test: impl FnOnce(&str) -> T) -> T {
        let path = std::env::temp_dir().join(format!(
            "golem-filesystem-tool-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::write(&path, content).unwrap();
        let result = test(path.to_str().unwrap());
        fs::remove_file(path).unwrap();
        result
    }

    #[test]
    fn line_ranges_are_inclusive_and_preserve_crlf() {
        with_file(b"one\r\ntwo\r\nthree", |path| {
            let result = read_file_page(path, Some(2), Some(9), None).unwrap();
            assert_eq!(result.content, "two\r\nthree");
            assert_eq!(result.start_line, Some(2));
            assert_eq!(result.end_line, Some(3));
            assert_eq!(result.next_cursor, None);
        });
    }

    #[test]
    fn empty_file_and_start_after_eof_return_an_empty_final_page() {
        with_file(b"", |path| {
            let result = read_file_page(path, None, None, None).unwrap();
            assert_eq!(result.content, "");
            assert_eq!(result.start_line, None);
            assert_eq!(result.next_cursor, None);
        });
        with_file(b"a\n", |path| {
            let result = read_file_page(path, Some(20), None, None).unwrap();
            assert_eq!(result.content, "");
            assert_eq!(result.next_cursor, None);
        });
    }

    #[test]
    fn invalid_and_descending_ranges_are_rejected() {
        assert!(read_file_page("file", Some(0), None, None).is_err());
        assert!(read_file_page("file", Some(2), Some(1), None).is_err());
    }

    #[test]
    fn pages_at_two_hundred_lines_and_continues() {
        let content: String = (1..=205).map(|line| format!("line {line}\n")).collect();
        with_file(content.as_bytes(), |path| {
            let first = read_file_page(path, None, None, None).unwrap();
            assert_eq!(first.start_line, Some(1));
            assert_eq!(first.end_line, Some(200));
            let cursor = first.next_cursor.unwrap();
            assert_eq!(cursor.line, 201);
            let second = read_file_page(path, None, None, Some(cursor)).unwrap();
            assert_eq!(second.start_line, Some(201));
            assert_eq!(second.end_line, Some(205));
            assert_eq!(format!("{}{}", first.content, second.content), content);
            assert_eq!(second.next_cursor, None);
        });
    }

    #[test]
    fn distant_start_advances_with_empty_pages() {
        let content: String = (1..=450).map(|line| format!("{line}\n")).collect();
        with_file(content.as_bytes(), |path| {
            let first = read_file_page(path, Some(401), Some(402), None).unwrap();
            assert!(first.content.is_empty());
            let second = read_file_page(path, Some(401), Some(402), first.next_cursor).unwrap();
            assert!(second.content.is_empty());
            let third = read_file_page(path, Some(401), Some(402), second.next_cursor).unwrap();
            assert_eq!(third.content, "401\n402\n");
            assert_eq!(third.next_cursor, None);
        });
    }

    #[test]
    fn oversized_lines_page_without_splitting_utf8() {
        let content = format!("{}é-tail\nnext", "a".repeat(MAX_READ_BYTES - 1));
        with_file(content.as_bytes(), |path| {
            let first = read_file_page(path, None, None, None).unwrap();
            assert_eq!(first.content.len(), MAX_READ_BYTES - 1);
            assert_eq!(first.end_line, Some(1));
            let second = read_file_page(path, None, None, first.next_cursor).unwrap();
            assert_eq!(format!("{}{}", first.content, second.content), content);
            assert_eq!(second.start_line, Some(1));
            assert_eq!(second.end_line, Some(2));
        });
    }

    #[test]
    fn paging_does_not_split_crlf() {
        let content = format!("{}\r\nnext", "a".repeat(MAX_READ_BYTES - 1));
        with_file(content.as_bytes(), |path| {
            let first = read_file_page(path, None, None, None).unwrap();
            assert!(!first.content.ends_with('\r'));
            let cursor = first.next_cursor.unwrap();
            assert_eq!(cursor.byte_offset, (MAX_READ_BYTES - 1) as u64);
            let second = read_file_page(path, None, None, Some(cursor)).unwrap();
            assert!(second.content.starts_with("\r\n"));
            assert_eq!(format!("{}{}", first.content, second.content), content);
        });
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
        with_file(&[0xc3], |path| {
            assert!(matches!(
                read_file_page(path, None, None, None),
                Err(FilesystemToolError::BinaryFile(_))
            ));
        });
        with_file(b"valid prefix \xc3", |path| {
            assert!(matches!(
                read_file_page(path, None, None, None),
                Err(FilesystemToolError::BinaryFile(_))
            ));
        });
    }

    #[test]
    fn forged_cursors_are_rejected() {
        with_file("é".as_bytes(), |path| {
            assert!(matches!(
                read_file_page(
                    path,
                    None,
                    None,
                    Some(ReadFileCursor {
                        byte_offset: 1,
                        line: 1,
                    }),
                ),
                Err(FilesystemToolError::InvalidRange(_))
            ));
            assert!(matches!(
                read_file_page(
                    path,
                    None,
                    None,
                    Some(ReadFileCursor {
                        byte_offset: 0,
                        line: 2,
                    }),
                ),
                Err(FilesystemToolError::InvalidRange(_))
            ));
        });
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
