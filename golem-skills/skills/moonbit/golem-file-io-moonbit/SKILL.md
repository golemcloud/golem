---
name: golem-file-io-moonbit
description: "Reading and writing files from MoonBit Golem agent code. Use when the user asks to read files, write files, or access the filesystem from a MoonBit agent."
---

# File I/O in MoonBit Golem Agents

Use `golemcloud/golem_sdk/filesystem` for Preview 3 filesystem access. Provision
initial files through `golem.yaml`; load `golem-add-initial-files` for that setup.
No WIT changes or binding regeneration are required in the consumer project.

## Setup

Add to `moon.pkg`:

```moonbit
import {
  "golemcloud/golem_sdk/filesystem" @fs,
  "golemcloud/golem_sdk/interface/wasi/filesystem/types" @fsTypes,
}
```

`@fs.get_root_dir()` returns the preopened directory mounted at `/`, or `None`.
Use `@fs.get_preopened_dir(path)` for another mount. Paths are relative to that
descriptor; SDK convenience functions also strip leading `/` characters.

## Reading, Writing and Listing

These helpers are async, as are methods and helpers that call them. MoonBit has
no `await` keyword. The examples return the filesystem `Result` so the caller
can handle errors instead of aborting on `.unwrap()`:

```moonbit
async fn read_file(path : String) -> Result[String, @fsTypes.ErrorCode] {
  let root = @fs.get_root_dir().unwrap()
  defer root.drop()
  @fs.read_string(root, path)
}

async fn read_binary(path : String) -> Result[FixedArray[Byte], @fsTypes.ErrorCode] {
  let root = @fs.get_root_dir().unwrap()
  defer root.drop()
  @fs.read_bytes(root, path)
}

async fn write_file(path : String, content : String) -> Result[Unit, @fsTypes.ErrorCode] {
  let root = @fs.get_root_dir().unwrap()
  defer root.drop()
  @fs.write_string(root, path, content)
}

async fn write_binary(path : String, data : FixedArray[Byte]) -> Result[Unit, @fsTypes.ErrorCode] {
  let root = @fs.get_root_dir().unwrap()
  defer root.drop()
  @fs.write_bytes(root, path, data)
}

async fn list_dir(path : String) -> Result[Array[String], @fsTypes.ErrorCode] {
  let root = @fs.get_root_dir().unwrap()
  defer root.drop()
  @fs.list_directory(root, path)
}
```

The examples require a `/` preopen. Handle `None` explicitly if your deployment
does not guarantee one. `read_string` decodes UTF-8 with replacement for invalid
sequences. Use `read_bytes` and a strict UTF-8 decoder to detect malformed text.
Writes create or truncate a file. Provisioned read-only files cannot be written.

## Complete Agent Example

```moonbit
#derive.agent
struct FileReader {
  name : String
}

fn FileReader::new(name : String) -> FileReader {
  { name }
}

pub async fn FileReader::read_text(self : Self, path : String) -> Result[String, String] {
  let root = @fs.get_root_dir().unwrap()
  defer root.drop()
  match @fs.read_string(root, path) {
    Ok(text) => Ok(text)
    Err(_) => Err("File read failed")
  }
}

pub async fn FileReader::write_text(self : Self, path : String, content : String) -> Result[Unit, String] {
  let root = @fs.get_root_dir().unwrap()
  defer root.drop()
  match @fs.write_string(root, path, content) {
    Ok(_) => Ok(())
    Err(_) => Err("File write failed")
  }
}

pub async fn FileReader::list_dir(self : Self, path : String) -> Result[Array[String], String] {
  let root = @fs.get_root_dir().unwrap()
  defer root.drop()
  match @fs.list_directory(root, path) {
    Ok(entries) => Ok(entries)
    Err(_) => Err("Directory listing failed")
  }
}

fn main {}
```

The agent maps the raw WASI error enum to a schema-supported application error
(`String`) instead of exposing `@fsTypes.ErrorCode` in its public agent interface.

## Low-Level Streams and Resource Ownership

Prefer the convenience wrappers: they consume the full stream, inspect its
completion result and drop descriptors they open. There is no Preview 2
`Descriptor::read(size, offset)` API or `(bytes, eof)` tuple in the current
contract. A stream can return multiple chunks regardless of file size.

For partial reads, use `Descriptor::read_via_stream(offset)`, which returns a
byte stream and a completion future. This fragment belongs inside an async
function, with `fd` an already-open descriptor:

```moonbit
let (contents, completed) = fd.read_via_stream(0)
defer contents.drop()
defer completed.drop()
let chunks : Array[FixedArray[Byte]] = []
for ;; {
  match contents.read(65536) {
    Some(chunk) => chunks.push(chunk)
    None => break
  }
}
// Stream end alone does not establish success. Inspect the completion result.
let result : Result[Unit, @fsTypes.ErrorCode] = completed.get()
```

Handle `result` and combine the chunks only on success. Release `fd` when its
owner is finished; use `defer fd.drop()` after opening it. Opening with
`open_at` requires a relative path plus `PathFlags`,
`OpenFlags` and `DescriptorFlags`. Writing uses `write_via_stream` with an async
producer and a completion future, not blocking writes. Directory listing uses
an entry stream and completion future, not the removed `DirectoryEntryStream`.

## Persistence

- Filesystems are isolated per agent instance.
- File changes persist across invocations of a durable agent.
- Read-only provisioned files remain read-only; writable files need the correct
  provisioning permissions.
- Use SDK wrappers unless partial-read or streaming behavior is required. Their
  bounded chunk reads collect the entire file, not just the first 64 KiB.
