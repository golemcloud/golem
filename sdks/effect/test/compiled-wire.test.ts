import { describe, expect, it } from "@effect/vitest"
import { Context, Effect, Schema, Stream } from "effect"
import { vi } from "vitest"
import type { SchemaGraph } from "golem:core/types@2.0.0"
import {
  createGuestPermissionCardHandle,
  peekGuestPermissionCardHandle,
} from "../src/internal/schema-model/permissionCardHandle.js"
import { PERMISSION_CARD_INTERNAL } from "../src/internal/schema-model/permissionCardInternal.js"
import { SchemaValueStream } from "./mocks/golem-core-types.js"
import {
  compiledWire,
  writeConcrete,
  writeConcreteAsync,
  type ConcreteCodec,
  type WireReader,
  type WireWriter,
} from "../src/internal/compiledWire.js"

const graph = {} as SchemaGraph

const stringCodec: ConcreteCodec = {
  read: (reader: WireReader, index: number) =>
    reader.node(index, "string-value", (node) => node.val),
  write: (value: unknown, writer: WireWriter) =>
    writer.add({ tag: "string-value", val: value as string }),
}

describe("compiled wire runtime", () => {
  it.effect(
    "defers eager stream reads until all siblings are wrapped and preserves Effect services",
    () =>
      Effect.gen(function* () {
        class Message extends Context.Service<Message, string>()("compiled/Message") {}
        let pulled = 0
        const source = Stream.fromEffect(
          Message.pipe(Effect.tap(() => Effect.sync(() => pulled++))),
        )
        const codec: ConcreteCodec = {
          read: () => undefined,
          write: (value, writer) =>
            writer.add({
              tag: "tuple-value",
              val: [writer.stream(value, stringCodec), writer.stream(value, stringCodec)],
            }),
        }
        let pending: Promise<IteratorResult<unknown>> | undefined
        let count = 0
        const wrap = vi.spyOn(SchemaValueStream, "wrap").mockImplementation(async (prepared) => {
          if (++count === 2) throw new Error("second wrap failed")
          pending = prepared[Symbol.asyncIterator]().next()
          return new SchemaValueStream(prepared)
        })
        try {
          yield* Effect.flip(
            writeConcreteAsync(codec, source).pipe(Effect.provideService(Message, "captured")),
          )
          expect(pulled).toBe(0)
          expect(yield* Effect.promise(() => pending!)).toEqual({ done: true, value: undefined })
        } finally {
          wrap.mockRestore()
        }
        const one: ConcreteCodec = {
          read: () => undefined,
          write: (value, writer) => writer.stream(value, stringCodec),
        }
        const tree = yield* writeConcreteAsync(one, source).pipe(
          Effect.provideService(Message, "captured"),
        )
        const node = tree.valueNodes[tree.root]!
        expect(node.tag).toBe("stream-value")
        const iterable = yield* Effect.promise(() => SchemaValueStream.unwrap((node as any).val))
        const iterator = iterable[Symbol.asyncIterator]()
        const item = yield* Effect.promise(() => iterator.next())
        expect(item.value).toEqual({
          root: 0,
          valueNodes: [{ tag: "string-value", val: "captured" }],
        })
        yield* Effect.promise(async () => {
          await iterator.return?.()
        })
        expect(pulled).toBe(1)
      }),
  )

  it("transfers affine resources only after the complete value prepares successfully", () => {
    const raw = {} as any
    const handle = createGuestPermissionCardHandle(PERMISSION_CARD_INTERNAL, raw)
    const permission: ConcreteCodec = {
      read: (r, i) => r.node(i, "permission-card-handle", (n) => r.resource(n)),
      write: (v, w) => w.resource("permission-card-handle", v as any),
    }
    const invalid: ConcreteCodec = {
      read: () => undefined,
      write: (v, w) => {
        permission.write(v, w)
        throw new TypeError("bad sibling")
      },
    }
    expect(() => writeConcrete(invalid, handle)).toThrow("bad sibling")
    expect(peekGuestPermissionCardHandle(PERMISSION_CARD_INTERNAL, handle)).toBe(raw)
    expect(writeConcrete(permission, handle).valueNodes).toEqual([
      { tag: "permission-card-handle", val: raw },
    ])
    expect(() => writeConcrete(permission, handle)).toThrow("already transferred")
  })

  it.effect(
    "rolls back adoption and drains resources when the source schema rejects a decoded value",
    () =>
      Effect.gen(function* () {
        const drop = vi.fn()
        const raw = { [Symbol.dispose]: drop }
        const permission: ConcreteCodec = {
          read: (r, i) => r.node(i, "permission-card-handle", (n) => r.resource(n)),
          write: () => 0,
        }
        const runtime = compiledWire(Schema.String, graph, permission)
        const tree = {
          valueNodes: [{ tag: "permission-card-handle" as const, val: raw as any }],
          root: 0,
        }
        yield* Effect.flip(runtime.decode(tree))
        expect(drop).toHaveBeenCalledTimes(1)
        expect(tree.valueNodes[0]!.val).toBeUndefined()
      }),
  )

  it.effect("awaits unread stream cleanup after a malformed sibling", () =>
    Effect.gen(function* () {
      const returned = vi.fn(async () => ({ done: true as const, value: undefined }))
      const drop = vi.fn()
      const raw = {
        source: {
          [Symbol.asyncIterator]: () => ({
            next: async () => ({ done: true as const, value: undefined }),
            return: returned,
          }),
        },
        [Symbol.dispose]: drop,
      }
      const stream: ConcreteCodec = {
        read: (r, i) => r.node(i, "stream-value", (n) => r.stream(n, stringCodec)),
        write: () => 0,
      }
      const pair: ConcreteCodec = {
        read: (r, i) =>
          r.node(i, "tuple-value", (n) => [
            stream.read(r, n.val[0]),
            stringCodec.read(r, n.val[1]),
          ]),
        write: () => 0,
      }
      yield* Effect.flip(
        compiledWire(Schema.Unknown, graph, pair).decode({
          root: 2,
          valueNodes: [
            { tag: "stream-value", val: raw as any },
            { tag: "bool-value", val: true },
            { tag: "tuple-value", val: [0, 1] },
          ],
        }),
      )
      expect(returned).toHaveBeenCalledTimes(1)
      expect(drop).toHaveBeenCalledTimes(1)
    }),
  )

  it.effect("applies the source schema transformation around concrete wire conversion", () =>
    Effect.gen(function* () {
      const runtime = compiledWire(Schema.NumberFromString, graph, stringCodec)
      const tree = yield* runtime.encode(42)
      expect(tree.valueNodes).toEqual([{ tag: "string-value", val: "42" }])
      expect(yield* runtime.decode(tree)).toBe(42)
    }),
  )

  it.effect("rejects aliases and unreachable arena nodes", () =>
    Effect.gen(function* () {
      const pair: ConcreteCodec = {
        read: (reader, index) =>
          reader.node(index, "tuple-value", (node) => [
            stringCodec.read(reader, node.val[0]),
            stringCodec.read(reader, node.val[1]),
          ]),
        write: () => 0,
      }
      const runtime = compiledWire(Schema.Unknown, graph, pair)
      const aliased = {
        valueNodes: [
          { tag: "string-value" as const, val: "x" },
          { tag: "tuple-value" as const, val: [0, 0] },
        ],
        root: 1,
      }
      yield* Effect.flip(runtime.decode(aliased))

      const primitive = compiledWire(Schema.String, graph, stringCodec)
      yield* Effect.flip(
        primitive.decode({
          valueNodes: [
            { tag: "string-value", val: "used" },
            { tag: "string-value", val: "orphan" },
          ],
          root: 0,
        }),
      )
    }),
  )
})
