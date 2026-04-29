import { Schema, SchemaGetter } from "effect"
import type * as CoreTypes from "golem:core/types@1.5.0"

type WitNode = CoreTypes.WitNode
type WitTypeNode = CoreTypes.WitTypeNode
type WitType = CoreTypes.WitType
type WitValue = CoreTypes.WitValue

/**
 * `WitValueTree` is the *denormalised* (tree) form of a Golem `WitValue`.
 * A `WitValue` is a graph: a flat list of `WitNode`s where children are
 * referenced by index. Effect Schema operates on trees, not graphs, so we
 * convert between the two before / after running any per-AST codec.
 *
 * The set of variants mirrors {@link CoreTypes.WitNode} 1:1, except that
 * every child is the inlined sub-tree instead of an index.
 */
export type WitValueTree =
  | { readonly tag: "record-value"; readonly val: ReadonlyArray<WitValueTree> }
  | {
      readonly tag: "variant-value"
      readonly val: readonly [number, WitValueTree | undefined]
    }
  | { readonly tag: "enum-value"; readonly val: number }
  | { readonly tag: "flags-value"; readonly val: ReadonlyArray<boolean> }
  | { readonly tag: "tuple-value"; readonly val: ReadonlyArray<WitValueTree> }
  | { readonly tag: "list-value"; readonly val: ReadonlyArray<WitValueTree> }
  | { readonly tag: "option-value"; readonly val: WitValueTree | undefined }
  | {
      readonly tag: "result-value"
      readonly val:
        | { readonly tag: "ok"; readonly val: WitValueTree | undefined }
        | { readonly tag: "err"; readonly val: WitValueTree | undefined }
    }
  | { readonly tag: "prim-u8"; readonly val: number }
  | { readonly tag: "prim-u16"; readonly val: number }
  | { readonly tag: "prim-u32"; readonly val: number }
  | { readonly tag: "prim-u64"; readonly val: bigint }
  | { readonly tag: "prim-s8"; readonly val: number }
  | { readonly tag: "prim-s16"; readonly val: number }
  | { readonly tag: "prim-s32"; readonly val: number }
  | { readonly tag: "prim-s64"; readonly val: bigint }
  | { readonly tag: "prim-float32"; readonly val: number }
  | { readonly tag: "prim-float64"; readonly val: number }
  | { readonly tag: "prim-char"; readonly val: string }
  | { readonly tag: "prim-bool"; readonly val: boolean }
  | { readonly tag: "prim-string"; readonly val: string }
  | { readonly tag: "handle"; readonly val: readonly [{ value: string }, bigint] }

export class WitGraphError {
  readonly _tag = "WitGraphError"
  constructor(readonly reason: string) {}
}

/**
 * Resolve a node-index inside a `WitType.nodes` graph, following the chain
 * of `NamedWitTypeNode.type` wrappers.
 */
const typeAt = (witType: WitType, idx: number): WitTypeNode => {
  const named = witType.nodes[idx]
  if (named === undefined) {
    throw new WitGraphError(`type node index out of range: ${idx}`)
  }
  return named.type
}

/**
 * Inflate a flat `WitValue` graph into a `WitValueTree`, using the matching
 * `WitType` to know the structural shape of each composite node (records,
 * tuples, options, etc. need to know their children's types so we can keep
 * walking).
 *
 * Both graphs are rooted at index 0 by construction.
 */
const inflate = (value: WitValue, witType: WitType): WitValueTree => {
  const nodeAt = (idx: number): WitNode => {
    const n = value.nodes[idx]
    if (n === undefined) throw new WitGraphError(`value node index out of range: ${idx}`)
    return n
  }

  const go = (valueIdx: number, typeIdx: number): WitValueTree => {
    const v = nodeAt(valueIdx)
    const t = typeAt(witType, typeIdx)
    switch (v.tag) {
      case "record-value": {
        if (t.tag !== "record-type") {
          throw new WitGraphError(`expected record-type, got ${t.tag}`)
        }
        const fields = v.val.map((childIdx, i) => go(childIdx, t.val[i]![1]))
        return { tag: "record-value", val: fields }
      }
      case "variant-value": {
        if (t.tag !== "variant-type") {
          throw new WitGraphError(`expected variant-type, got ${t.tag}`)
        }
        const [caseIdx, payloadIdx] = v.val
        const payloadTypeIdx = t.val[caseIdx]?.[1]
        const payload =
          payloadIdx === undefined || payloadTypeIdx === undefined
            ? undefined
            : go(payloadIdx, payloadTypeIdx)
        return { tag: "variant-value", val: [caseIdx, payload] }
      }
      case "tuple-value": {
        if (t.tag !== "tuple-type") {
          throw new WitGraphError(`expected tuple-type, got ${t.tag}`)
        }
        const items = v.val.map((childIdx, i) => go(childIdx, t.val[i]!))
        return { tag: "tuple-value", val: items }
      }
      case "list-value": {
        if (t.tag !== "list-type") {
          throw new WitGraphError(`expected list-type, got ${t.tag}`)
        }
        const items = v.val.map((childIdx) => go(childIdx, t.val))
        return { tag: "list-value", val: items }
      }
      case "option-value": {
        if (t.tag !== "option-type") {
          throw new WitGraphError(`expected option-type, got ${t.tag}`)
        }
        return {
          tag: "option-value",
          val: v.val === undefined ? undefined : go(v.val, t.val),
        }
      }
      case "result-value": {
        if (t.tag !== "result-type") {
          throw new WitGraphError(`expected result-type, got ${t.tag}`)
        }
        const [okType, errType] = t.val
        if (v.val.tag === "ok") {
          const child =
            v.val.val === undefined || okType === undefined ? undefined : go(v.val.val, okType)
          return { tag: "result-value", val: { tag: "ok", val: child } }
        } else {
          const child =
            v.val.val === undefined || errType === undefined ? undefined : go(v.val.val, errType)
          return { tag: "result-value", val: { tag: "err", val: child } }
        }
      }
      // Leaves: no children, no type lookup needed.
      case "enum-value":
      case "flags-value":
      case "prim-u8":
      case "prim-u16":
      case "prim-u32":
      case "prim-u64":
      case "prim-s8":
      case "prim-s16":
      case "prim-s32":
      case "prim-s64":
      case "prim-float32":
      case "prim-float64":
      case "prim-char":
      case "prim-bool":
      case "prim-string":
      case "handle":
        return v as WitValueTree
    }
  }

  return go(0, 0)
}

