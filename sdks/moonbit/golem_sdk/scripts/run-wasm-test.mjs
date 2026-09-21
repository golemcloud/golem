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
let toolHostMode = 0
let durableStreamHostMode = 0
let nextDurableStreamHandle = 400
const durableStreamResources = new Map()
const durableStreamReplies = []
const durableStreamConstructors = { reader: 0, writer: 0 }
const resourceDrops = {
  secret: 0,
  "quota-token": 0,
  "permission-card": 0,
  "schema-value-stream": 0,
  "future-invoke-result": 0,
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
        case 4:
          return resourceDrops["future-invoke-result"]
        default:
          throw new Error(`unknown resource kind requested by test: ${kind}`)
      }
    },
    "set-schema-value-stream-host-mode"(mode) {
      schemaValueStreamHostMode = mode
    },
    "set-tool-host-mode"(mode) {
      toolHostMode = mode
    },
    "set-durable-stream-host-mode"(mode) {
      durableStreamHostMode = mode
    },
    "durable-stream-reply-string"(pointer, length) {
      durableStreamReplies.push([pointer, length])
    },
    "durable-stream-resource-stat"(handle, field) {
      if (field === 0) return durableStreamConstructors.reader
      if (field === 1) return durableStreamConstructors.writer
      const resource = durableStreamResources.get(handle)
      if (!resource) throw new Error("unknown DS resource")
      if (field === 2) return resource.calls
      if (field === 3) return resource.dropped ? 1 : 0
      throw new Error("unknown DS resource statistic")
    },
  },
}

