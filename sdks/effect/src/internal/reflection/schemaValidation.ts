import type {
  NumericBound,
  NumericRestrictions,
  QuantityValue,
  SchemaGraph,
  SchemaType,
  SchemaValue,
  UnionBranch,
} from "../schema-model/model.js"
import {
  floatFromBits,
  isQuantityValueRepresentable,
  quantityLessOrEqual,
} from "../schema-model/validation.js"
import { assertSchemaValueRepresentable } from "../schema-model/wit.js"
import type * as CoreTypes from "golem:core/types@2.0.0"

/** Validate a WIT value tree without lifting or transferring any affine resources. */
export function schemaValueTreeConforms(
  graph: SchemaGraph,
  type: SchemaType,
  tree: CoreTypes.SchemaValueTree,
): boolean {
  const onPath = new Set<number>()
  const reached = new Set<number>()
  const capabilities = new Set<unknown>()
  const decode = (index: number): SchemaValue => {
    if (
      !Number.isInteger(index) ||
      index < 0 ||
      index >= tree.valueNodes.length ||
      onPath.has(index)
    )
      throw new TypeError("invalid schema value node reference")
    onPath.add(index)
    reached.add(index)
    const node = tree.valueNodes[index]
    const child = (i: number) => decode(i)
    let value: SchemaValue
    switch (node.tag) {
      case "bool-value":
        value = { tag: "bool", value: node.val }
        break
      case "s8-value":
        value = { tag: "s8", value: node.val }
        break
      case "s16-value":
        value = { tag: "s16", value: node.val }
        break
      case "s32-value":
        value = { tag: "s32", value: node.val }
        break
      case "s64-value":
        value = { tag: "s64", value: node.val }
        break
      case "u8-value":
        value = { tag: "u8", value: node.val }
        break
      case "u16-value":
        value = { tag: "u16", value: node.val }
        break
      case "u32-value":
        value = { tag: "u32", value: node.val }
        break
      case "u64-value":
        value = { tag: "u64", value: node.val }
        break
      case "f32-value":
        value = { tag: "f32", value: node.val }
        break
      case "f64-value":
        value = { tag: "f64", value: node.val }
        break
      case "char-value":
        value = { tag: "char", value: node.val }
        break
      case "string-value":
        value = { tag: "string", value: node.val }
        break
      case "record-value":
        value = { tag: "record", fields: node.val.map(child) }
        break
      case "variant-value":
        value = {
          tag: "variant",
          caseIndex: node.val.case_,
          payload: node.val.payload === undefined ? undefined : child(node.val.payload),
        }
        break
      case "enum-value":
        value = { tag: "enum", caseIndex: node.val }
        break
      case "flags-value":
        value = { tag: "flags", flags: node.val }
        break
      case "tuple-value":
        value = { tag: "tuple", elements: node.val.map(child) }
        break
      case "list-value":
        value = { tag: "list", elements: node.val.map(child) }
        break
      case "fixed-list-value":
        value = { tag: "fixed-list", elements: node.val.map(child) }
        break
      case "map-value":
        value = {
          tag: "map",
          entries: node.val.map((entry) => ({ key: child(entry.key), value: child(entry.value) })),
        }
        break
      case "option-value":
        value = { tag: "option", value: node.val === undefined ? undefined : child(node.val) }
        break
      case "result-value":
        value = {
          tag: "result",
          result: {
            tag: node.val.tag === "ok-value" ? "ok" : "err",
            value: node.val.val === undefined ? undefined : child(node.val.val),
          },
        }
        break
      case "text-value":
        value = { tag: "text", text: node.val.text, language: node.val.language }
        break
      case "binary-value":
        value = { tag: "binary", bytes: node.val.bytes, mimeType: node.val.mimeType }
        break
      case "path-value":
        value = { tag: "path", value: node.val }
        break
      case "url-value":
        value = { tag: "url", value: node.val }
        break
      case "datetime-value":
        value = { tag: "datetime", value: node.val }
        break
      case "duration-value":
        value = { tag: "duration", nanoseconds: node.val.nanoseconds }
        break
      case "quantity-value-node":
        value = { tag: "quantity", value: node.val }
        break
      case "union-value":
        value = { tag: "union", unionTag: node.val.tag, body: child(node.val.body) }
        break
      case "secret-value":
        if (node.val === undefined || capabilities.has(node.val))
          throw new TypeError("invalid secret")
        capabilities.add(node.val)
        value = { tag: "secret", handle: null as never }
        break
      case "quota-token-handle":
        if (node.val === undefined || capabilities.has(node.val))
          throw new TypeError("invalid quota token")
        capabilities.add(node.val)
        value = { tag: "quota-token", handle: null as never }
        break
      case "permission-card-handle":
        if (node.val === undefined || capabilities.has(node.val))
          throw new TypeError("invalid permission card")
        capabilities.add(node.val)
        value = { tag: "permission-card", handle: null as never }
        break
      case "stream-value":
        if (node.val === undefined || capabilities.has(node.val))
          throw new TypeError("invalid stream")
        capabilities.add(node.val)
        value = { tag: "stream", handle: null as never }
        break
    }
    onPath.delete(index)
    return value
  }
  try {
    const value = decode(tree.root)
    return reached.size === tree.valueNodes.length && schemaValueMatches(graph, type, value)
  } catch {
    return false
  }
}
export function schemaValueConforms(
  graph: SchemaGraph,
  type: SchemaType,
  value: SchemaValue,
): boolean {
  try {
    assertSchemaValueRepresentable(value)
  } catch {
    return false
  }
  return schemaValueMatches(graph, type, value)
}

