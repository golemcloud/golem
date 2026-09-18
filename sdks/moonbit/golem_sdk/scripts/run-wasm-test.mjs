#!/usr/bin/env node

import fs from "node:fs"
import path from "node:path"

const args = process.argv.slice(2)
const wasmPath = args.find(arg => arg.endsWith(".wasm"))
if (!wasmPath) {
  console.error(
    "usage: node scripts/run-wasm-test.mjs [--test-args <json>] <test.wasm>",
  )
  process.exit(2)
}

const testArgsIndex = args.indexOf("--test-args")
const testOptions =
  testArgsIndex >= 0 ? JSON.parse(args[testArgsIndex + 1]) : null
const requestedTests = testOptions?.file_and_index ?? null

const bytes = fs.readFileSync(wasmPath)
const module = new WebAssembly.Module(bytes)
const exceptionTag = new WebAssembly.Tag({ parameters: [] })
let instance
let componentContext = 0
let nextWaitableSet = 1
let schemaValueStreamHostMode = 0
let durableStreamHostMode = 0
const resourceDrops = {
  secret: 0,
  "quota-token": 0,
  "permission-card": 0,
  "schema-value-stream": 0,
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
        case "[subtask-cancel]":
          return () =>
            schemaValueStreamHostMode === 3 || schemaValueStreamHostMode === 4
              ? 3
              : 0
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
      return { value, offset: 0 }
    },
    string_read_char(handle) {
      if (handle.offset >= handle.value.length) {
        return -1
      }
      const codePoint = handle.value.codePointAt(handle.offset)
      handle.offset += codePoint > 0xffff ? 2 : 1
      return codePoint
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
        case 3:
          return resourceDrops["schema-value-stream"]
        default:
          throw new Error(`unknown resource kind requested by test: ${kind}`)
      }
    },
    "set-schema-value-stream-host-mode"(mode) {
      schemaValueStreamHostMode = mode
    },
    "set-durable-stream-host-mode"(mode) {
      durableStreamHostMode = mode
    },
  },
}

