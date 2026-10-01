---
name: golem-add-go-module
description: "Add a Go module dependency to a Go Golem project. Use when the user asks to add a library, package, module, or dependency in Go."
---

# Add a Go Module Dependency

## Important constraints

- The component compiles to a WebAssembly component (WASI) via `componentize-go` — only code that works on that target will build.
- Modules that use OS threads, raw sockets (`net.Dial`), most database drivers, `cgo`, or platform-specific syscalls **will not work**.
- Pure-Go modules, and modules that do I/O through the standard library the SDK adapts (e.g. HTTP via `net/http`), generally work.
- If unsure whether a module builds for WASM, add it and run `golem build` to find out.

## Steps

1. **Add the dependency** in the component's module directory (where its `go.mod` lives):

   ```shell
   go get github.com/example/mod@latest
   ```

   Then import and use it in your code. `go get` records it in `go.mod`.

2. **Build to verify**

   ```shell
   golem build --yes
   ```

   `golem build` runs `go mod tidy` for you before compiling. Do NOT rely on a bare `go build` for the component — it can't link the WASI component; always use `golem build`.

3. **If the build fails**

   - Look for `wasip1`/`wasm`-unsupported errors, references to `net`, `syscall`, or C dependencies — those modules are incompatible with the WASM target.
   - Check whether the module offers a pure-Go or `wasm` build mode/build tag.
   - Look for an alternative module that supports WASM.

## Already available (do NOT re-add)

- `github.com/golemcloud/golem/sdks/go/golem` — the Golem Go SDK (agents, durability, RPC, retry, config/secrets, keyvalue/blobstore/rdbms/websocket, logging). Its subpackages (`.../golem/retry`, `.../golem/keyvalue`, etc.) are part of the same module.
- `github.com/bytecodealliance/componentize-go` — pinned as a **`tool`** dependency in `go.mod` so `go tool componentize-go` runs the pinned version. Leave the `tool` directive in place; don't remove it.

## HTTP and networking

Use the standard `net/http` package — the SDK routes it through the durable WASI HTTP transport (see `golem-make-http-request-go`). Raw sockets (`net.Dial`, `crypto/tls` over a custom conn) are **not** available on the WASM target.

### Related Skills

| Skill | When to Load |
|-------|--------------|
| `golem-make-http-request-go` | Call an external service over HTTP |
| `golem-add-agent-go` | Use a new dependency inside an agent |
