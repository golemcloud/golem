use crate::{FilesystemToolError, io_error, validate_path};
use golem_rust::{
    FromSchema, FromWire, IntoSchema, IntoWire, WireSchema, tool_definition, tool_implementation,
};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};

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

#[tool_definition(version = "0.1.0", requires_filesystem = true)]
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

struct ReadFileImpl;

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
        Err(error) if error.error_len().is_none() && !at_eof => bytes.truncate(error.valid_up_to()),
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
