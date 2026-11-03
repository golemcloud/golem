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
let toolPendingGet = false
let toolReplyNode = 0
let toolReplyEmpty = []
let toolNextOutput = 3001
const toolOutputReads = new Set()
let durableStreamHostMode = 0
let nextDurableStreamHandle = 400
const durableStreamResources = new Map()
const durableStreamReplies = []
const durableStreamConstructors = { reader: 0, writer: 0 }
// Fake `golem:permissions` host for SDK unit tests. Strings and lists returned
// to the guest are written into buffers that the test allocates with the
// guest allocator and hands over with `push-permission-host-buffer`, because
// test binaries export no `cabi_realloc`.
const permissionHost = {
  buffers: [],
  nextHandle: 100,
  parents: new Map(),
  scopeRoots: new Map(),
  // Expiry seconds by handle; a card without an entry does not expire.
  expiries: new Map(),
  revokeCalls: 0,
  lastRevoked: -1,
  installCalls: 0,
  lastInstalled: -1,
  lastDeriveParent: -1,
  lastScopeParentCount: -1,
  lastLifespan: null,
  persistentTemplate: false,
  lentScope: false,
}

function resetPermissionHost() {
  permissionHost.buffers = []
  permissionHost.nextHandle = 100
  permissionHost.parents = new Map()
  permissionHost.scopeRoots = new Map()
  permissionHost.expiries = new Map()
  permissionHost.revokeCalls = 0
  permissionHost.lastRevoked = -1
  permissionHost.installCalls = 0
  permissionHost.lastInstalled = -1
  permissionHost.lastDeriveParent = -1
  permissionHost.lastScopeParentCount = -1
  permissionHost.lastLifespan = null
  permissionHost.persistentTemplate = false
  permissionHost.lentScope = false
}

function permissionHostAllocate(size) {
  const buffer = permissionHost.buffers.shift()
  if (!buffer) throw new Error("permission test host ran out of guest buffers")
  if (buffer.size < size) {
    throw new Error(`permission test host buffer too small: ${buffer.size} < ${size}`)
  }
  return buffer.ptr
}

function permissionHostReadString(ptr, len) {
  return String.fromCharCode(
    ...new Uint16Array(instance.exports.memory.buffer, ptr, len),
  )
}

function permissionHostWriteString(memory, at, text) {
  const ptr = permissionHostAllocate(text.length * 2)
  for (let index = 0; index < text.length; index++) {
    memory.setUint16(ptr + index * 2, text.charCodeAt(index), true)
  }
  memory.setInt32(at, ptr, true)
  memory.setInt32(at + 4, text.length, true)
}

function permissionHostWriteList(memory, at, elementSize, items, writeItem) {
  const ptr = permissionHostAllocate(Math.max(items.length * elementSize, 1))
  items.forEach((item, index) => writeItem(ptr + index * elementSize, item))
  memory.setInt32(at, ptr, true)
  memory.setInt32(at + 4, items.length, true)
}

const PERMISSION_ERROR_CASES = [
  "not-permitted",
  "lower-bound-too-broad",
  "upper-bound-too-broad",
  "wallet-multi-source-required",
  "card-revoked",
  "card-expired",
  "install-recipient-mismatch",
  "unknown-resource-class",
  "invalid-pattern",
  "invalid-verb",
  "invalid-owner",
  "invalid-recipient",
]

function permissionHostWriteError(memory, at, kind, message) {
  const tag = PERMISSION_ERROR_CASES.indexOf(kind)
  if (tag < 0) throw new Error(`unknown permission error case: ${kind}`)
  memory.setUint8(at, tag)
  permissionHostWriteString(memory, at + 4, message)
}

function permissionHostWriteCardId(memory, at, handle) {
  memory.setBigUint64(at, BigInt(handle), true)
  memory.setBigUint64(at + 8, 7n, true)
}

const PERMISSION_HOST_DERIVED_EXPIRY = 1700000000n

function permissionHostNewCard(parents, expiresAt) {
  const handle = permissionHost.nextHandle++
  permissionHost.parents.set(handle, parents)
  if (expiresAt !== undefined) permissionHost.expiries.set(handle, expiresAt)
  return handle
}

function permissionHostReadGrantVerb(listPtr, listLen) {
  if (listLen === 0) return null
  const memory = new DataView(instance.exports.memory.buffer)
  return permissionHostReadString(
    memory.getInt32(listPtr + 24, true),
    memory.getInt32(listPtr + 28, true),
  )
}

function permissionHostOkCard(retPtr, handle) {
  const memory = new DataView(instance.exports.memory.buffer)
  memory.setUint8(retPtr, 0)
  memory.setInt32(retPtr + 4, handle, true)
}

