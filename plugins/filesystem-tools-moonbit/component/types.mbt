///|
#derive.golem_schema
pub(all) struct ReadFileResult {
  content : String
  start_line : UInt64?
  end_line : UInt64?
  total_lines : UInt64
  truncated_before : Bool
  truncated_after : Bool
}

///|
#derive.golem_schema
pub(all) enum WriteDisposition {
  Created
  Replaced
}

///|
#derive.golem_schema
pub(all) struct WriteFileResult {
  disposition : WriteDisposition
  bytes_written : UInt64
}

///|
#derive.golem_schema
pub(all) struct EditFileResult {
  replacements : UInt64
  bytes_before : UInt64
  bytes_after : UInt64
}

///|
enum FilesystemToolError {
  /// The path is empty, contains NUL, or contains a parent traversal component.
  #derive.error(kind="usage-error", exit_code="2")
  UnsafePath(String)
  /// The requested line range is invalid.
  #derive.error(kind="usage-error", exit_code="2")
  InvalidRange(String)
  /// The path does not exist.
  #derive.error(kind="runtime-error", exit_code="1")
  NotFound(String)
  /// The path exists but is not a regular file.
  #derive.error(kind="runtime-error", exit_code="1")
  NotAFile(String)
  /// File contents are not text suitable for these tools.
  #derive.error(kind="runtime-error", exit_code="1")
  BinaryFile(String)
  /// The edit search text is empty.
  #derive.error(kind="usage-error", exit_code="2")
  EmptyOldText
  /// The expected text was not found; the caller likely has stale content.
  #derive.error(kind="runtime-error", exit_code="1")
  StaleEdit(String)
  /// The expected text occurs more than once and cannot be edited unambiguously.
  #derive.error(kind="usage-error", exit_code="2")
  AmbiguousEdit(String)
  /// A filesystem operation failed.
  #derive.error(kind="runtime-error", exit_code="1")
  Io(String)
}