export function schemaValueMatches(
  graph: SchemaGraph,
  type: SchemaType,
  value: SchemaValue,
): boolean {
  const body = resolveType(graph, type, new Set())
  if (!body) return false
  const resolvedBody = body.body
  switch (resolvedBody.tag) {
    case "bool":
      return value.tag === "bool" && typeof value.value === "boolean"
    case "s8":
      return (
        value.tag === "s8" &&
        integerInRange(value.value, -128n, 127n) &&
        numericRestrictionsMatch(resolvedBody.restrictions, BigInt(value.value))
      )
    case "s16":
      return (
        value.tag === "s16" &&
        integerInRange(value.value, -32768n, 32767n) &&
        numericRestrictionsMatch(resolvedBody.restrictions, BigInt(value.value))
      )
    case "s32":
      return (
        value.tag === "s32" &&
        integerInRange(value.value, -(2n ** 31n), 2n ** 31n - 1n) &&
        numericRestrictionsMatch(resolvedBody.restrictions, BigInt(value.value))
      )
    case "s64":
      return (
        value.tag === "s64" &&
        typeof value.value === "bigint" &&
        value.value >= -(2n ** 63n) &&
        value.value <= 2n ** 63n - 1n &&
        numericRestrictionsMatch(resolvedBody.restrictions, value.value)
      )
    case "u8":
      return (
        value.tag === "u8" &&
        integerInRange(value.value, 0n, 255n) &&
        numericRestrictionsMatch(resolvedBody.restrictions, BigInt(value.value))
      )
    case "u16":
      return (
        value.tag === "u16" &&
        integerInRange(value.value, 0n, 65535n) &&
        numericRestrictionsMatch(resolvedBody.restrictions, BigInt(value.value))
      )
    case "u32":
      return (
        value.tag === "u32" &&
        integerInRange(value.value, 0n, 2n ** 32n - 1n) &&
        numericRestrictionsMatch(resolvedBody.restrictions, BigInt(value.value))
      )
    case "u64":
      return (
        value.tag === "u64" &&
        typeof value.value === "bigint" &&
        value.value >= 0n &&
        value.value <= 2n ** 64n - 1n &&
        numericRestrictionsMatch(resolvedBody.restrictions, value.value)
      )
    case "f32":
      return (
        value.tag === "f32" &&
        typeof value.value === "number" &&
        numericRestrictionsMatch(resolvedBody.restrictions, Math.fround(value.value))
      )
    case "f64":
      return (
        value.tag === "f64" &&
        typeof value.value === "number" &&
        numericRestrictionsMatch(resolvedBody.restrictions, value.value)
      )
    case "char":
      return value.tag === "char" && isUnicodeScalar(value.value)
    case "string":
      return value.tag === "string" && typeof value.value === "string"
    case "path":
      return (
        value.tag === "path" &&
        typeof value.value === "string" &&
        pathMatches(resolvedBody.spec.allowedExtensions, value.value)
      )
    case "url":
      return (
        value.tag === "url" &&
        typeof value.value === "string" &&
        urlMatches(resolvedBody.restrictions, value.value)
      )
    case "datetime":
      return (
        value.tag === "datetime" &&
        typeof value.value.seconds === "bigint" &&
        value.value.seconds >= -(2n ** 63n) &&
        value.value.seconds <= 2n ** 63n - 1n &&
        Number.isInteger(value.value.nanoseconds) &&
        value.value.nanoseconds >= 0 &&
        value.value.nanoseconds < 1_000_000_000
      )
    case "duration":
      return (
        value.tag === "duration" &&
        typeof value.nanoseconds === "bigint" &&
        value.nanoseconds >= -(2n ** 63n) &&
        value.nanoseconds <= 2n ** 63n - 1n
      )
    case "secret":
    case "quota-token":
      return value.tag === resolvedBody.tag
    case "permission-card":
      return value.tag === "permission-card"
    case "text": {
      if (value.tag !== "text") return false
      if (
        typeof value.text !== "string" ||
        (value.language !== undefined && typeof value.language !== "string")
      ) {
        return false
      }
      const length = [...value.text].length
      return (
        (value.language === undefined ||
          resolvedBody.restrictions.languages === undefined ||
          resolvedBody.restrictions.languages.includes(value.language)) &&
        (resolvedBody.restrictions.minLength === undefined ||
          length >= resolvedBody.restrictions.minLength) &&
        (resolvedBody.restrictions.maxLength === undefined ||
          length <= resolvedBody.restrictions.maxLength) &&
        (resolvedBody.restrictions.regex === undefined ||
          regexMatches(resolvedBody.restrictions.regex, value.text))
      )
    }
    case "binary":
      return (
        value.tag === "binary" &&
        value.bytes instanceof Uint8Array &&
        (value.mimeType === undefined || typeof value.mimeType === "string") &&
        (value.mimeType === undefined ||
          resolvedBody.restrictions.mimeTypes === undefined ||
          resolvedBody.restrictions.mimeTypes.includes(value.mimeType)) &&
        (resolvedBody.restrictions.minBytes === undefined ||
          value.bytes.byteLength >= resolvedBody.restrictions.minBytes) &&
        (resolvedBody.restrictions.maxBytes === undefined ||
          value.bytes.byteLength <= resolvedBody.restrictions.maxBytes)
      )
    case "quantity":
      return value.tag === "quantity" && quantityMatches(resolvedBody.spec, value.value)
    case "record":
      return (
        value.tag === "record" &&
        value.fields.length === resolvedBody.fields.length &&
        resolvedBody.fields.every((entry, index) =>
          schemaValueMatches(graph, entry.body, value.fields[index]),
        )
      )
    case "variant": {
      if (value.tag !== "variant") return false
      const selected = resolvedBody.cases[value.caseIndex]
      return (
        !!selected &&
        (selected.payload === undefined
          ? value.payload === undefined
          : value.payload !== undefined &&
            schemaValueMatches(graph, selected.payload, value.payload))
      )
    }
    case "enum":
      return (
        value.tag === "enum" && value.caseIndex >= 0 && value.caseIndex < resolvedBody.cases.length
      )
    case "flags":
      return (
        value.tag === "flags" &&
        Array.isArray(value.flags) &&
        value.flags.length === resolvedBody.names.length &&
        value.flags.every((flag) => typeof flag === "boolean")
      )
    case "tuple":
      return (
        value.tag === "tuple" &&
        value.elements.length === resolvedBody.elements.length &&
        resolvedBody.elements.every((entry, index) =>
          schemaValueMatches(graph, entry, value.elements[index]),
        )
      )
    case "list":
      return (
        value.tag === "list" &&
        value.elements.every((entry) => schemaValueMatches(graph, resolvedBody.element, entry))
      )
    case "fixed-list":
      return (
        value.tag === "fixed-list" &&
        value.elements.length === resolvedBody.length &&
        value.elements.every((entry) => schemaValueMatches(graph, resolvedBody.element, entry))
      )
    case "map":
      return (
        value.tag === "map" &&
        value.entries.every(
          (entry) =>
            schemaValueMatches(graph, resolvedBody.key, entry.key) &&
            schemaValueMatches(graph, resolvedBody.value, entry.value),
        )
      )
    case "option":
      return (
        value.tag === "option" &&
        (value.value === undefined || schemaValueMatches(graph, resolvedBody.element, value.value))
      )
    case "result":
      return (
        value.tag === "result" &&
        (value.result.tag === "ok"
          ? resolvedBody.ok === undefined
            ? value.result.value === undefined
            : value.result.value !== undefined &&
              schemaValueMatches(graph, resolvedBody.ok, value.result.value)
          : resolvedBody.err === undefined
            ? value.result.value === undefined
            : value.result.value !== undefined &&
              schemaValueMatches(graph, resolvedBody.err, value.result.value))
      )
    case "union": {
      if (value.tag !== "union") return false
      const branch = resolvedBody.branches.find((entry) => entry.tag === value.unionTag)
      return (
        !!branch &&
        schemaValueMatches(graph, branch.body, value.body) &&
        discriminatorMatches(graph, branch, value.body)
      )
    }
    case "stream":
      return value.tag === "stream"
    case "future":
    case "ref":
      return false
  }
}

