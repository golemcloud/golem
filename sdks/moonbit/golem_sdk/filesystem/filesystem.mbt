///|
pub using @types {type Descriptor}

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
pub async fn read_bytes(
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
async fn read_all_chunks(
  fd : Descriptor,
) -> Result[FixedArray[Byte], @types.ErrorCode] {
  let chunks : Array[FixedArray[Byte]] = []
  let (contents, completed) = fd.read_via_stream(0)
  for ;; {
    match contents.read(MAX_READ_CHUNK_SIZE.to_int()) {
      Some(chunk) => {
        let chunk_len = chunk.length()
        if chunk_len > 0 {
          chunks.push(chunk)
        }
      }
      None => break
    }
  }
  contents.drop()
  let result = completed.get()
  completed.drop()
  match result {
    Err(e) => return Err(e)
    Ok(_) => ()
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
pub async fn read_string(
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
pub async fn write_bytes(
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
async fn write_all_bytes(
  fd : Descriptor,
  data : FixedArray[Byte],
) -> Result[Unit, @types.ErrorCode] {
  let contents = @asyncCore.Stream::produce(async fn(sink) {
    if sink.write_all(data[:]) {
      sink.close()
    }
  })
  let completed = fd.write_via_stream(contents, 0)
  let result = completed.get()
  completed.drop()
  result
}

///|
/// Writes a string to a file as UTF-8, creating or truncating it.
pub async fn write_string(
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
pub async fn list_directory(
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
async fn read_all_dir_entries(
  fd : Descriptor,
) -> Result[Array[String], @types.ErrorCode] {
  let (contents, completed) = fd.read_directory()
  let entries : Array[String] = []
  for ;; {
    match contents.read(64) {
      Some(chunk) =>
        for entry in chunk {
          entries.push(entry.name)
        }
      None => break
    }
  }
  contents.drop()
  let result = completed.get()
  completed.drop()
  match result {
    Err(e) => Err(e)
    Ok(_) => Ok(entries)
  }
}
