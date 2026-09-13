import { Effect, Schema } from "effect"
import { beforeEach, describe, expect, it, vi } from "vitest"
import type { PermissionCard } from "golem:core/types@2.0.0"
import { toolDefinition, resetTools } from "../src/Tool.js"
import * as GolemSchema from "../src/Schema.js"
import { invokeRegistered } from "../src/internal/tool/runtime.js"
import { compile } from "../src/WitCodec.js"
import { Uint32 } from "../src/WitTypes.js"
import { emptyMetadata } from "../src/internal/schema-model/model.js"
import { createGuestPermissionCardHandle } from "../src/internal/schema-model/permissionCardHandle.js"
import { PERMISSION_CARD_INTERNAL } from "../src/internal/schema-model/permissionCardInternal.js"

describe("tool input graph validation", () => {
  beforeEach(resetTools)

  it("accepts descriptor-derived field metadata and recursive definition identities", async () => {
    interface Node {
      readonly label: string
      readonly children: ReadonlyArray<Node>
    }
    const Node: Schema.Codec<Node> = Schema.suspend(() =>
      Schema.Struct({
        label: Schema.String,
        children: Schema.Array(Node),
      }),
    )
    const Message = Schema.String.annotate({ description: "The message" })
    const codec = Effect.runSync(compile(Schema.Struct({ message: Message, node: Node })))
    const handler = vi.fn(
      (_input: { readonly message: string; readonly node: Node }) => Effect.void,
    )
    toolDefinition("recursive")
      .body((body) => body.positional("message", Message).positional("node", Node))
      .implement({ recursive: handler })
    const graph = codec.schemaGraph
    const root = graph.typeNodes[graph.root]!
    if (root.body.tag !== "record-type") throw new Error("expected record")
    const fields = root.body.val
    const input = {
      graph: {
        ...graph,
        defs: graph.defs.map((def, index) => ({ ...def, id: `caller-node-${index}` })),
        typeNodes: graph.typeNodes.map((node, index) =>
          index === graph.root
            ? {
                ...root,
                body: {
                  tag: "record-type" as const,
                  val: fields.map((field) => ({ ...field, metadata: emptyMetadata() })),
                },
              }
            : node,
        ),
      },
      value: Effect.runSync(
        codec.encode({
          message: "hello",
          node: { label: "root", children: [{ label: "leaf", children: [] }] },
        }),
      ),
    }
    await invokeRegistered("recursive", [], input, undefined, undefined, {})
    expect(handler).toHaveBeenCalledOnce()
    expect(handler.mock.calls[0]?.[0]).toEqual({
      message: "hello",
      node: { label: "root", children: [{ label: "leaf", children: [] }] },
    })
  })

  it("decodes count flags from the canonical u32 wire representation", async () => {
    const handler = vi.fn((_input: { readonly verbosity: number }) => Effect.void)
    toolDefinition("count")
      .body((body) => body.countFlag("verbosity"))
      .implement({ count: handler })
    const codec = Effect.runSync(compile(Schema.Struct({ verbosity: Uint32 })))
    await invokeRegistered(
      "count",
      [],
      {
        graph: codec.schemaGraph,
        value: Effect.runSync(codec.encode({ verbosity: 3 })),
      },
      undefined,
      undefined,
      {},
    )
    expect(handler.mock.calls[0]?.[0]).toEqual({ verbosity: 3 })
  })

  it("rejects a typed input whose graph does not match the selected command body", async () => {
    const expected = Effect.runSync(compile(Schema.Struct({})))
    const unrelated = Effect.runSync(compile(Schema.String))
    const handler = vi.fn(() => Effect.void)
    toolDefinition("graph-check")
      .body((body) => body)
      .implement({ graphCheck: handler })

    await expect(
      invokeRegistered(
        "graph-check",
        [],
        {
          graph: unrelated.schemaGraph,
          value: Effect.runSync(expected.encode({})),
        },
        undefined,
        undefined,
        {},
      ),
    ).rejects.toMatchObject({ tag: "invalid-input" })
    expect(handler).not.toHaveBeenCalled()
  })

  it("rejects malformed graphs without consuming a permission card and accepts a corrected retry", async () => {
    const Card = GolemSchema.PermissionCard({ polymorphic: false })
    const codec = Effect.runSync(compile(Schema.Struct({ card: Card })))
    const handler = vi.fn(() => Effect.void)
    toolDefinition("card-check")
      .body((body) => body.positional("card", Card))
      .implement({ cardCheck: handler })
    const value = Effect.runSync(
      codec.encode({
        card: createGuestPermissionCardHandle(PERMISSION_CARD_INTERNAL, {} as PermissionCard),
      }),
    )
    await expect(
      invokeRegistered(
        "card-check",
        [],
        {
          graph: { ...codec.schemaGraph, root: codec.schemaGraph.typeNodes.length },
          value,
        },
        undefined,
        undefined,
        {},
      ),
    ).rejects.toMatchObject({ tag: "invalid-input" })
    expect(handler).not.toHaveBeenCalled()
    await invokeRegistered(
      "card-check",
      [],
      { graph: codec.schemaGraph, value },
      undefined,
      undefined,
      {},
    )
    expect(handler).toHaveBeenCalledOnce()
  })

  it("compares schema structure rather than flat wire node indices", async () => {
    const codec = Effect.runSync(compile(Schema.Struct({})))
    const handler = vi.fn(() => Effect.void)
    toolDefinition("graph-check")
      .body((body) => body)
      .implement({ graphCheck: handler })
    const root = codec.schemaGraph.typeNodes[codec.schemaGraph.root]!
    await invokeRegistered(
      "graph-check",
      [],
      {
        graph: { ...codec.schemaGraph, typeNodes: [root, root], root: 1 },
        value: Effect.runSync(codec.encode({})),
      },
      undefined,
      undefined,
      {},
    )
    expect(handler).toHaveBeenCalledOnce()
  })
})