function discriminatorMatches(
  graph: SchemaGraph,
  branch: UnionBranch,
  value: SchemaValue,
): boolean {
  const discriminator = branch.discriminator
  switch (discriminator.tag) {
    case "prefix":
      return stringView(value)?.startsWith(discriminator.val) ?? false
    case "suffix":
      return stringView(value)?.endsWith(discriminator.val) ?? false
    case "contains":
      return stringView(value)?.includes(discriminator.val) ?? false
    case "regex": {
      const text = stringView(value)
      return text !== undefined && regexMatches(discriminator.val, text)
    }
    case "field-equals": {
      const record = recordView(graph, branch.body, value)
      if (!record) return false
      const index = record.names.indexOf(discriminator.val.fieldName)
      if (index < 0) return false
      return (
        discriminator.val.literal === undefined ||
        stringView(record.values[index]) === discriminator.val.literal
      )
    }
    case "field-absent": {
      const record = recordView(graph, branch.body, value)
      return !!record && !record.names.includes(discriminator.val)
    }
  }
}

function stringView(value: SchemaValue): string | undefined {
  switch (value.tag) {
    case "string":
    case "path":
    case "url":
      return value.value
    case "text":
      return value.text
    default:
      return undefined
  }
}

function recordView(
  graph: SchemaGraph,
  type: SchemaType,
  value: SchemaValue,
): { readonly names: string[]; readonly values: SchemaValue[] } | undefined {
  const resolved = resolveType(graph, type, new Set())
  return resolved?.body.tag === "record" &&
    value.tag === "record" &&
    value.fields.length === resolved.body.fields.length
    ? { names: resolved.body.fields.map((field) => field.name), values: value.fields }
    : undefined
}