for (const imported of WebAssembly.Module.imports(module)) {
  if (imported.kind === "function" && imported.module === "golem:agent/durable-streams@2.0.0") {
    importObject[imported.module] ??= {}
    importObject[imported.module][imported.name] = (...args) => {
      const memory = new DataView(instance.exports.memory.buffer)
      const string = (pointer, length) => String.fromCharCode(
        ...new Uint16Array(instance.exports.memory.buffer, pointer, length),
      )
      const writer = imported.name.includes("durable-stream-writer")
      const kind = writer ? "writer" : "reader"
      if (imported.name === `[constructor]durable-stream-${kind}`) {
        const descriptor = writer ? {
          url: string(args[0], args[1]), contentType: string(args[2], args[3]),
          producerId: string(args[4], args[5]), epoch: args[6], timeout: args[7],
          auth: args[8] ? args[9] : null,
        } : {
          url: string(args[0], args[1]), mode: args[2], timeout: args[3],
          auth: args[4] ? args[5] : null,
        }
        if (durableStreamHostMode !== 0 &&
            (descriptor.url !== "https://example.test/stream" ||
             descriptor.auth !== 77 || descriptor.timeout !== 12345n ||
             (writer && (descriptor.contentType !== "application/json" || descriptor.producerId !== "producer")))) {
          throw new Error("incorrect immutable DS descriptor or borrowed secret lowering")
        }
        const handle = nextDurableStreamHandle++
        durableStreamConstructors[kind]++
        durableStreamResources.set(handle, {
          kind, descriptor: Object.freeze(descriptor), calls: 0, dropped: false,
          requests: new Map(),
        })
        return handle
      }
      const dropping = imported.name === `[resource-drop]durable-stream-${kind}`
      const handle = dropping ? args[0] : memory.getInt32(args[0], true)
      const resource = durableStreamResources.get(handle)
      if (!resource || resource.kind !== kind || resource.dropped) {
        throw new Error("invalid or dropped DS resource handle")
      }
      if (dropping) {
        resource.dropped = true
        return
      }
      const method = writer ? "append" : "read"
      if (imported.name !== `[async-lower][method]durable-stream-${kind}.${method}` || durableStreamHostMode === 0) {
        throw new Error(`unexpected live import in SDK state test: ${imported.name}`)
      }
      const [request, result] = args
      resource.calls++
      const sequence = writer ? memory.getBigUint64(request + 24, true) : null
      if (writer) {
        const tag = memory.getUint8(request + 8)
        const pointer = memory.getInt32(request + 12, true)
        const length = memory.getInt32(request + 16, true)
        const payload = tag === 0 ? Array.from({ length }, (_, index) => {
          const base = pointer + index * 8
          return string(memory.getInt32(base, true), memory.getInt32(base + 4, true))
        }) : Array.from(new Uint8Array(instance.exports.memory.buffer, pointer, length))
        const body = JSON.stringify([tag, payload, memory.getUint8(request + 32)])
        if (resource.requests.has(sequence) && resource.requests.get(sequence) !== body) {
          throw new Error("uncertain append changed body or close flag on the same resource")
        }
        resource.requests.set(sequence, body)
      }
      const replyString = offset => {
        const reply = durableStreamReplies.shift()
        if (!reply) throw new Error("fixture reply string not supplied")
        memory.setInt32(offset, reply[0], true)
        memory.setInt32(offset + 4, reply[1], true)
      }
      new Uint8Array(instance.exports.memory.buffer, result, 72).fill(0)
      if (!writer && durableStreamHostMode === 4) {
        const offset = string(memory.getInt32(request + 4, true), memory.getInt32(request + 8, true))
        const cursor = memory.getUint8(request + 12) ? string(memory.getInt32(request + 16, true), memory.getInt32(request + 20, true)) : null
        const contentType = memory.getUint8(request + 28) ? string(memory.getInt32(request + 32, true), memory.getInt32(request + 36, true)) : null
        const first = resource.calls === 1
        if (offset !== (first ? "now" : "opaque:next") ||
            cursor !== (first ? null : "independent:cursor") ||
            contentType !== (first ? null : "application/json") ||
            memory.getUint8(request + 24) !== (first ? 0 : 2)) {
          throw new Error("incorrect compact read checkpoint, transport or pinned content type")
        }
        replyString(result + 8) // owned byte allocation
        replyString(result + 16)
        replyString(result + 24)
        memory.setUint8(result + 32, 1)
        replyString(result + 36)
        memory.setUint8(result + 44, 1)
        memory.setUint8(result + 45, first ? 0 : 1)
      } else if (writer && (durableStreamHostMode === 2 || durableStreamHostMode === 3)) {
        if (durableStreamHostMode === 3) {
          memory.setUint8(result + 8, 1)
          replyString(result + 12)
        }
        memory.setBigUint64(result + 24, resource.descriptor.epoch, true)
        memory.setBigUint64(result + 32, sequence, true)
        memory.setUint8(result + 40, memory.getUint8(request + 32))
      } else {
        memory.setUint8(result, 1)
        memory.setUint8(result + 8, 13) // unavailable
        replyString(result + 12)
        memory.setUint8(result + 24, 1)
        memory.setBigUint64(result + 32, 987n, true)
        memory.setUint8(result + 40, 1)
        memory.setBigUint64(result + 48, 9007199254740991n, true)
        memory.setUint8(result + 56, 1)
        memory.setBigUint64(result + 64, 9007199254740990n, true)
      }
      return 2
    }
    continue
  }
  if (
    imported.kind === "function" &&
    ((imported.module === "wasi:clocks/monotonic-clock@0.3.0" &&
        imported.name === "[async-lower]wait-for") ||
      (imported.module === "golem:core/types@2.0.0" &&
        imported.name === "uuid-to-string") ||
      (imported.module === "golem:api/host@1.5.0" &&
        imported.name === "generate-idempotency-key"))
  ) {
    importObject[imported.module] ??= {}
    importObject[imported.module][imported.name] = request => {
      if (imported.name === "[async-lower]wait-for" && durableStreamHostMode === 1) {
        if (request !== 1000000n) throw new Error("incorrect timer duration lowering")
        return 2
      }
      throw new Error(`unexpected live import in SDK state test: ${imported.name}`)
    }
    continue
  }
  if (
    imported.kind === "function" &&
    imported.module === "golem:tool/streams@0.1.0"
  ) {
    importObject[imported.module] ??= {}
    importObject[imported.module][imported.name] = () => 0
    continue
  }
  if (
    imported.kind === "function" &&
    imported.module === "golem:tool/host@0.1.0"
  ) {
    importObject[imported.module] ??= {}
    importObject[imported.module][imported.name] = (...args) => {
      if (toolHostMode === 0) {
        throw new Error(
          `cancelled tool input unexpectedly reached the host: ${imported.name}`,
        )
      }
      const memory = new DataView(instance.exports.memory.buffer)
      switch (imported.name) {
        case "[static]tool-rpc.create": {
          const resultPtr = args[2]
          memory.setUint8(resultPtr, 0)
          memory.setInt32(resultPtr + 4, 1001, true)
          return
        }
        case "[resource-drop]tool-rpc":
          return
        case "[method]tool-rpc.async-invoke-and-await":
          return 2001
        case "[async-lower][method]future-invoke-result.get": {
          const resultPtr = args[1]
          if (toolHostMode === 1) {
            memory.setUint8(resultPtr, 1)
            memory.setUint8(resultPtr + 4, 5)
          } else {
            memory.setUint8(resultPtr, 0)
            memory.setUint8(resultPtr + 4, 0)
            memory.setUint8(resultPtr + 40, 0)
          }
          return 2
        }
        case "[resource-drop]future-invoke-result":
          resourceDrops["future-invoke-result"]++
          return
        default:
          throw new Error(`unsupported tool test import: ${imported.name}`)
      }
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