function permissionHostOkPersistentCard(retPtr, handle) {
  const memory = new DataView(instance.exports.memory.buffer)
  memory.setUint8(retPtr, 0)
  memory.setUint8(retPtr + 4, permissionHost.persistentTemplate ? 1 : 0)
  memory.setInt32(retPtr + 8, handle, true)
}

function permissionHostErr(retPtr, kind, message) {
  const memory = new DataView(instance.exports.memory.buffer)
  memory.setUint8(retPtr, 1)
  permissionHostWriteError(memory, retPtr + 4, kind, message)
}

const permissionHostImports = {
  "golem:permissions/types@0.1.0": {
    id(handle, retPtr) {
      permissionHostWriteCardId(
        new DataView(instance.exports.memory.buffer),
        retPtr,
        handle,
      )
    },
    parents(handle, retPtr) {
      const memory = new DataView(instance.exports.memory.buffer)
      permissionHostWriteList(
        memory,
        retPtr,
        16,
        permissionHost.parents.get(handle) ?? [],
        (at, parent) => permissionHostWriteCardId(memory, at, parent),
      )
    },
    "expires-at"(handle, retPtr) {
      const memory = new DataView(instance.exports.memory.buffer)
      const expiresAt = permissionHost.expiries.get(handle)
      if (expiresAt !== undefined) {
        memory.setUint8(retPtr, 1)
        memory.setBigInt64(retPtr + 8, expiresAt, true)
        memory.setUint32(retPtr + 16, 5, true)
      } else {
        memory.setUint8(retPtr, 0)
      }
    },
  },
  "golem:permissions/inspect@0.1.0": {
    "inspect-card"(handle, retPtr) {
      const memory = new DataView(instance.exports.memory.buffer)
      if (handle !== 11) {
        memory.setUint8(retPtr, 1)
        permissionHostWriteError(memory, retPtr + 4, "not-permitted", "card:inspect is not permitted")
        return
      }
      memory.setUint8(retPtr, 0)
      const grant = ["card", "alice/*", "*", "inspect", "*"]
      permissionHostWriteList(memory, retPtr + 4, 40, [grant], (at, fields) =>
        fields.forEach((field, index) =>
          permissionHostWriteString(memory, at + index * 8, field),
        ),
      )
      for (const offset of [12, 20, 28]) {
        permissionHostWriteList(memory, retPtr + offset, 40, [], () => {})
      }
    },
  },
  "golem:permissions/derive@0.1.0": {
    derive(parent, lp, lpn, ln, lnn, up, upn, un, unn, hasLifespan, lifespanPtr, lifespanLen, retPtr) {
      permissionHost.lastDeriveParent = parent
      permissionHost.lastLifespan = hasLifespan
        ? permissionHostReadString(lifespanPtr, lifespanLen)
        : null
      permissionHostOkPersistentCard(retPtr, permissionHostNewCard(
          [parent],
          hasLifespan !== 0 ? PERMISSION_HOST_DERIVED_EXPIRY : undefined,
        ))
    },
    "derive-from-wallet"(lp, lpn, ln, lnn, up, upn, un, unn, hasLifespan, lifespanPtr, lifespanLen, retPtr) {
      permissionHost.lastLifespan = hasLifespan
        ? permissionHostReadString(lifespanPtr, lifespanLen)
        : null
      if (permissionHostReadGrantVerb(lp, lpn) === "forbidden") {
        permissionHostErr(retPtr, "not-permitted", "card:derive is not permitted")
        return
      }
      permissionHostOkPersistentCard(retPtr, permissionHostNewCard(
          [11],
          hasLifespan !== 0 ? PERMISSION_HOST_DERIVED_EXPIRY : undefined,
        ))
    },
    "derive-scope"(parentsPtr, parentsLen, lp, lpn, ln, lnn, up, upn, un, unn, retPtr) {
      const memory = new DataView(instance.exports.memory.buffer)
      const parents = []
      for (let index = 0; index < parentsLen; index++) {
        parents.push(memory.getInt32(parentsPtr + index * 4, true))
      }
      permissionHost.lastScopeParentCount = parents.length
      // A scope card names its root card ids, sorted and without duplicates
      // like the real host: a scope-card parent contributes its own roots.
      const roots = [
        ...new Set(parents.flatMap(parent => permissionHost.scopeRoots.get(parent) ?? [parent])),
      ].sort((left, right) => left - right)
      // Like the real host, a scope card expires with its earliest root.
      const rootExpiries = roots
        .map(root => permissionHost.expiries.get(root))
        .filter(expiresAt => expiresAt !== undefined)
      const handle = permissionHostNewCard(
        roots,
        rootExpiries.length === 0
          ? undefined
          : rootExpiries.reduce((left, right) => (left < right ? left : right)),
      )
      permissionHost.scopeRoots.set(handle, roots)
      permissionHostOkCard(retPtr, handle)
    },
  },
  "golem:permissions/revoke@0.1.0": {
    "revoke-card"(handle, retPtr) {
      permissionHost.revokeCalls++
      permissionHost.lastRevoked = handle
      const memory = new DataView(instance.exports.memory.buffer)
      if (handle === 11) {
        memory.setUint8(retPtr, 0)
        memory.setUint32(retPtr + 4, 3, true)
      } else {
        permissionHostErr(retPtr, "card-revoked", "permission card is unknown or revoked")
      }
    },
  },
  "golem:permissions/wallet@0.1.0": {
    "self-wallet"(retPtr) {
      const memory = new DataView(instance.exports.memory.buffer)
      // `wallet-card` cases: 0 concrete, 1 template, 2 scope.
      const entries = [[0, 11], [1, 12]]
      if (permissionHost.lentScope) entries.push([2, 13])
      permissionHostWriteList(memory, retPtr, 8, entries, (at, [tag, handle]) => {
        memory.setUint8(at, tag)
        memory.setInt32(at + 4, handle, true)
      })
    },
    "self-version"(retPtr) {
      const memory = new DataView(instance.exports.memory.buffer)
      permissionHostWriteList(memory, retPtr, 1, [...Array(32).keys()], (at, byte) =>
        memory.setUint8(at, byte),
      )
      memory.setBigUint64(retPtr + 8, 9n, true)
    },
    "install-card"(handle, holderTag, high, low, agentPtr, agentLen, retPtr) {
      permissionHost.installCalls++
      permissionHost.lastInstalled = handle
      permissionHostErr(retPtr, "install-recipient-mismatch", "card does not cover the recipient")
    },
  },
  "golem:permissions/kernel-introspection@0.1.0": {
    "list-modules"(retPtr) {
      const memory = new DataView(instance.exports.memory.buffer)
      permissionHostWriteList(memory, retPtr, 16, [["card", ["derive", "inspect"]]], (at, [name, verbs]) => {
        permissionHostWriteString(memory, at, name)
        permissionHostWriteList(memory, at + 8, 8, verbs, (verbAt, verb) =>
          permissionHostWriteString(memory, verbAt, verb),
        )
      })
    },
    "validate-grant"(cp, cl, op, ol, rp, rl, verbPtr, verbLen, idp, idl, retPtr) {
      const memory = new DataView(instance.exports.memory.buffer)
      const verb = permissionHostReadString(verbPtr, verbLen)
      if (verb === "read") {
        memory.setUint8(retPtr, 0)
      } else {
        memory.setUint8(retPtr, 1)
        permissionHostWriteError(memory, retPtr + 4, "invalid-verb", verb)
      }
    },
  },
}

