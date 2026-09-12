import { describe, expect, it, vi } from "vitest"
import { Effect } from "effect"
import type { SchemaValueTree } from "golem:core/types@2.0.0"
import { AgentStream, agentStreamToHandle } from "../src/internal/agentStream.js"
import { toWitCodec } from "../src/WitCodec.js"
import { Uint32 } from "../src/WitTypes.js"
import { withCapabilityAdoptionTransaction } from "../src/internal/schema-model/capabilityTransaction.js"
import { PreparedStream } from "../src/internal/schema-model/preparedStream.js"
import { GuestSchemaValueStreamHandle } from "../src/internal/schema-model/schemaValueStreamHandle.js"
import { STREAM_INTERNAL } from "../src/internal/schema-model/streamInternal.js"
import { schemaValueToWitAsync } from "../src/internal/schema-model/wit.js"
import { v } from "../src/internal/schema-model/model.js"

const transport = vi.hoisted(() => ({ wrap: vi.fn() }))
vi.mock("golem:core/types@2.0.0", () => ({ SchemaValueStream: transport }))

const numbers = Effect.runSync(toWitCodec(Uint32)).codec

describe("stream transfer preparation", () => {
  it("aborts a pending transport pull without opening or closing the source", async () => {
    const open = vi.fn(() => ({ next: vi.fn(), return: vi.fn() }))
    const proxy = new PreparedStream({ [Symbol.asyncIterator]: open })
    const pending = proxy.next()
    proxy.abort()
    await expect(pending).resolves.toEqual({ done: true, value: undefined })
    await proxy.return()
    expect(open).not.toHaveBeenCalled()
  })

  it("forwards after commit and closes the source only once", async () => {
    const close = vi.fn(async () => ({ done: true as const, value: undefined }))
    const next = vi.fn(async () => ({ done: false as const, value: 29 }))
    const open = vi.fn(() => ({ next, return: close }))
    const proxy = new PreparedStream({ [Symbol.asyncIterator]: open })
    const pending = proxy.next()
    expect(open).not.toHaveBeenCalled()
    proxy.commit()
    await expect(pending).resolves.toEqual({ done: false, value: 29 })
    await proxy.return()
    await proxy.return()
    expect(open).toHaveBeenCalledTimes(1)
    expect(close).toHaveBeenCalledTimes(1)
  })

  it("restores both native handles when a sibling wrap fails without pulling either source", async () => {
    const open = vi.fn(() => ({
      next: vi.fn(async () => ({ done: true as const, value: undefined })),
    }))
    const source: AsyncIterable<SchemaValueTree> = { [Symbol.asyncIterator]: open }
    const first = new GuestSchemaValueStreamHandle(STREAM_INTERNAL, {
      kind: "native",
      value: source,
    })
    const second = new GuestSchemaValueStreamHandle(STREAM_INTERNAL, {
      kind: "native",
      value: { [Symbol.asyncIterator]: open },
    })
    const close = vi.fn()
    const pulls: Array<Promise<IteratorResult<SchemaValueTree>>> = []
    transport.wrap.mockReset()
    transport.wrap.mockImplementationOnce(async (proxy: AsyncIterable<SchemaValueTree>) => {
      pulls.push(proxy[Symbol.asyncIterator]().next())
      return { [Symbol.dispose]: close }
    })
    transport.wrap.mockImplementationOnce(async (proxy: AsyncIterable<SchemaValueTree>) => {
      pulls.push(proxy[Symbol.asyncIterator]().next())
      throw new Error("second wrap failed")
    })
    const tree = v.tuple([v.stream(first), v.stream(second)])
    await expect(schemaValueToWitAsync(tree)).rejects.toThrow("second wrap failed")
    expect(await Promise.all(pulls)).toEqual([
      { done: true, value: undefined },
      { done: true, value: undefined },
    ])
    expect(open).not.toHaveBeenCalled()
    expect(close).toHaveBeenCalledTimes(1)
    expect(first.peek()?.kind).toBe("native")
    expect(second.peek()?.kind).toBe("native")
    transport.wrap.mockImplementation(async () => ({ [Symbol.dispose]: vi.fn() }))
    const wire = await schemaValueToWitAsync(tree)
    expect(wire.valueNodes.map((node) => node.tag)).toEqual([
      "stream-value",
      "stream-value",
      "tuple-value",
    ])
    expect(wire.valueNodes[wire.root]).toEqual({ tag: "tuple-value", val: [0, 1] })
    expect(first.peek()).toBeUndefined()
    expect(second.peek()).toBeUndefined()
  })

  it("isolates interruption and late wrap completion from a retry", async () => {
    const open = vi.fn(() => ({ next: vi.fn() }))
    const handle = new GuestSchemaValueStreamHandle(STREAM_INTERNAL, {
      kind: "native",
      value: { [Symbol.asyncIterator]: open },
    })
    let finish!: (value: object) => void
    let started!: () => void
    const entered = new Promise<void>((resolve) => {
      started = resolve
    })
    const pending = new Promise<object>((resolve) => {
      finish = resolve
    })
    transport.wrap.mockReset()
    transport.wrap.mockImplementationOnce(() => {
      started()
      return pending
    })
    const controller = new AbortController()
    const first = schemaValueToWitAsync(v.stream(handle), controller.signal)
    await entered
    await expect(schemaValueToWitAsync(v.stream(handle))).rejects.toThrow(/reserved/)
    controller.abort()
    const lateDrop = vi.fn()
    transport.wrap.mockImplementation(async () => ({ [Symbol.dispose]: vi.fn() }))
    await schemaValueToWitAsync(v.stream(handle))
    finish({ [Symbol.dispose]: lateDrop })
    await expect(first).rejects.toThrow(/interrupted/)
    expect(lateDrop).toHaveBeenCalledTimes(1)
    expect(handle.peek()).toBeUndefined()
    expect(open).not.toHaveBeenCalled()
  })

  // PROVISIONAL bug_finder reproducer — remove if the finding is rejected.
  it("restores an AgentStream when asynchronous wrapping fails", async () => {
    const stream = AgentStream.from([23])
    const value = withCapabilityAdoptionTransaction(() =>
      v.stream(agentStreamToHandle(stream, numbers)),
    )
    transport.wrap.mockReset()
    transport.wrap.mockRejectedValueOnce(new Error("wrap failed"))

    await expect(schemaValueToWitAsync(value)).rejects.toThrow("wrap failed")
    await expect(stream.next()).resolves.toEqual({ done: false, value: 23 })
  })
})
