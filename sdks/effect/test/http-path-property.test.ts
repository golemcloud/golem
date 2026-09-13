import { describe, expect, it } from "@effect/vitest"
import { Effect } from "effect"
import * as fc from "effect/testing/FastCheck"
import {
  agentType,
  agentVersion,
  literal,
  parseEndpointPath,
  parseMountPath,
  pathVar,
  type PathSegment,
  type QueryVariable,
  restVar,
} from "../src/Http.js"

// ---------------------------------------------------------------------------
// Arbitraries
// ---------------------------------------------------------------------------

/**
 * Variable name grammar (per `isValidVarName` in `src/Http.ts`):
 * `^[A-Za-z_][A-Za-z0-9_-]*$`. Disallow the two reserved system names
 * (`agent-type`, `agent-version`) so they only appear via the dedicated
 * `agentType()` / `agentVersion()` constructors.
 */
const varNameArb = fc
  .stringMatching(/^[A-Za-z_][A-Za-z0-9_-]*$/)
  .filter((s) => s.length > 0 && s !== "agent-type" && s !== "agent-version")

/**
 * Literal segments: any non-empty string that contains no `/`, `{`,
 * `}`, or `?`. The parser splits on `/`, reserves `{...}` for variables,
 * and (for endpoints) treats the first `?` as the query-string boundary.
 */
const literalArb = fc.stringMatching(/^[^/{}?]+$/).filter((s) => s.length > 0)

const nonRestSegmentArb: fc.Arbitrary<PathSegment> = fc.oneof(
  literalArb.map((v) => literal(v)),
  varNameArb.map((n) => pathVar(n)),
  fc.constant(agentType()),
  fc.constant(agentVersion()),
)

const restSegmentArb: fc.Arbitrary<PathSegment> = varNameArb.map((n) => restVar(n))

/**
 * Build a sequence of path segments. The catch-all (`{*rest}`) is only
 * legal as the LAST segment, so we generate a body of non-rest segments
 * plus an optional trailing rest segment.
 */
const segmentsArb: fc.Arbitrary<ReadonlyArray<PathSegment>> = fc
  .tuple(
    fc.array(nonRestSegmentArb, { maxLength: 6 }),
    fc.option(restSegmentArb, { nil: undefined }),
  )
  .map(([head, tail]) => (tail ? [...head, tail] : head))

/**
 * Mount-path segments: same as above but without `{*rest}` (rejected by
 * `parseMountPath`).
 */
const mountSegmentsArb = fc.array(nonRestSegmentArb, { maxLength: 6 })

const queryVarArb: fc.Arbitrary<QueryVariable> = fc.record({
  // Query parameter names: keep them simple so we don't accidentally
  // collide with the `&` / `=` / `?` separators the parser uses.
  queryParam: fc.stringMatching(/^[A-Za-z][A-Za-z0-9_-]*$/).filter((s) => s.length > 0),
  varName: varNameArb,
})

/** De-duplicate query keys — the parser rejects duplicates. */
const queriesArb = fc.uniqueArray(queryVarArb, {
  maxLength: 4,
  selector: (q) => q.queryParam,
})

// ---------------------------------------------------------------------------
// Format helpers (no canonical formatter exists in src/Http.ts; we
// build one here so the property has a real inverse to roundtrip
// against).
// ---------------------------------------------------------------------------

const formatSegment = (s: PathSegment): string => {
  switch (s._tag) {
    case "Literal":
      return s.value
    case "PathVar":
      return `{${s.name}}`
    case "RestVar":
      return `{*${s.name}}`
    case "SystemVar":
      return `{${s.name}}`
  }
}

const formatPath = (segments: ReadonlyArray<PathSegment>): string =>
  segments.length === 0 ? "/" : "/" + segments.map(formatSegment).join("/")

const formatQuery = (q: ReadonlyArray<QueryVariable>): string =>
  q.map(({ queryParam, varName }) => `${queryParam}={${varName}}`).join("&")

// ---------------------------------------------------------------------------
// Properties
// ---------------------------------------------------------------------------

describe("Http path parser properties", () => {
  it.effect.prop(
    "parseMountPath ∘ formatPath = id (no rest, no query)",
    { segments: mountSegmentsArb },
    ({ segments }) =>
      Effect.gen(function* () {
        const raw = formatPath(segments)
        const parsed = yield* parseMountPath(raw)
        expect(parsed).toEqual(segments)
      }),
  )

  it.effect.prop(
    "parseEndpointPath ∘ formatPath round-trips path segments (with optional rest)",
    { segments: segmentsArb },
    ({ segments }) =>
      Effect.gen(function* () {
        const raw = formatPath(segments)
        const parsed = yield* parseEndpointPath(raw)
        expect(parsed.path).toEqual(segments)
        expect(parsed.query).toEqual([])
      }),
  )

  it.effect.prop(
    "parseEndpointPath ∘ format round-trips path + inline query",
    { segments: segmentsArb, queries: queriesArb },
    ({ segments, queries }) =>
      Effect.gen(function* () {
        const path = formatPath(segments)
        const raw = queries.length === 0 ? path : `${path}?${formatQuery(queries)}`
        const parsed = yield* parseEndpointPath(raw)
        expect(parsed.path).toEqual(segments)
        expect(parsed.query).toEqual(queries)
      }),
  )

  it.effect.prop(
    "parseMountPath rejects any path containing a `{*rest}` segment",
    { head: mountSegmentsArb, tail: restSegmentArb },
    ({ head, tail }) =>
      Effect.gen(function* () {
        const raw = formatPath([...head, tail])
        const exit = yield* Effect.exit(parseMountPath(raw))
        expect(exit._tag).toBe("Failure")
      }),
  )
})