for (const imported of WebAssembly.Module.imports(module)) {
  if (
    imported.kind === "function" &&
    ((imported.module === "golem:agent/host@2.0.0" &&
      ["[async-lower]read-durable-stream-batch", "[async-lower]append-durable-stream-batch"].includes(imported.name)) ||
      (imported.module === "wasi:clocks/monotonic-clock@0.3.0" &&
        imported.name === "[async-lower]wait-for") ||
      (imported.module === "golem:core/types@2.0.0" &&
        imported.name === "uuid-to-string") ||
      (imported.module === "golem:api/host@1.5.0" &&
        imported.name === "generate-idempotency-key"))
  ) {
    importObject[imported.module] ??= {}
    importObject[imported.module][imported.name] = (request, result) => {
      const receiptFixture =
        imported.name === "[async-lower]append-durable-stream-batch" &&
        (durableStreamHostMode === 2 || durableStreamHostMode === 3)
      if (durableStreamHostMode !== 1 && !receiptFixture) {
        throw new Error(`unexpected live import in SDK state test: ${imported.name}`)
      }
      if (imported.name === "[async-lower]wait-for") {
        if (request !== 1000000n) throw new Error("incorrect timer duration lowering")
        return 2
      }
      if (imported.name === "generate-idempotency-key" || imported.name === "uuid-to-string") {
        throw new Error("binding fixture requires an explicit producer ID")
      }
      const memory = new DataView(instance.exports.memory.buffer)
      const append = imported.name === "[async-lower]append-durable-stream-batch"
      const authOffset = append ? 72 : 56
      if (memory.getUint8(request + authOffset) !== 1 ||
          memory.getInt32(request + authOffset + 4, true) !== 77) {
        throw new Error("secret capability was not borrowed unchanged")
      }
      const timeoutOffset = append ? 64 : 48
      if (memory.getBigUint64(request + timeoutOffset, true) !== 12345n) {
        throw new Error("incorrect DS timeout lowering")
      }
      new Uint8Array(instance.exports.memory.buffer, result, 72).fill(0)
      if (receiptFixture) {
        if (durableStreamHostMode === 3) {
          memory.setUint8(result + 8, 1)
          // Transfer the URL allocation as an opaque offset, just as for errors.
          memory.setInt32(result + 12, memory.getInt32(request, true), true)
          memory.setInt32(result + 16, memory.getInt32(request + 4, true), true)
        }
        memory.setBigUint64(result + 24, memory.getBigUint64(request + 40, true), true)
        memory.setBigUint64(result + 32, memory.getBigUint64(request + 48, true), true)
        memory.setUint8(result + 40, memory.getUint8(request + 56))
        return 2
      }
      // Typed error fixture, including every optional integer field.
      memory.setUint8(result, 1)
      memory.setUint8(result + 8, 13) // unavailable
      // Transfer the lowered URL string allocation back as the fixture message.
      memory.setInt32(result + 12, memory.getInt32(request, true), true)
      memory.setInt32(result + 16, memory.getInt32(request + 4, true), true)
      memory.setUint8(result + 24, 1)
      memory.setBigUint64(result + 32, 987n, true)
      memory.setUint8(result + 40, 1)
      memory.setBigUint64(result + 48, 9007199254740991n, true)
      memory.setUint8(result + 56, 1)
      memory.setBigUint64(result + 64, 9007199254740990n, true)
      return 2
    }
    continue
  }
  if (
    imported.kind === "function" &&
    imported.module === "golem:tool/host@0.1.0"
  ) {
    importObject[imported.module] ??= {}
    importObject[imported.module][imported.name] = () => {
      throw new Error(
        `cancelled tool input unexpectedly reached the host: ${imported.name}`,
      )
    }
    continue
  }
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
  if (
    imported.kind === "function" &&
    imported.module === "golem:core/types@2.0.0" &&
    imported.name.includes("schema-value-stream")
  ) {
    importObject[imported.module] ??= {}
    switch (imported.name) {
      case "[stream-new-0][static]schema-value-stream.wrap":
        importObject[imported.module][imported.name] = () =>
          (12n << 32n) | 11n
        break
      case "[async-lower][static]schema-value-stream.wrap":
        importObject[imported.module][imported.name] = (_reader, resultPtr) => {
          if (schemaValueStreamHostMode === 3) {
            return (1 << 4) | 0
          }
          new DataView(instance.exports.memory.buffer).setInt32(
            resultPtr,
            901,
            true,
          )
          return 2
        }
        break
      case "[async-lower][static]schema-value-stream.unwrap":
        importObject[imported.module][imported.name] = (_stream, resultPtr) => {
          if (schemaValueStreamHostMode === 4) {
            return (1 << 4) | 0
          }
          if (schemaValueStreamHostMode === 1) {
            return 4
          }
          new DataView(instance.exports.memory.buffer).setInt32(
            resultPtr,
            701,
            true,
          )
          return 2
        }
        break
      case "[async-lower][stream-read-0][static]schema-value-stream.unwrap":
        importObject[imported.module][imported.name] = () =>
          schemaValueStreamHostMode === 2 ? 2 : 1
        break
      default:
        importObject[imported.module][imported.name] = () => 0
        break
    }
    continue
  }
  if (importObject[imported.module]?.[imported.name] !== undefined) {
    continue
  }
  throw new Error(
    `unsupported WebAssembly test import: ${imported.module}#${imported.name}`,
  )
}

instance = await WebAssembly.instantiate(module, importObject)

function executeRanges(fileAndIndex) {
  for (const [filename, ranges] of fileAndIndex) {
    for (const { start, end } of ranges) {
      for (let index = start; index < end; index++) {
        console.log("----- BEGIN MOON TEST RESULT -----")
        console.log(JSON.stringify({ type: "start", file: filename, index }))
        console.log("----- END MOON TEST RESULT -----")
        try {
          instance.exports.moonbit_test_driver_internal_execute(filename, index)
        } catch (error) {
          const message = error?.stack?.toString() ?? String(error)
          console.log("----- BEGIN MOON TEST RESULT -----")
          console.log(
            JSON.stringify({
              type: "result",
              file: filename,
              index,
              message,
            }),
          )
          console.log("----- END MOON TEST RESULT -----")
        }
      }
    }
  }
}

if (requestedTests) {
  executeRanges(requestedTests)
} else {
  const match = path.basename(wasmPath).match(/\.(\w+)_test\.wasm$/)
  if (!match) {
    throw new Error(`cannot infer test metadata for ${wasmPath}`)
  }
  const metadataPath = path.join(
    path.dirname(wasmPath),
    `__${match[1]}_test_info.json`,
  )
  const metadata = JSON.parse(fs.readFileSync(metadataPath, "utf8"))
  executeRanges(
    Object.entries(metadata.tests).map(([filename, tests]) => [
      filename,
      tests.map(({ index }) => ({ start: index, end: index + 1 })),
    ]),
  )
}

instance.exports.moonbit_test_driver_finish()
