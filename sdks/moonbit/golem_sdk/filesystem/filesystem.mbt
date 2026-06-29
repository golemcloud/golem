///|
pub using @types {type Descriptor}

///|
pub using @types {type DirectoryEntryStream}

///|
pub using @types {type DescriptorFlags}

///|
pub using @types {type PathFlags}

///|
pub using @types {type OpenFlags}

///|
pub using @types {type DirectoryEntry}

///|
pub using @types {type DescriptorStat}

///|
pub using @types {type NewTimestamp}

///|
pub using @types {type DescriptorType}

///|
pub using @types {type Advice}

///|
pub using @types {type MetadataHashValue}

///|
pub using @types {type DescriptorFlagsFlag}

///|
pub using @types {type PathFlagsFlag}

///|
pub using @types {type OpenFlagsFlag}

///|
/// Returns the preopened directories and their paths.
pub fn get_directories() -> Array[(Descriptor, String)] {
  @preopens.get_directories()
}

///|
/// Returns the preopened directory descriptor for the given path, or `None`
/// if no preopened directory matches.
pub fn get_root_dir() -> Descriptor? {
  get_preopened_dir("/")
}

///|
/// Returns the preopened directory descriptor for the given path, or `None`
/// if no preopened directory matches.
pub fn get_preopened_dir(path : String) -> Descriptor? {
  let dirs = @preopens.get_directories()
  for pair in dirs {
    let (descriptor, dir_path) = pair
    if dir_path == path {
      return Some(descriptor)
    }
  }
  None
}

///|
/// Normalizes a path for use with WASI `open-at`, which requires paths
/// relative to the preopened descriptor. Strips any leading `/`
/// characters so callers can naturally use either `"data/file"` or
/// `"/data/file"` (matching the IFS `targetPath` in `golem.yaml`).
fn normalize_path(path : String) -> String {
  let mut start = 0
  let len = path.length()
  while start < len && path[start] == '/' {
    start += 1
  }
  if start == 0 {
    path
  } else {
    path.substring(start~, end=len)
  }
}

///|
/// Maximum number of bytes the host (wasmtime) will allocate per WASI
/// `read` call (`MAX_READ_SIZE_ALLOC` in wasmtime's
/// `crates/wasi/src/lib.rs`). Any single `Descriptor::read` request
/// larger than this is silently truncated by the host, so we must issue
/// chunked reads to read larger files.
const MAX_READ_CHUNK_SIZE : UInt64 = 65536

///|
/// Reads the full contents of a file as a `FixedArray[Byte]`.
///
/// The underlying WASI host implementation caps each `read` call at
/// `MAX_READ_CHUNK_SIZE` bytes, so this helper performs a sequence of
/// chunked reads at increasing offsets and concatenates the results.
pub fn read_bytes(
  root : Descriptor,
  path : String,
) -> Result[FixedArray[Byte], @types.ErrorCode] {
  let file = root.open_at(
    PathFlags::default(),
    normalize_path(path),
    OpenFlags::default(),
    DescriptorFlags::default().set(READ),
  )
  match file {
    Err(e) => Err(e)
    Ok(fd) => {
      let result = read_all_chunks(fd)
      // The opened descriptor is a host-side resource and must be
      // explicitly dropped to avoid leaking handles, regardless of
      // whether the read succeeded or failed.
      fd.drop()
      result
    }
  }
}

///|
/// Helper that performs the chunked-read loop on an already-open
/// descriptor and concatenates the chunks into a single
/// `FixedArray[Byte]`.
fn read_all_chunks(
  fd : Descriptor,
) -> Result[FixedArray[Byte], @types.ErrorCode] {
  let chunks : Array[FixedArray[Byte]] = []
  let mut offset : UInt64 = 0
  let mut done = false
  while not(done) {
    match fd.read(MAX_READ_CHUNK_SIZE, offset) {
      Err(e) => return Err(e)
      Ok((chunk, eof)) => {
        let chunk_len = chunk.length()
        if chunk_len > 0 {
          chunks.push(chunk)
          offset = offset + chunk_len.to_uint64()
        }
        // Stop on EOF, OR on a zero-length non-EOF response. Treating a
        // zero-byte read as EOF is intentional: it prevents an infinite
        // loop in case a host ever returns no progress without setting
        // the EOF flag. The trade-off (potentially truncating data on a
        // misbehaving host) is preferable to spinning forever in a
        // "read whole file" helper. Do NOT remove this guard.
        if eof || chunk_len == 0 {
          done = true
        }
      }
    }
  }
  let mut total = 0
  for c in chunks {
    total = total + c.length()
  }
  let result = FixedArray::make(total, b'\x00')
  let mut pos = 0
  for c in chunks {
    let cl = c.length()
    let mut i = 0
    while i < cl {
      result[pos + i] = c[i]
      i += 1
    }
    pos = pos + cl
  }
  Ok(result)
}

