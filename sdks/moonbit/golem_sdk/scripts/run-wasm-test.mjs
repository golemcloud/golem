#!/usr/bin/env node

import fs from "node:fs"

const wasmPath = process.argv[2]
if (!wasmPath) {
  console.error("usage: node scripts/run-wasm-test.mjs <test.wasm>")
  process.exit(2)
}

const bytes = fs.readFileSync(wasmPath)
const module = new WebAssembly.Module(bytes)
const exceptionTag = new WebAssembly.Tag({ parameters: [] })
let instance
let componentContext = 0
let nextWaitableSet = 1
const resourceDrops = {
  secret: 0,
  "quota-token": 0,
  "permission-card": 0,
}

const rootImports = new Proxy(
  {},
  {
    get(_target, name) {
      switch (name) {
        case "[context-get-0]":
          return () => componentContext
        case "[context-set-0]":
          return value => {
            componentContext = value
          }
        case "[waitable-set-new]":
          return () => nextWaitableSet++
        case "[stream-new-unit]":
          return () => 0n
        default:
          return () => 0
      }
    },
  },
)

const importObject = {
  exception: {
    tag: exceptionTag,
    throw() {
      throw new WebAssembly.Exception(exceptionTag, [])
    },
  },
  wasi_snapshot_preview1: {
    fd_write(_fd, iovecs, iovecCount, bytesWritten) {
      const memory = new DataView(instance.exports.memory.buffer)
      const output = []
      let total = 0
      for (let index = 0; index < iovecCount; index++) {
        const base = iovecs + index * 8
        const pointer = memory.getUint32(base, true)
        const length = memory.getUint32(base + 4, true)
        total += length
        output.push(
          new TextDecoder().decode(
            new Uint8Array(instance.exports.memory.buffer, pointer, length),
          ),
        )
      }
      memory.setUint32(bytesWritten, total, true)
      process.stdout.write(output.join(""))
      return 0
    },
  },
  __moonbit_fs_unstable: {
    begin_read_string(value) {
      return value
    },
    string_read_char() {
      return -1
    },
    finish_read_string() {},
  },
  "$root": rootImports,
  "[export]$root": rootImports,
  "golem:test": {
    "reset-resource-drop-counts"() {
      for (const kind of Object.keys(resourceDrops)) {
        resourceDrops[kind] = 0
      }
    },
    "resource-drop-count"(kind) {
      switch (kind) {
        case 0:
          return resourceDrops.secret
        case 1:
          return resourceDrops["quota-token"]
        case 2:
          return resourceDrops["permission-card"]
        default:
          throw new Error(`unknown resource kind requested by test: ${kind}`)
      }
    },
  },
}

for (const imported of WebAssembly.Module.imports(module)) {
  if (
    imported.kind === "function" &&
    imported.module === "golem:core/types@2.0.0" &&
    imported.name.startsWith("[resource-drop]")
  ) {
    importObject[imported.module] ??= {}
    const kind = imported.name.slice("[resource-drop]".length)
    if (!(kind in resourceDrops)) {
      throw new Error(`unsupported resource-drop test import: ${kind}`)
    }
    importObject[imported.module][imported.name] = () => {
      resourceDrops[kind]++
    }
    continue
  }
  if (importObject[imported.module]) {
    continue
  }
  throw new Error(
    `unsupported WebAssembly test import: ${imported.module}#${imported.name}`,
  )
}

instance = await WebAssembly.instantiate(module, importObject)
instance.exports._start()