const resourceDrops = {
  secret: 0,
  "quota-token": 0,
  "permission-card": 0,
  "schema-value-stream": 0,
  "future-invoke-result": 0,
  "future-invoke-result-cancel": 0,
  "tool-output-readable": 0,
  "tool-output-cancel-read": 0,
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
          return () => (802n << 32n) | 801n
        case "[async-lower][stream-read-unit]":
          return () => -1
        case "[async-lower][stream-write-unit]":
          return () => 16
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
  "wasi:cli/environment@0.3.0": {
    "get-environment"(resultPtr) {
      const memory = new DataView(instance.exports.memory.buffer)
      memory.setUint32(resultPtr, 0, true)
      memory.setUint32(resultPtr + 4, 0, true)
    },
  },
  "wasi:random/random@0.3.0": {
    "get-random-u64"() {
      const [value] = crypto.getRandomValues(new BigUint64Array(1))
      return BigInt.asIntN(64, value)
    },
  },
  "golem:agent/host@2.0.0": {
    "parse-agent-id"() {
      throw new Error("parse-agent-id is not supplied by the SDK unit-test host")
    },
  },
  "wasi:logging/logging": {
    log() {
      throw new Error("unexpected guest log in SDK unit-test host")
    },
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
        case 5:
          return resourceDrops["future-invoke-result-cancel"]
        case 6:
          return resourceDrops["tool-output-readable"]
        case 7:
          return resourceDrops["tool-output-cancel-read"]
        default:
          throw new Error(`unknown resource kind requested by test: ${kind}`)
      }
    },
    "reset-permission-host"() {
      resetPermissionHost()
    },
    "set-persistent-template"(enabled) {
      permissionHost.persistentTemplate = enabled !== 0
    },
    "lend-permission-scope-card"() {
      // The lent scope card 13 has the root 12, which expires.
      permissionHost.lentScope = true
      permissionHost.parents.set(13, [12])
      permissionHost.scopeRoots.set(13, [12])
      permissionHost.expiries.set(12, PERMISSION_HOST_DERIVED_EXPIRY)
      permissionHost.expiries.set(13, PERMISSION_HOST_DERIVED_EXPIRY)
    },
    "set-permission-card-expiry"(handle, seconds) {
      permissionHost.expiries.set(handle, BigInt(seconds))
    },
    "push-permission-host-buffer"(ptr, size) {
      permissionHost.buffers.push({ ptr, size })
    },
    "permission-host-stat"(field) {
      switch (field) {
        case 0:
          return permissionHost.revokeCalls
        case 1:
          return permissionHost.lastRevoked
        case 2:
          return permissionHost.installCalls
        case 3:
          return permissionHost.lastInstalled
        case 4:
          return permissionHost.lastDeriveParent
        case 5:
          return permissionHost.lastScopeParentCount
        case 6:
          return permissionHost.lastLifespan === null
            ? -1
            : permissionHost.lastLifespan.length
        default:
          throw new Error(`unknown permission host statistic: ${field}`)
      }
    },
    "set-schema-value-stream-host-mode"(mode) {
      schemaValueStreamHostMode = mode
    },
    "set-tool-host-mode"(mode) {
      toolHostMode = mode
      toolNextOutput = 3001
      toolOutputReads.clear()
    },
    "tool-result-node"(pointer, types, defs, name) {
      toolReplyNode = pointer
      toolReplyEmpty = [types, defs, name]
    },
    "settle-tool-result"() {
      if (!toolPendingGet) throw new Error("no active tool result get")
      toolPendingGet = false
      return 901
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
        (imported.name === "parse-uuid" ||
          imported.name === "uuid-to-string")) ||
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
        case "[constructor]tool-rpc":
          return 1001
        case "[static]tool-rpc.create": {
          const resultPtr = args[2]
          memory.setUint8(resultPtr, 0)
          memory.setInt32(resultPtr + 4, 1001, true)
          return
        }
        case "[resource-drop]tool-rpc":
          return
        case "create-output": {
          const resultPtr = args[0]
          memory.setInt32(resultPtr, toolNextOutput++, true)
          memory.setInt32(resultPtr + 4, toolNextOutput++, true)
          return
        }
        case "[async-lower][stream-read-0]create-output": {
          if (toolHostMode === 8 && !toolOutputReads.has(args[0])) {
            toolOutputReads.add(args[0])
            const pointer = args[1]
            // One canonical result<list<u8>, failure> item, backed by owned bytes.
            memory.setUint8(pointer, 0)
            memory.setInt32(pointer + 4, toolReplyNode, true)
            memory.setInt32(pointer + 8, 2, true)
            return 16
          }
          return toolHostMode === 3 ? -1 : 1
        }
        case "[stream-cancel-read-0]create-output":
          resourceDrops["tool-output-cancel-read"]++
          return 2
        case "[stream-drop-readable-0]create-output":
          resourceDrops["tool-output-readable"]++
          return
        case "[method]tool-rpc.async-invoke-and-await":
          return 2001
        case "[method]future-invoke-result.cancel":
          resourceDrops["future-invoke-result-cancel"]++
          return
        case "[async-lower][method]future-invoke-result.get": {
          const resultPtr = args[1]
          new Uint8Array(instance.exports.memory.buffer, resultPtr, 56).fill(0)
          if (toolHostMode >= 4 && toolHostMode <= 7) {
            const custom = toolHostMode === 5 || toolHostMode === 7
            const payload = custom ? resultPtr + 20 : resultPtr + 8
            if (custom) {
              memory.setUint8(resultPtr, 1)
              memory.setUint8(resultPtr + 4, 4)
              memory.setUint8(resultPtr + 8, 5)
              memory.setInt32(resultPtr + 12, toolReplyEmpty[2], true)
            } else {
              memory.setUint8(resultPtr + 4, 1)
            }
            memory.setInt32(payload, toolReplyEmpty[0], true)
            memory.setInt32(payload + 8, toolReplyEmpty[1], true)
            memory.setInt32(payload + 20, toolReplyNode, true)
            memory.setInt32(payload + 24, 1, true)
            if (toolHostMode <= 5) {
              toolPendingGet = true
              return (901 << 4) | 1
            }
            return 2
          }
          if (toolHostMode === 1) {
            memory.setUint8(resultPtr, 1)
            memory.setUint8(resultPtr + 4, 5)
          } else {
            memory.setUint8(resultPtr, 0)
            memory.setUint8(resultPtr + 4, 0)
            memory.setUint8(resultPtr + 40, 0)
            memory.setUint8(resultPtr + 48, 0)
          }
          return 2
        }
        case "[resource-drop]future-invoke-result":
          if (toolPendingGet) throw new Error("dropped tool future with an active get borrow")
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
  if (
    imported.kind === "function" &&
    permissionHostImports[imported.module]?.[imported.name] !== undefined
  ) {
    importObject[imported.module] ??= {}
    importObject[imported.module][imported.name] =
      permissionHostImports[imported.module][imported.name]
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
