---
name: golem-file-io-rust
description: "Reading and writing files from a Rust Golem agent. Use when the user asks to read files, write files, or do filesystem operations from agent code in Rust."
---

# File I/O in Rust Golem Agents

## Overview

Golem Rust agents compile to `wasm32-wasip1` which provides WASI filesystem access. Use the standard `std::fs` module for all filesystem operations — it works out of the box with WASI.

To provision files into an agent's filesystem, load the `golem-add-initial-files` skill.

## Reading Files

### Text Files

```rust
use std::fs;

let content = fs::read_to_string("/data/config.json")
    .expect("Failed to read file");
```

### Binary Files

```rust
use std::fs;

let bytes = fs::read("/data/image.png")
    .expect("Failed to read file");
```

## Writing Files

Only files provisioned with `read-write` permission (or files in non-provisioned paths) can be written to.

```rust
use std::fs;

fs::write("/tmp/output.txt", "Hello, world!")
    .expect("Failed to write file");
```

### Appending to Files

```rust
use std::fs::OpenOptions;
use std::io::Write;

let mut file = OpenOptions::new()
    .append(true)
    .create(true)
    .open("/tmp/agent.log")
    .expect("Failed to open file");

writeln!(file, "Log message").expect("Failed to write");
```

## Checking File Existence

```rust
use std::path::Path;

if Path::new("/data/config.json").exists() {
    let content = std::fs::read_to_string("/data/config.json").unwrap();
}
```

## Listing Directories

```rust
use std::fs;

let entries = fs::read_dir("/data").expect("Failed to read directory");
for entry in entries {
    let entry = entry.expect("Failed to read entry");
    println!("{}", entry.file_name().to_string_lossy());
}
```

## Complete Agent Example

```rust
use golem_rust::{agent_definition, agent_implementation};
use std::fs;
use std::io::Write;

#[agent_definition]
pub trait FileReaderAgent {
    fn new(name: String) -> Self;
    fn read_greeting(&self) -> String;
    fn write_log(&mut self, message: String);
}

struct FileReaderAgentImpl {
    name: String,
}

#[agent_implementation]
impl FileReaderAgent for FileReaderAgentImpl {
    fn new(name: String) -> Self {
        Self { name }
    }

    fn read_greeting(&self) -> String {
        fs::read_to_string("/data/greeting.txt")
            .expect("Failed to read greeting")
            .trim()
            .to_string()
    }

    fn write_log(&mut self, message: String) {
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open("/tmp/agent.log")
            .expect("Failed to open log");
        writeln!(file, "{}", message).expect("Failed to write log");
    }
}
```

## Key Constraints

- Use standard `std::fs` — it works with WASI out of the box
- Files provisioned via `golem-add-initial-files` with `read-only` permission cannot be written to
- The filesystem is per-agent-instance — each agent has its own isolated filesystem
- File changes within an agent are persistent across invocations (durable state)
- Third-party crates that use `std::fs` (e.g., `serde_json`'s file reading) also work
