---
name: golem-file-io-effect
description: "Reading and writing files from Effect Golem agents with wasm-rquickjs Node filesystem modules, including provisioned Initial File System paths and large files. Use when an Effect agent must read, write, list, or checksum filesystem data."
---

# File I/O in Effect Golem Agents

Effect agents run in wasm-rquickjs, which implements Node's filesystem APIs on top of WASI. Import
`node:fs` or `node:fs/promises` exactly as in an ordinary TypeScript Golem component. The component
build keeps wasm-rquickjs-provided Node modules external, while installed third-party npm packages
remain bundleable.

## Read a Provisioned File

Load `golem-add-initial-files` to provision a local file through the agent's `files:` manifest
section. Agent code reads the configured `targetPath`; it must not use the local `sourcePath`.

Use `Effect.promise` when failure is impossible by contract, or `Effect.tryPromise` when the method
must model filesystem errors. Method handlers must return an Effect rather than being declared
`async`:

```typescript
import { readFile } from "node:fs/promises";
import { Effect, Schema } from "effect";
import { defineAgent, method } from "@golemcloud/effect-golem";

export const ProvisionedFileAgent = defineAgent({
  name: "ProvisionedFileAgent",
  mode: "durable",
  constructorParams: {
    name: Schema.String,
  },
  methods: {
    readConfig: method({
      params: {},
      success: Schema.String,
    }),
    fileStats: method({
      params: {},
      success: Schema.String,
    }),
  },
}).implement(() =>
  Effect.succeed({
    readConfig: () =>
      Effect.promise(() => readFile("/data/config.json", "utf8")),

    fileStats: () =>
      Effect.promise(() => readFile("/data/large.bin")).pipe(
        Effect.map((bytes) => {
          let sum = 0n;
          for (const byte of bytes) sum += BigInt(byte);
          return `size=${bytes.byteLength},sum=${sum}`;
        }),
      ),
  }),
);
```

Register the implementation from the component entry point:

```typescript
// src/main.ts
import "./provisioned-file-agent.js";
```

`readFile` returns a `Buffer`, which is a `Uint8Array`, and wasm-rquickjs performs the low-level
chunked reads needed for files larger than the WASI per-read allocation limit. Do not truncate the
result or replace a provisioned-file read with generated fixture bytes.

## Other Node Filesystem Operations

Use the usual `node:fs/promises` operations (`writeFile`, `mkdir`, `readdir`, `stat`, `unlink`, and
others) and wrap their Promises in Effect. The synchronous `node:fs` APIs are also available; wrap
them in `Effect.sync` or `Effect.try`. Paths are inside the agent's sandboxed WASI filesystem, not
the developer machine.

Use Blobstore instead when the data is logically object storage rather than filesystem data. A
Blobstore key is not a mounted path and cannot read a file provisioned through `golem.yaml`.

## Key Constraints

- Import filesystem APIs from `node:fs` or `node:fs/promises`.
- Return Effects from method handlers; do not use plain `async` functions.
- Use the provisioned `targetPath`, such as `/data/config.json`, in application code.
- Let Golem handle durable host-operation retries; do not add manual retry loops.
- Use TypeScript camelCase for Effect method names and TypeScript syntax for CLI agent values.
- Installed third-party npm packages are bundled normally, but native Node addons cannot run in
  the WebAssembly component; choose pure JavaScript dependencies or wasm-rquickjs Node APIs.
