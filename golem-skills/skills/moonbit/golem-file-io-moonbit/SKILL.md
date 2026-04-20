---
name: golem-file-io-moonbit
description: "Reading and writing files from MoonBit Golem agent code. Use when the user asks to read files, write files, or access the filesystem from a MoonBit agent."
---

# File I/O in MoonBit Golem Agents

## Overview

MoonBit Golem agents can access files provisioned into the agent's filesystem via `golem.yaml`. File access uses the WASI filesystem API (`wasi:filesystem/types` and `wasi:filesystem/preopens`), which is available through the SDK's generated WIT bindings.

To provision files into an agent's filesystem, load the `golem-add-initial-files` skill.

## Prerequisites: Enabling Filesystem Imports

The `wasi:filesystem` interfaces are defined in the SDK's WIT dependencies (`golem_sdk/wit/deps/filesystem/`) but are **not imported by default** in the `agent-guest` world. You must add the imports to `golem_sdk/wit/main.wit`:

```wit
world agent-guest {
  // ... existing imports ...

  import wasi:filesystem/types@0.2.3;
  import wasi:filesystem/preopens@0.2.3;

  // ... existing exports ...
}
```

Then regenerate the WIT bindings:

```sh
cd golem_sdk
wit-bindgen moonbit ./wit --derive-show --derive-eq --derive-error --project-name golemcloud/golem_sdk --ignore-stub
moon fmt
```

This generates MoonBit bindings under `interface/wasi/filesystem/` with types like `Descriptor`, `DescriptorFlags`, `OpenFlags`, and functions like `get_directories()`.

## File Provisioning

Files must first be provisioned via `golem.yaml` (see `golem-add-initial-files` skill). Provisioned files are available through WASI preopened directories. The preopened directory for provisioned files is typically mounted at `/`.

## Reading Files

### Getting a Preopened Directory

Use `wasi:filesystem/preopens` to get access to the filesystem root:

```moonbit
///|
fn get_root_dir() -> @wasi.filesystem.types.Descriptor? {
  let dirs = @wasi.filesystem.preopens.get_directories()
  for pair in dirs {
    let (descriptor, path) = pair
    if path == "/" {
      return Some(descriptor)
    }
  }
  None
}
```

### Reading a Text File

Use `open-at` on the preopened directory descriptor, then `read` from the file descriptor:

```moonbit
///|
fn read_file(path : String) -> String!Error {
  let root = get_root_dir().unwrap()
  let file = root
    .open_at(
      @wasi.filesystem.types.PathFlags::default(),
      path,
      @wasi.filesystem.types.OpenFlags::default(),
      { read: true, ..@wasi.filesystem.types.DescriptorFlags::default() },
    )
    .unwrap()
  let stat = file.stat().unwrap()
  let bytes = file.read(stat.size, 0).unwrap()
  // Convert bytes to string
  let buf = Buffer::new()
  for b in bytes {
    buf.write_byte(b)
  }
  buf.to_string()
}
```

### Reading a Binary File

```moonbit
///|
fn read_binary(path : String) -> Bytes!Error {
  let root = get_root_dir().unwrap()
  let file = root
    .open_at(
      @wasi.filesystem.types.PathFlags::default(),
      path,
      @wasi.filesystem.types.OpenFlags::default(),
      { read: true, ..@wasi.filesystem.types.DescriptorFlags::default() },
    )
    .unwrap()
  let stat = file.stat().unwrap()
  let data = file.read(stat.size, 0).unwrap()
  Bytes::from_array(data)
}
```

## Writing Files

Only files provisioned with `read-write` permission (or files in non-provisioned paths) can be written to.

```moonbit
///|
fn write_file(path : String, content : String) -> Unit!Error {
  let root = get_root_dir().unwrap()
  let file = root
    .open_at(
      @wasi.filesystem.types.PathFlags::default(),
      path,
      { create: true, truncate: true, ..@wasi.filesystem.types.OpenFlags::default() },
      { write: true, ..@wasi.filesystem.types.DescriptorFlags::default() },
    )
    .unwrap()
  let bytes = content.to_array().map(fn(c) { c.to_int().to_byte() })
  let _ = file.write(bytes, 0).unwrap()
}
```

## Listing Directory Entries

```moonbit
///|
fn list_directory(path : String) -> Array[String]!Error {
  let root = get_root_dir().unwrap()
  let dir = root
    .open_at(
      @wasi.filesystem.types.PathFlags::default(),
      path,
      { directory: true, ..@wasi.filesystem.types.OpenFlags::default() },
      { read: true, ..@wasi.filesystem.types.DescriptorFlags::default() },
    )
    .unwrap()
  let stream = dir.read_directory().unwrap()
  let entries : Array[String] = []
  loop {
    match stream.read_directory_entry() {
      Ok(Some(entry)) => entries.push(entry.name)
      Ok(None) => break
      Err(_) => break
    }
  }
  entries
}
```

## Complete Agent Example

```moonbit
/// File reader agent that reads provisioned files
#derive.agent
struct FileReader {
  name : String
}

fn FileReader::new(name : String) -> FileReader {
  { name }
}

/// Reads the content of a provisioned text file
pub fn FileReader::read_text(self : Self, path : String) -> String {
  let _ = self
  let root = get_root_dir().unwrap()
  let file = root
    .open_at(
      @wasi.filesystem.types.PathFlags::default(),
      path,
      @wasi.filesystem.types.OpenFlags::default(),
      { read: true, ..@wasi.filesystem.types.DescriptorFlags::default() },
    )
    .unwrap()
  let stat = file.stat().unwrap()
  let bytes = file.read(stat.size, 0).unwrap()
  let buf = Buffer::new()
  for b in bytes {
    buf.write_byte(b)
  }
  buf.to_string()
}

/// Writes content to a file (must be writable)
pub fn FileReader::write_text(self : Self, path : String, content : String) -> Unit {
  let _ = self
  let root = get_root_dir().unwrap()
  let file = root
    .open_at(
      @wasi.filesystem.types.PathFlags::default(),
      path,
      { create: true, truncate: true, ..@wasi.filesystem.types.OpenFlags::default() },
      { write: true, ..@wasi.filesystem.types.DescriptorFlags::default() },
    )
    .unwrap()
  let bytes = content.to_array().map(fn(c) { c.to_int().to_byte() })
  let _ = file.write(bytes, 0).unwrap()
}

fn get_root_dir() -> @wasi.filesystem.types.Descriptor? {
  let dirs = @wasi.filesystem.preopens.get_directories()
  for pair in dirs {
    let (descriptor, path) = pair
    if path == "/" {
      return Some(descriptor)
    }
  }
  None
}
```

## Key Constraints

- **Filesystem imports must be added** — `wasi:filesystem/types` and `wasi:filesystem/preopens` must be imported in the `agent-guest` world and bindings regenerated before filesystem operations are available
- Files provisioned via `golem-add-initial-files` with `read-only` permission cannot be written to
- The filesystem is per-agent-instance — each agent has its own isolated filesystem
- File changes within an agent are persistent across invocations (durable state)
- All paths in WASI are relative to a preopened directory descriptor — there is no global filesystem root; you must obtain a descriptor via `get_directories()`
- MoonBit does not have a `std::fs` equivalent — all file operations go through the WASI filesystem bindings directly
- The exact MoonBit binding names (e.g., `@wasi.filesystem.types.Descriptor`) depend on the generated code — check the generated `interface/wasi/filesystem/` directory after running `wit-bindgen`
