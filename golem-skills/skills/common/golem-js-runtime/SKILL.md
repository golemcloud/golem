---
name: golem-js-runtime
description: "JavaScript runtime environment for TypeScript and Scala Golem agents. Use when asking about available APIs, Node.js modules, browser APIs, or runtime capabilities in the QuickJS-based WASM environment."
---

# Golem JavaScript Runtime (QuickJS)

Both TypeScript and Scala (via Scala.js) Golem agents compile to JavaScript and run inside a [QuickJS](https://github.com/DelSkayn/rquickjs/)-based WebAssembly runtime provided by [wasm-rquickjs](https://github.com/golemcloud/wasm-rquickjs).

## Language Support

The runtime supports **ES2020** including modules, async/await, async generators, Proxies, BigInt, WeakRef, FinalizationRegistry, and all standard built-ins (Array, Map, Set, Promise, RegExp, Date, JSON, Math, typed arrays, etc.).

## Available Web Platform APIs

The following browser/web APIs are available out of the box — no imports needed:

- **HTTP**: `fetch`, `Headers`, `Request`, `Response`, `FormData`
- **Files/Blobs**: `Blob`, `File`
- **URLs**: `URL`, `URLSearchParams`
- **Console**: `console`
- **Timers**: `setTimeout`/`clearTimeout`, `setInterval`/`clearInterval`, `setImmediate`
- **Abort**: `AbortController`, `AbortSignal`
- **Encoding**: `TextEncoder`, `TextDecoder`, `TextEncoderStream`, `TextDecoderStream`
- **Streams**: `ReadableStream`, `WritableStream`, `TransformStream`
- **Structured clone**: `structuredClone`
- **Crypto**: `crypto.randomUUID`, `crypto.getRandomValues`
- **Events**: `Event`, `EventTarget`, `CustomEvent`
- **Messaging**: `MessageChannel`, `MessagePort`
- **Errors**: `DOMException`
- **Internationalization**: `Intl` (DateTimeFormat, NumberFormat, Collator, PluralRules)

## Available Node.js Modules

These Node.js-compatible modules can be imported:

| Module | Description |
|--------|-------------|
| `node:buffer` | Buffer API |
| `node:crypto` | Hashes, HMAC, ciphers, key generation, sign/verify, DH, ECDH, X509, etc. |
| `node:dgram` | UDP sockets |
| `node:dns` | DNS resolution |
| `node:events` | EventEmitter |
| `node:fs`, `node:fs/promises` | Comprehensive filesystem API (read, write, stat, mkdir, readdir, etc.) |
| `node:http`, `node:https` | HTTP client and server |
| `node:module` | Module utilities |
| `node:net` | TCP sockets and servers |
| `node:os` | OS information |
| `node:path` | Path manipulation |
| `node:perf_hooks` | Performance measurement |
| `node:process` | Process information |
| `node:punycode` | Punycode encoding |
| `node:querystring` | Query string parsing |
| `node:readline` | Readline interface |
| `node:sqlite` | Embedded SQLite (requires feature flag) |
| `node:stream`, `node:stream/promises` | Stream API |
| `node:string_decoder` | String decoding |
| `node:test` | Test runner |
| `node:timers` | Timer APIs |
| `node:url` | URL parsing |
| `node:util` | Utility functions |
| `node:v8` | V8 compatibility shim |
| `node:vm` | Script evaluation |
| `node:zlib` | Compression (gzip, deflate, brotli) |

## Stub Modules (Throw or No-op)

These modules exist for compatibility but will throw or no-op when used:

- `node:child_process`
- `node:cluster`
- `node:http2`
- `node:inspector`
- `node:tls`
- `node:worker_threads`

## npm Package Compatibility

Additional npm packages can be installed with `npm install` (TypeScript) or added as Scala.js-compatible dependencies (Scala). Most packages targeting browsers or using the Node.js APIs listed above will work. Packages that depend on native C/C++ bindings or JVM-specific APIs will **not** work.

## File I/O

Use `node:fs` or `node:fs/promises` for filesystem operations. This is the standard way to read/write files in both TypeScript and Scala agents.

### TypeScript

```typescript
import * as fs from 'node:fs';

// Synchronous
const content = fs.readFileSync('/data/config.json', 'utf-8');

// Async
import * as fsp from 'node:fs/promises';
const data = await fsp.readFile('/data/config.json', 'utf-8');
```

### Scala (via Scala.js interop)

```scala
import scala.scalajs.js
import scala.scalajs.js.annotation.JSImport

@js.native
@JSImport("node:fs", JSImport.Namespace)
private object Fs extends js.Object {
  def readFileSync(path: String, encoding: String): String = js.native
  def writeFileSync(path: String, data: String): Unit = js.native
  def existsSync(path: String): Boolean = js.native
}

// Usage
val content = Fs.readFileSync("/data/config.json", "utf-8")
```

> **Important (Scala.js):** WASI modules like `node:fs` are **not** available during the build-time pre-initialization (wizer) phase — they are only available at runtime. If you import `node:fs` at the top level of a Scala.js module, the import executes during pre-initialization and will fail. Use **lazy initialization** (`lazy val`) or defer the import to method bodies to ensure the module is loaded at runtime:
>
> ```scala
> // ✅ CORRECT — lazy val defers initialization to first use at runtime
> private lazy val fs: Fs.type = Fs
>
> // ❌ WRONG — top-level val triggers import during pre-initialization
> private val fs: Fs.type = Fs
> ```

## Reference

Check the [wasm-rquickjs README](https://github.com/golemcloud/wasm-rquickjs) for the most up-to-date list of available APIs.