function resolveType(
  graph: SchemaGraph,
  type: SchemaType,
  visited: Set<string>,
): SchemaType | undefined {
  if (type.body.tag !== "ref") return type
  if (visited.has(type.body.id)) return undefined
  visited.add(type.body.id)
  const definition = graph.defs.get(type.body.id)
  return definition ? resolveType(graph, definition.body, visited) : undefined
}

function integerInRange(value: number, min: bigint, max: bigint): boolean {
  return Number.isInteger(value) && BigInt(value) >= min && BigInt(value) <= max
}

function numericRestrictionsMatch(
  restrictions: NumericRestrictions | undefined,
  value: number | bigint,
): boolean {
  if (!restrictions) return true
  const compare = (bound: NumericBound): number | undefined => {
    if (typeof value === "number") {
      if (bound.tag !== "float-bits") return undefined
      const decoded = floatFromBits(bound.val)
      if (decoded === undefined || !Number.isFinite(decoded) || Number.isNaN(value)) {
        return undefined
      }
      return value < decoded ? -1 : value > decoded ? 1 : 0
    }
    if (bound.tag === "float-bits") return undefined
    return value < bound.val ? -1 : value > bound.val ? 1 : 0
  }
  const min = restrictions.min ? compare(restrictions.min) : 0
  const max = restrictions.max ? compare(restrictions.max) : 0
  return min !== undefined && max !== undefined && min >= 0 && max <= 0
}

function isUnicodeScalar(value: string): boolean {
  const points = [...value]
  if (points.length !== 1) return false
  const codePoint = points[0].codePointAt(0)
  return codePoint !== undefined && (codePoint < 0xd800 || codePoint > 0xdfff)
}

function regexMatches(pattern: string, value: string): boolean {
  try {
    return new RegExp(pattern, "u").test(value)
  } catch {
    return false
  }
}

function pathMatches(allowedExtensions: string[] | undefined, value: string): boolean {
  if (value.length === 0) return false
  if (!allowedExtensions) return true
  const name = value.split("/").at(-1)
  const dot = name?.lastIndexOf(".") ?? -1
  if (dot < 0 || dot + 1 >= (name?.length ?? 0)) return true
  return allowedExtensions.includes(name!.slice(dot + 1))
}

function urlMatches(
  restrictions: Extract<SchemaType["body"], { tag: "url" }>["restrictions"],
  value: string,
): boolean {
  try {
    const url = new URL(value)
    return (
      (restrictions.allowedSchemes === undefined ||
        restrictions.allowedSchemes.some(
          (scheme) => scheme.toLowerCase() === url.protocol.slice(0, -1).toLowerCase(),
        )) &&
      (restrictions.allowedHosts === undefined ||
        restrictions.allowedHosts.some((host) => host.toLowerCase() === url.hostname.toLowerCase()))
    )
  } catch {
    return false
  }
}

function quantityMatches(
  spec: Extract<SchemaType["body"], { tag: "quantity" }>["spec"],
  value: QuantityValue,
): boolean {
  if (!isQuantityValueRepresentable(value)) return false
  const unitAllowed =
    spec.allowedSuffixes.length === 0
      ? value.unit === spec.baseUnit
      : spec.allowedSuffixes.includes(value.unit)
  return (
    unitAllowed &&
    (spec.min === undefined || quantityLessOrEqual(spec.min, value) === true) &&
    (spec.max === undefined || quantityLessOrEqual(value, spec.max) === true)
  )
}
