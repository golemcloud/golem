---
name: golem-add-npm-package
description: "Add a new npm package dependency to a TypeScript Golem project. Use when the user asks to add a library, package, or dependency."
---

# Add an NPM Package Dependency

## Important constraints

- Golem TypeScript runs inside a [QuickJS](https://github.com/DelSkayn/rquickjs/) WebAssembly runtime, NOT Node.js.
- The runtime implements a broad set of Browser and Node.js APIs. Most packages targeting browsers or standard Node.js APIs will work.
- Packages that use native Node.js C++ addons **will not work** (no native compilation in WASM).
- Some modules are stubs that throw for compatibility: `node:child_process`, `node:cluster`, `node:http2`, `node:worker_threads`.

### Supported runtime APIs

- **Web Platform APIs**: `fetch`, `Headers`, `Request`, `Response`, `FormData`, `Blob`, `File`, `URL`, `URLSearchParams`, `console`, timers, `AbortController`, `TextEncoder`/`TextDecoder`, Streams (`ReadableStream`, `WritableStream`, `TransformStream`), `structuredClone`, `crypto.randomUUID`/`crypto.getRandomValues`, `Event`/`EventTarget`, `MessageChannel`/`MessagePort`, `Intl`
- **Node.js modules**: `node:buffer`, `node:crypto` (hashes, HMAC, ciphers, key generation, sign/verify, DH, ECDH, X509), `node:dgram` (UDP), `node:dns`, `node:events`, `node:fs` and `node:fs/promises` (comprehensive filesystem), `node:http`/`node:https` (client and server), `node:net` (TCP sockets and servers), `node:os`, `node:path`, `node:perf_hooks`, `node:process`, `node:querystring`, `node:readline`, `node:sqlite` (embedded, requires feature flag), `node:stream`, `node:test`, `node:timers`, `node:url`, `node:util`, `node:v8`, `node:vm`, `node:zlib` (gzip, deflate, brotli)

Check the [wasm-rquickjs README](https://github.com/golemcloud/wasm-rquickjs) for the up-to-date list.

## Steps

1. **Install the package**

   From the project root (where `package.json` is):

   ```shell
   npm install <package-name>
   ```

   For dev-only dependencies (build tools, type definitions):

   ```shell
   npm install --save-dev <package-name>
   ```

2. **Build to verify**

   ```shell
   golem build
   ```

   Do NOT run `npx rollup` or `npx tsc` directly — always use `golem build`.

3. **If the build or runtime fails**

   - **Build error**: the package may use TypeScript features or module formats incompatible with the Rollup bundling pipeline. Check if the package has an ESM or browser build.
   - **Runtime error (`X is not defined`)**: the package depends on a Node.js or browser API not available in the QuickJS runtime. Look for an alternative package.

## Already available packages

These are already in the project's `package.json` — do NOT add them again:

- `@golemcloud/golem-ts-sdk` — Golem agent framework, durability, transactions, RPC
- `@golemcloud/golem-ts-typegen` (dev) — type metadata generation
- `rollup` with plugins (dev) — bundling pipeline
- `typescript` (dev) — TypeScript compiler

## HTTP requests

Use the built-in `fetch` API — it is available globally. 

## AI / LLM features

Use any third party AI/LLM library from npmjs.com, but it has to only depend on the _supported_ runtime APIs.