///|
/// Reads the full contents of a file as a UTF-8 string.
///
/// Invalid UTF-8 sequences are replaced with the Unicode replacement
/// character (U+FFFD) — see `@utf8.decode_lossy`. If you need to detect
/// decode errors, use `read_bytes` and decode the result yourself.
pub fn read_string(
  root : Descriptor,
  path : String,
) -> Result[String, @types.ErrorCode] {
  match read_bytes(root, path) {
    Err(e) => Err(e)
    Ok(bytes) => {
      let bs = Bytes::from_fixedarray(bytes)
      Ok(@utf8.decode_lossy(bs[:]))
    }
  }
}

///|
/// Writes bytes to a file, creating or truncating it.
///
/// `Descriptor::write` may legally write fewer bytes than requested
/// (a partial write); this wrapper loops until all bytes are written
/// or the host stops making progress.
pub fn write_bytes(
  root : Descriptor,
  path : String,
  data : FixedArray[Byte],
) -> Result[Unit, @types.ErrorCode] {
  let file = root.open_at(
    PathFlags::default(),
    normalize_path(path),
    OpenFlags::default().set(CREATE).set(TRUNCATE),
    DescriptorFlags::default().set(WRITE),
  )
  match file {
    Err(e) => Err(e)
    Ok(fd) => {
      let result = write_all_bytes(fd, data)
      fd.drop()
      result
    }
  }
}

///|
/// Helper that writes all of `data` to the given descriptor starting
/// at offset 0, looping over partial writes until the entire buffer is
/// written.
fn write_all_bytes(
  fd : Descriptor,
  data : FixedArray[Byte],
) -> Result[Unit, @types.ErrorCode] {
  let total = data.length()
  let mut written : Int = 0
  while written < total {
    let remaining = total - written
    // Slice off the next chunk to write. We bound each write at
    // MAX_READ_CHUNK_SIZE for parity with the read path; the wasmtime
    // write impl does not currently use the same allocation cap, but
    // chunking keeps memory usage and behavior predictable across
    // hosts.
    let chunk_len = if remaining.to_uint64() < MAX_READ_CHUNK_SIZE {
      remaining
    } else {
      MAX_READ_CHUNK_SIZE.to_int()
    }
    let chunk : FixedArray[Byte] = FixedArray::make(chunk_len, b'\x00')
    let mut i = 0
    while i < chunk_len {
      chunk[i] = data[written + i]
      i += 1
    }
    match fd.write(chunk, written.to_uint64()) {
      Err(e) => return Err(e)
      Ok(n) => {
        let n_int = n.to_int()
        if n_int <= 0 {
          // No progress — bail out rather than spinning forever. We
          // surface this as IoError because the host failed to write
          // any of the remaining bytes.
          return Err(@types.ErrorCode::IO)
        }
        written = written + n_int
      }
    }
  }
  Ok(())
}

///|
/// Writes a string to a file as UTF-8, creating or truncating it.
pub fn write_string(
  root : Descriptor,
  path : String,
  content : String,
) -> Result[Unit, @types.ErrorCode] {
  let encoded = @utf8.encode(content[:])
  let bytes = Bytes::to_fixedarray(encoded)
  write_bytes(root, path, bytes)
}

///|
/// Lists the names of entries in a directory.
pub fn list_directory(
  root : Descriptor,
  path : String,
) -> Result[Array[String], @types.ErrorCode] {
  let dir = root.open_at(
    PathFlags::default(),
    normalize_path(path),
    OpenFlags::default().set(DIRECTORY),
    DescriptorFlags::default().set(READ),
  )
  match dir {
    Err(e) => Err(e)
    Ok(fd) => {
      let result = read_all_dir_entries(fd)
      fd.drop()
      result
    }
  }
}

///|
/// Helper that reads all directory entries from an open directory
/// descriptor and returns their names. The created entry stream is
/// explicitly dropped to release the host-side resource.
fn read_all_dir_entries(
  fd : Descriptor,
) -> Result[Array[String], @types.ErrorCode] {
  match fd.read_directory() {
    Err(e) => Err(e)
    Ok(s) => {
      let entries : Array[String] = []
      let mut err : @types.ErrorCode? = None
      let mut done = false
      while not(done) {
        match s.read_directory_entry() {
          Ok(Some(entry)) => entries.push(entry.name)
          Ok(None) => done = true
          Err(e) => {
            err = Some(e)
            done = true
          }
        }
      }
      s.drop()
      match err {
        Some(e) => Err(e)
        None => Ok(entries)
      }
    }
  }
}
