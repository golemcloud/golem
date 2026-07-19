---
name: golem-file-io-effect
description: "Explains the Effect Golem runtime's file-I/O boundary and uses Blobstore for supported file-like byte and text storage. Use when an Effect agent must read, write, list, or checksum data, or when a task asks it to access provisioned Initial File System paths."
---

# File-Like I/O in Effect Golem Agents

Effect agents run in a QuickJS-based WebAssembly component, not Node.js. The current
`@golemcloud/effect-golem` component does **not** expose a general filesystem API to application
code. Use the SDK's `Blobstore` API for application-owned bytes and text.

## Initial File System Is Not Available

Files declared under `files:` in `golem.yaml` are provisioned into a WASI Initial File System,
but the Effect SDK's `agent-guest` world does not import `wasi:filesystem/types` or
`wasi:filesystem/preopens`. It also ships no `Filesystem` wrapper. Consequently, an Effect agent
cannot currently open a provisioned path such as `/data/config.json`.

Follow these rules:

- Do **not** import `node:fs` or `node:fs/promises`; unrestricted Node filesystem modules are not
  available in the QuickJS component.
- Do **not** import `wasi:filesystem/preopens@0.2.3` or
  `wasi:filesystem/types@0.2.3`. Rollup preserves `wasi:*` imports, but the base component has no
  native modules for these two interfaces, so such imports cannot resolve at runtime.
- Do **not** hard-code known contents or an expected checksum inside a read method to disguise the
  missing filesystem access. A Blobstore adaptation must ingest bytes in the outer initialization
  Effect or through a separate write method before the read method processes the stored object.
- If access to an existing provisioned path is mandatory, report the SDK capability gap. The SDK
  must add the filesystem interfaces to its WIT world, regenerate its declarations and base WASM,
  and expose a supported API before the task can be implemented.
- Loading `golem-add-initial-files` can configure the manifest, but manifest provisioning alone
  does not make the file readable from an Effect component.

This limitation is visible in the pinned SDK's
[`wit/main.wit`](https://github.com/golemcloud/effect-golem/blob/4b75f5e4d3cc306c3df75050db93d93aaa379ec3/wit/main.wit)
and public
[`src/index.ts`](https://github.com/golemcloud/effect-golem/blob/4b75f5e4d3cc306c3df75050db93d93aaa379ec3/src/index.ts).

## Use Blobstore for Supported File-Like Storage

When the data producer can write through an agent method, model a file as a blob object:

| Filesystem concept | Supported Blobstore equivalent |
|---|---|
| isolated storage owner | container named from the agent identity |
| file name | object key |
| binary read/write | `container.getData(key)` / `container.writeData(key, bytes)` |
| existence check | `container.hasObject(key)` |
| metadata | `container.objectInfo(key)` |
| list stored names | `container.listObjects` stream |
| delete | `container.deleteObject(key)` |

Blobstore is not a mounted filesystem: object keys are not paths, there are no directories or
open file descriptors, and it cannot read a file provisioned through `golem.yaml`.

Acquire a container once in the outer `.implement(...)` Effect and close over it in the method
handlers. The agent runtime keeps that constructor scope alive for the agent instance. Do not wrap
this acquisition in `Effect.scoped`, which would close the resource before later invocations.

## Complete Text and Binary Example

Load `golem-add-agent-effect` when adding the agent definition itself.

```typescript
import { Effect, Schema, Stream } from "effect";
import { Blobstore, defineAgent, method } from "@golemcloud/effect-golem";

export const FileObjectAgent = defineAgent({
  name: "FileObjectAgent",
  mode: "durable",
  constructorParams: {
    name: Schema.String,
  },
  methods: {
    putText: method({
      params: { key: Schema.String, content: Schema.String },
      success: Schema.Void,
    }),
    readText: method({
      params: { key: Schema.String },
      success: Schema.String,
    }),
    exists: method({
      params: { key: Schema.String },
      success: Schema.Boolean,
    }),
    fileStats: method({
      params: { key: Schema.String },
      success: Schema.String,
    }),
    listKeys: method({
      params: {},
      success: Schema.Array(Schema.String),
    }),
    deleteKey: method({
      params: { key: Schema.String },
      success: Schema.Void,
    }),
  },
}).implement(({ name }) =>
  Effect.gen(function* () {
    const container = yield* Blobstore.getOrCreateContainer(name);
    const encoder = new TextEncoder();
    const decoder = new TextDecoder();

    return {
      putText: ({ key, content }) =>
        container.writeData(key, encoder.encode(content)),

      readText: ({ key }) =>
        container.getData(key).pipe(
          Effect.map((bytes) => decoder.decode(bytes)),
        ),

      exists: ({ key }) => container.hasObject(key),

      fileStats: ({ key }) =>
        container.getData(key).pipe(
          Effect.map((bytes) => {
            let sum = 0n;
            for (const byte of bytes) sum += BigInt(byte);
            return `size=${bytes.byteLength},sum=${sum}`;
          }),
        ),

      listKeys: () =>
        Stream.runCollect(container.listObjects).pipe(
          Effect.map((keys) => keys.slice()),
        ),

      deleteKey: ({ key }) => container.deleteObject(key),
    };
  }),
);
```

Register the implementation from the component entry point:

```typescript
// src/main.ts
import "./file-object-agent.js";
```

For arbitrary binary data, skip text encoding and pass a `Uint8Array` directly to
`container.writeData`; `container.getData` returns the complete object as a `Uint8Array`.

### Large Binary Objects

Use the same whole-object APIs for data larger than 64 KiB. `container.writeData` handles the
host's write chunking, and `container.getData(key)` reads the complete object. The 64 KiB
`Descriptor.read` cap that applies to low-level `wasi:filesystem` descriptors is not an API an
Effect component can use and does not require a manual read loop around Blobstore.

When adapting a test or application that previously relied on a provisioned fixture, keep data
creation separate from data reading. Ingest the bytes in the outer agent initialization Effect or
expose a separate write method that stores them with `writeData`; the read method can then call
`getData` and process the returned bytes. This verifies real host-backed I/O without pretending
that a Blobstore key is a provisioned filesystem path.

## Key Constraints

- Import Effect APIs from `effect` and `Blobstore` from `@golemcloud/effect-golem`.
- Return Effects from method handlers; do not use plain `async` functions.
- Use `Blobstore.getOrCreateContainer`, not create-only logic, so reconstruction and snapshot
  restoration can reopen existing data safely.
- Container names determine storage sharing. Derive the name from constructor identity when each
  durable agent instance should own separate objects.
- Let Golem handle durable host-operation retries; do not add manual retry loops.
- Use TypeScript camelCase for Effect method names and TypeScript syntax for CLI agent values.
- Treat a request for a provisioned filesystem path as blocked, not as a request to silently
  substitute Blobstore. Offer Blobstore only when changing the data-ingestion contract is allowed.
