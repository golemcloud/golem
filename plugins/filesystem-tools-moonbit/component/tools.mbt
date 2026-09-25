///|
/// Read a UTF-8 regular file, optionally selecting a 1-based inclusive line range.
#derive.tool("read-file-moonbit", version="0.1.0-moonbit")
#warnings("-struct_never_constructed")
struct ReadFile {}

///|
/// Read text while preserving its original line endings.
#derive.command(read_only=true, destructive=false, idempotent=true)
pub async fn ReadFile::read_file(
  path : String,
  range : Array[UInt64],
) -> Result[ReadFileResult, FilesystemToolError] {
  let normalized = match validate_path(path) {
    Ok(path) => path
    Err(error) => return Err(error)
  }
  let bytes = match read_regular_file(path, normalized) {
    Ok(bytes) => bytes
    Err(error) => return Err(error)
  }
  if !valid_utf8(bytes) {
    return Err(BinaryFile("file '\{path}' contains NUL or invalid UTF-8"))
  }
  if range.length() > 2 {
    return Err(
      InvalidRange("range must contain zero, one, or two line numbers"),
    )
  }
  let start_line = if range.length() > 0 { Some(range[0]) } else { None }
  let end_line = if range.length() > 1 { Some(range[1]) } else { None }
  match slice_lines(bytes, start_line, end_line) {
    Err(error) => Err(error)
    Ok((selected, total, effective_start, effective_end)) =>
      Ok({
        content: @utf8.decode_lossy(Bytes::from_array(selected)[:]),
        start_line: effective_start,
        end_line: effective_end,
        total_lines: total,
        truncated_before: effective_start is Some(start) && start > 1UL,
        truncated_after: effective_end is Some(end) && end < total,
      })
  }
}

///|
/// Create or replace a regular file with exact UTF-8 content.
#derive.tool("write-file-moonbit", version="0.1.0-moonbit")
#warnings("-struct_never_constructed")
struct WriteFile {}

///|
/// Write a file, optionally creating missing parent directories.
#derive.command(destructive=true, idempotent=true)
pub async fn WriteFile::write_file(
  path : String,
  content : String,
  create_parent_directories : Bool,
) -> Result[WriteFileResult, FilesystemToolError] {
  let normalized = match validate_path(path) {
    Ok(path) => path
    Err(error) => return Err(error)
  }
  let root = match @fs.get_root_dir() {
    Some(root) => root
    None => return Err(Io("cannot open '\{path}': root preopen is unavailable"))
  }
  let existed = match root.stat_at(@fs.PathFlags::default(), normalized) {
    Ok(stat) => {
      if stat.type_ != @fs.DescriptorType::RegularFile {
        return Err(NotAFile(path))
      }
      true
    }
    Err(@fsTypes.ErrorCode::NoEntry) => false
    Err(error) => return Err(io_error(path, "inspect", error))
  }
  if create_parent_directories {
    match create_parents(root, normalized) {
      Err(error) => return Err(error)
      Ok(_) => ()
    }
  }
  match @fs.write_string(root, normalized, content) {
    Err(error) => Err(io_error(path, "write", error))
    Ok(_) =>
      Ok({
        disposition: if existed {
          Replaced
        } else {
          Created
        },
        bytes_written: @utf8.encode(content[:]).length().to_uint64(),
      })
  }
}

///|
/// Replace one unambiguous occurrence of text in a UTF-8 regular file.
#derive.tool("edit-file-moonbit", version="0.1.0-moonbit")
#warnings("-struct_never_constructed")
struct EditFile {}

///|
/// Make exactly one replacement and reject stale or ambiguous edits.
#derive.command(destructive=true, idempotent=false)
pub async fn EditFile::edit_file(
  path : String,
  old_text : String,
  new_text : String,
) -> Result[EditFileResult, FilesystemToolError] {
  let normalized = match validate_path(path) {
    Ok(path) => path
    Err(error) => return Err(error)
  }
  if old_text == "" {
    return Err(EmptyOldText)
  }
  let bytes = match read_regular_file(path, normalized) {
    Ok(bytes) => bytes
    Err(error) => return Err(error)
  }
  if !valid_utf8(bytes) {
    return Err(BinaryFile("file '\{path}' contains NUL or invalid UTF-8"))
  }
  let content = @utf8.decode_lossy(Bytes::from_array(bytes)[:])
  let matches = find_matches(content, old_text)
  if matches.length() == 0 {
    return Err(StaleEdit("text to replace was not found in '\{path}'"))
  }
  if matches.length() != 1 {
    return Err(
      AmbiguousEdit(
        "text to replace occurs \{matches.length()} times in '\{path}'",
      ),
    )
  }
  let at = matches[0]
  let updated = content[:at].to_owned() +
    new_text +
    content[at + old_text.length():].to_owned()
  let root = match @fs.get_root_dir() {
    Some(root) => root
    None => return Err(Io("cannot open '\{path}': root preopen is unavailable"))
  }
  match @fs.write_string(root, normalized, updated) {
    Err(error) => Err(io_error(path, "write edit", error))
    Ok(_) =>
      Ok({
        replacements: 1UL,
        bytes_before: bytes.length().to_uint64(),
        bytes_after: @utf8.encode(updated[:]).length().to_uint64(),
      })
  }
}

///|
async fn read_regular_file(
  original : String,
  normalized : String,
) -> Result[FixedArray[Byte], FilesystemToolError] {
  let root = match @fs.get_root_dir() {
    Some(root) => root
    None =>
      return Err(Io("cannot open '\{original}': root preopen is unavailable"))
  }
  match root.stat_at(@fs.PathFlags::default(), normalized) {
    Err(@fsTypes.ErrorCode::NoEntry) => return Err(NotFound(original))
    Err(error) => return Err(io_error(original, "inspect", error))
    Ok(stat) =>
      if stat.type_ != @fs.DescriptorType::RegularFile {
        return Err(NotAFile(original))
      }
  }
  match @fs.read_bytes(root, normalized) {
    Ok(bytes) => Ok(bytes)
    Err(error) => Err(io_error(original, "read", error))
  }
}

///|
async fn create_parents(
  root : @fs.Descriptor,
  path : String,
) -> Result[Unit, FilesystemToolError] {
  let parts = path.split("/").to_array()
  let current = StringBuilder()
  let mut has_component = false
  for i in 0..<(parts.length() - 1) {
    if parts[i] == "" || parts[i] == "." {
      continue
    }
    if has_component {
      current.write_string("/")
    }
    current.write_string(parts[i].to_owned())
    has_component = true
    let directory = current.to_string()
    match root.create_directory_at(directory) {
      Ok(_) | Err(@fsTypes.ErrorCode::Exist) => ()
      Err(error) => return Err(io_error(path, "create parent directory", error))
    }
  }
  Ok(())
}

///|
fn io_error(
  path : String,
  operation : String,
  error : @fsTypes.ErrorCode,
) -> FilesystemToolError {
  ignore(error)
  Io("\{operation} '\{path}' failed in WASI filesystem")
}

///|
fn main {

}
