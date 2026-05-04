---
name: golem-file-io-moonbit
description: "Reading and writing files from MoonBit Golem agent code. Use when the user asks to read files, write files, or access the filesystem from a MoonBit agent."
---

# File I/O in MoonBit Golem Agents

## Overview

MoonBit Golem agents can access files provisioned into the agent's filesystem via `golem.yaml`. File access uses the SDK's `@fs` package (`golemcloud/golem_sdk/filesystem`), which provides convenience functions and re-exports of the necessary types.

To provision files into an agent's filesystem, load the `golem-add-initial-files` skill.

## Prerequisites: Adding the Dependency

Add the filesystem package to your agent's `moon.pkg`:

```
import {
  "golemcloud/golem_sdk/filesystem" @fs,
}
```

No WIT changes or binding regeneration is needed — the SDK already includes the filesystem imports.

## File Provisioning

Files must first be provisioned via `golem.yaml` (see `golem-add-initial-files` skill). Provisioned files are available through preopened directories. The preopened directory for provisioned files is typically mounted at `/`.

## Reading Files

### Getting the Root Directory

Use `@fs.get_root_dir()` to get the preopened directory at `/`:

```moonbit
///|
fn get_root() -> @fs.Descriptor {
  @fs.get_root_dir().unwrap()
}
```

Or use `@fs.get_preopened_dir(path)` for a specific mount point.

### Reading a Text File

Use the convenience function `@fs.read_string`:

```moonbit
///|
fn read_file(path : String) -> String!Error {
  let root = @fs.get_root_dir().unwrap()
  @fs.read_string(root, path).unwrap()
}
```

### Reading a Binary File

Use `@fs.read_bytes`:

```moonbit
///|
fn read_binary(path : String) -> FixedArray[Byte]!Error {
  let root = @fs.get_root_dir().unwrap()
  @fs.read_bytes(root, path).unwrap()
}
```

## Writing Files

Only files provisioned with `read-write` permission (or files in non-provisioned paths) can be written to.

### Writing a Text File

```moonbit
///|
fn write_file(path : String, content : String) -> Unit!Error {
  let root = @fs.get_root_dir().unwrap()
  @fs.write_string(root, path, content).unwrap()
}
```

### Writing Binary Data

```moonbit
///|
fn write_binary(path : String, data : FixedArray[Byte]) -> Unit!Error {
  let root = @fs.get_root_dir().unwrap()
  @fs.write_bytes(root, path, data).unwrap()
}
```

## Listing Directory Entries

```moonbit
///|
fn list_dir(path : String) -> Array[String]!Error {
  let root = @fs.get_root_dir().unwrap()
  @fs.list_directory(root, path).unwrap()
}
```

## Low-Level Access

For more control (e.g., opening with specific flags, using streams, stat), use the re-exported types directly from `@fs`:

```moonbit
///|
fn open_read_only(path : String) -> @fs.Descriptor!Error {
  let root = @fs.get_root_dir().unwrap()
  root
    .open_at(
      @fs.PathFlags::default(),
      path,
      @fs.OpenFlags::default(),
      @fs.DescriptorFlags::default().set(@fs.READ),
    )
    .unwrap()
}
```

Available re-exported types from `@fs`: `Descriptor`, `DirectoryEntryStream`, `DescriptorFlags`, `PathFlags`, `OpenFlags`, `ErrorCode`, `DirectoryEntry`, `DescriptorStat`, `NewTimestamp`, `DescriptorType`, `Advice`, `MetadataHashValue`, `DescriptorFlagsFlag` (`READ`, `WRITE`, etc.), `PathFlagsFlag`, `OpenFlagsFlag` (`CREATE`, `DIRECTORY`, `EXCLUSIVE`, `TRUNCATE`).

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
  let root = @fs.get_root_dir().unwrap()
  @fs.read_string(root, path).unwrap()
}

/// Writes content to a file (must be writable)
pub fn FileReader::write_text(self : Self, path : String, content : String) -> Unit {
  let _ = self
  let root = @fs.get_root_dir().unwrap()
  @fs.write_string(root, path, content).unwrap()
}

/// Lists entries in a directory
pub fn FileReader::list_dir(self : Self, path : String) -> Array[String] {
  let _ = self
  let root = @fs.get_root_dir().unwrap()
  @fs.list_directory(root, path).unwrap()
}
```

## Key Constraints

- Files provisioned via `golem-add-initial-files` with `read-only` permission cannot be written to
- The filesystem is per-agent-instance — each agent has its own isolated filesystem
- File changes within an agent are persistent across invocations (durable state)
- All paths are relative to a preopened directory descriptor — there is no global filesystem root; you must obtain a descriptor via `get_root_dir()` or `get_preopened_dir(path)`
