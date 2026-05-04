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
/// Reads the full contents of a file as a `FixedArray[Byte]`.
pub fn read_bytes(
  root : Descriptor,
  path : String,
) -> Result[FixedArray[Byte], @types.ErrorCode] {
  let file = root.open_at(
    PathFlags::default(),
    path,
    OpenFlags::default(),
    DescriptorFlags::default().set(READ),
  )
  match file {
    Err(e) => Err(e)
    Ok(fd) => {
      let stat = fd.stat()
      match stat {
        Err(e) => Err(e)
        Ok(s) => {
          let result = fd.read(s.size, 0)
          match result {
            Err(e) => Err(e)
            Ok((bytes, _eof)) => Ok(bytes)
          }
        }
      }
    }
  }
}

///|
/// Reads the full contents of a file as a `String`.
pub fn read_string(
  root : Descriptor,
  path : String,
) -> Result[String, @types.ErrorCode] {
  match read_bytes(root, path) {
    Err(e) => Err(e)
    Ok(bytes) =>
      Ok(String::from_array(bytes.map(fn(b) { Char::from_int(b.to_int()) })))
  }
}

///|
/// Writes bytes to a file, creating or truncating it.
pub fn write_bytes(
  root : Descriptor,
  path : String,
  data : FixedArray[Byte],
) -> Result[Unit, @types.ErrorCode] {
  let file = root.open_at(
    PathFlags::default(),
    path,
    OpenFlags::default().set(CREATE).set(TRUNCATE),
    DescriptorFlags::default().set(WRITE),
  )
  match file {
    Err(e) => Err(e)
    Ok(fd) => {
      let result = fd.write(data, 0)
      match result {
        Err(e) => Err(e)
        Ok(_) => Ok(())
      }
    }
  }
}

///|
/// Writes a string to a file, creating or truncating it.
pub fn write_string(
  root : Descriptor,
  path : String,
  content : String,
) -> Result[Unit, @types.ErrorCode] {
  let bytes = FixedArray::make(content.iter().count(), b'\x00')
  let mut i = 0
  for c in content {
    bytes[i] = c.to_int().to_byte()
    i += 1
  }
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
    path,
    OpenFlags::default().set(DIRECTORY),
    DescriptorFlags::default().set(READ),
  )
  match dir {
    Err(e) => Err(e)
    Ok(fd) => {
      let stream = fd.read_directory()
      match stream {
        Err(e) => Err(e)
        Ok(s) => {
          let entries : Array[String] = []
          while true {
            match s.read_directory_entry() {
              Ok(Some(entry)) => entries.push(entry.name)
              Ok(None) => return Ok(entries)
              Err(e) => return Err(e)
            }
          }
          Ok(entries)
        }
      }
    }
  }
}