/**
 * Flatten a `WitValueTree` back into a `WitValue` graph. Node ordering is
 * "root first, children appended depth-first", which matches what the rest
 * of the codebase emits for `WitType` graphs.
 */
const flatten = (tree: WitValueTree): WitValue => {
  const nodes: Array<WitNode> = []

  const push = (n: WitNode): number => {
    const idx = nodes.length
    nodes.push(n)
    return idx
  }

  // Reserve index 0 for the root before recursing.
  const placeholder: WitNode = { tag: "prim-bool", val: false }
  push(placeholder)

  const go = (t: WitValueTree): WitNode => {
    switch (t.tag) {
      case "record-value":
        return { tag: "record-value", val: t.val.map((c) => push(go(c))) }
      case "variant-value": {
        const [caseIdx, payload] = t.val
        const payloadIdx = payload === undefined ? undefined : push(go(payload))
        return { tag: "variant-value", val: [caseIdx, payloadIdx] }
      }
      case "tuple-value":
        return { tag: "tuple-value", val: t.val.map((c) => push(go(c))) }
      case "list-value":
        return { tag: "list-value", val: t.val.map((c) => push(go(c))) }
      case "option-value":
        return {
          tag: "option-value",
          val: t.val === undefined ? undefined : push(go(t.val)),
        }
      case "result-value": {
        if (t.val.tag === "ok") {
          const child = t.val.val === undefined ? undefined : push(go(t.val.val))
          return { tag: "result-value", val: { tag: "ok", val: child } }
        } else {
          const child = t.val.val === undefined ? undefined : push(go(t.val.val))
          return { tag: "result-value", val: { tag: "err", val: child } }
        }
      }
      // Leaves: enumerated explicitly so a new `WitNode` variant added
      // upstream lands as a non-exhaustive switch (caught by
      // `noImplicitReturns`) rather than silently slipping through a
      // `default` cast.
      case "enum-value":
      case "flags-value":
      case "prim-u8":
      case "prim-u16":
      case "prim-u32":
      case "prim-u64":
      case "prim-s8":
      case "prim-s16":
      case "prim-s32":
      case "prim-s64":
      case "prim-float32":
      case "prim-float64":
      case "prim-char":
      case "prim-bool":
      case "prim-string":
      case "handle":
        return t as WitNode
    }
  }

  nodes[0] = go(tree)
  return { nodes }
}

/**
 * Effect-Schema base for opaque `WitValue` graphs. We don't structurally
 * validate the node array beyond shape, so the runtime guard is loose.
 */
const ValueGraph = Schema.declare(
  (u): u is WitValue => typeof u === "object" && u !== null && Array.isArray((u as WitValue).nodes),
)

/**
 * Effect-Schema base for `WitValueTree`. The tree shape is derived per-AST
 * downstream; here we only need an opaque carrier with the right type.
 */
const ValueTree = Schema.declare((_u): _u is WitValueTree => true)

/**
 * A `Schema.Codec` whose `Type` is `WitValueTree` and `Encoded` is
 * `WitValue`, parameterised over the matching `WitType`. Decoding a graph
 * that doesn't match `witType` throws a `WitGraphError` synchronously
 * (i.e. surfaces as a Schema decode failure issue).
 */
export const witGraphCodec = (witType: WitType) =>
  ValueGraph.pipe(
    Schema.decodeTo(ValueTree, {
      decode: SchemaGetter.transform((graph: WitValue) => inflate(graph, witType)),
      encode: SchemaGetter.transform((tree: WitValueTree) => flatten(tree)),
    }),
  )

// Internal helpers exported for unit tests.
export const _internal = { inflate, flatten }
