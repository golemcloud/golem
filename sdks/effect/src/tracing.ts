import { Cause, Context, Effect, Exit, Layer, Option, Tracer, type Scope } from "effect"
import type * as ContextHost from "golem:api/context@1.5.0"
import { TracingHost, type TracingHostShape } from "./host/TracingHost.js"
import { safeStringify } from "./logging.js"

/**
 * Effect-idiomatic façade over `golem:api/context@1.5.0`.
 *
 * Provides:
 *
 * - {@link layer} — installs a Tracer that creates host spans through
 *   `golem:api/context.startSpan`, so every `Effect.withSpan` ends up
 *   in the Golem oplog / propagated trace context. This is the
 *   production wiring; the agent dispatcher applies it automatically.
 *   Requires {@link TracingHost} (provided by `HostLive`).
 * - {@link withInvocationParent} — captures the host's current
 *   invocation context and wraps a program with the matching Effect
 *   `ExternalSpan` parent, so user-side `Effect.withSpan` chains under
 *   the live invocation root. Effect-typed: requires {@link TracingHost}.
 * - {@link currentContext} / {@link traceContextHeaders} — Effect-typed
 *   wrappers around the host's snapshot accessors.
 * - {@link allowForwardingTraceContextHeaders} / {@link withForwardedHeaders}
 *   — toggle for outgoing-HTTP propagation, available imperatively or
 *   as a scoped helper.
 *
 * Limitations (intentional, see oracle review):
 *
 * - The host has no concept of span events; `Effect.log*` calls become
 *   `wasi:logging` lines (via the Logger layer) but are NOT replayed as
 *   host span events. Span-event state is kept locally on the
 *   {@link GolemSpan} so anything reading them in-process still works.
 * - The host has no concept of span links; addLinks is captured locally
 *   only.
 * - The host's `startSpan` always parents the new span under
 *   `currentContext()`. We accept this for sequential / structured
 *   nested spans (the typical Effect use case). When `Effect.withSpan`
 *   requests an explicit `parent` or `root: true` AND that hint
 *   conflicts with the host's stack, we fall back to a local
 *   `Tracer.NativeSpan` so the host stack is never corrupted.
 *
 * @since 0.1.0
 */

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/**
 * Raised when one of the imperative `Tracing.*` host calls throws.
 *
 * @since 0.1.0
 * @category errors
 */
export class TracingHostError {
  readonly _tag = "TracingHostError"
  readonly message: string
  constructor(readonly cause: unknown) {
    this.message = `TracingHostError: ${cause instanceof Error ? cause.message : String(cause)}`
  }
}

// ---------------------------------------------------------------------------
// Attribute coercion
// ---------------------------------------------------------------------------

/**
 * Coerce an arbitrary Effect span attribute into the host's
 * `{ tag: "string", val }` shape. The host only carries string
 * attributes today, so all primitive types collapse to `String(value)`,
 * `Date` becomes ISO-8601, and composites use {@link safeStringify}.
 */
const toAttributeValue = (value: unknown): ContextHost.AttributeValue => {
  if (typeof value === "string") return { tag: "string", val: value }
  if (typeof value === "number" || typeof value === "boolean") {
    return { tag: "string", val: String(value) }
  }
  if (typeof value === "bigint") return { tag: "string", val: `${value.toString()}n` }
  if (value === undefined) return { tag: "string", val: "undefined" }
  if (value === null) return { tag: "string", val: "null" }
  if (value instanceof Date) return { tag: "string", val: value.toISOString() }
  return { tag: "string", val: safeStringify(value) }
}

// ---------------------------------------------------------------------------
// GolemSpan — Effect Span backed by a host Span resource handle
// ---------------------------------------------------------------------------

const ZERO_TRACE_ID = "00000000000000000000000000000000"
const ZERO_SPAN_ID = "0000000000000000"

/**
 * Decide whether a given `Effect.withSpan` request can safely be
 * delegated to the host. The host always parents new spans under
 * `currentContext()`, so we can delegate when:
 *
 *  - there is no explicit Effect parent (the host's current span IS
 *    the right parent — typically the invocation root or an outer
 *    `GolemSpan` we ourselves pushed), OR
 *  - the explicit Effect parent's `spanId` matches the host's current
 *    span (sequential / nested case).
 *
 * Otherwise (e.g. an explicit external span parent that doesn't match
 * the host stack, or a parent set by a forked fiber whose host stack
 * has drifted) we fall back to a {@link Tracer.NativeSpan} so the
 * in-Effect span tree stays coherent without corrupting the host
 * stack.
 *
 * Note: `root: true` set by the user is intentionally ignored when
 * `parent` is None — Effect normalizes `root` to `Option.isNone(parent)`
 * for top-level spans, so we can't tell explicit-root from
 * default-root apart at this layer. In Golem the host invocation
 * context is the canonical root for a worker call, so silently
 * attaching to it is the correct behaviour for the common case.
 */
const isHostStackSafe = (
  host: TracingHostShape,
  parent: Option.Option<Tracer.AnySpan>,
): boolean => {
  if (Option.isNone(parent)) return true
  try {
    const ctx = host.currentContext()
    return parent.value.spanId === ctx.spanId() && parent.value.traceId === ctx.traceId()
  } catch {
    return false
  }
}

/**
 * Effect `Span` implementation that delegates to a host `Span` resource
 * handle. Local state is kept for everything the host doesn't model
 * (events, links, status) so reading code keeps working.
 */
class GolemSpan implements Tracer.Span {
  readonly _tag = "Span" as const
  readonly attributes: Map<string, unknown> = new Map()
  readonly events: Array<[name: string, startTime: bigint, attributes: Record<string, unknown>]> =
    []
  readonly links: Array<Tracer.SpanLink>
  status: Tracer.SpanStatus

  constructor(
    private readonly handle: ContextHost.Span,
    readonly name: string,
    readonly spanId: string,
    readonly traceId: string,
    readonly parent: Option.Option<Tracer.AnySpan>,
    readonly annotations: Context.Context<never>,
    initialLinks: ReadonlyArray<Tracer.SpanLink>,
    readonly kind: Tracer.SpanKind,
    readonly sampled: boolean,
    startTime: bigint,
  ) {
    this.links = [...initialLinks]
    this.status = { _tag: "Started", startTime }
  }

  end(endTime: bigint, exit: Exit.Exit<unknown, unknown>): void {
    if (this.status._tag === "Ended") return
    this.status = {
      _tag: "Ended",
      startTime: this.status.startTime,
      endTime,
      exit,
    }
    if (Exit.isFailure(exit)) {
      try {
        this.handle.setAttribute("error", { tag: "string", val: "true" })
        this.handle.setAttribute("error.message", {
          tag: "string",
          val: Cause.pretty(exit.cause),
        })
      } catch {
        // best-effort
      }
    }
    try {
      this.handle.finish()
    } catch {
      // best-effort: never let telemetry break business logic
    }
  }

  attribute(key: string, value: unknown): void {
    this.attributes.set(key, value)
    try {
      this.handle.setAttribute(key, toAttributeValue(value))
    } catch {
      // best-effort
    }
  }

  event(name: string, startTime: bigint, attributes?: Record<string, unknown>): void {
    this.events.push([name, startTime, attributes ?? {}])
  }

  addLinks(links: ReadonlyArray<Tracer.SpanLink>): void {
    for (const l of links) this.links.push(l)
  }
}

// ---------------------------------------------------------------------------
// Tracer factory
// ---------------------------------------------------------------------------

/**
 * Build the Effect `Tracer` backed by the given {@link TracingHost}.
 * Each `withSpan` call either delegates to the host (sequential /
 * nested case) or falls back to {@link Tracer.NativeSpan}
 * (forked / explicit-parent case). All operations are best-effort —
 * host failures degrade gracefully to local-only tracing.
 */
const makeGolemTracer = (host: TracingHostShape): Tracer.Tracer =>
  Tracer.make({
    span({ annotations, kind, links, name, parent, sampled, startTime }) {
      if (!isHostStackSafe(host, parent)) {
        return new Tracer.NativeSpan({
          name,
          parent,
          annotations,
          links: [...links],
          startTime,
          kind,
          sampled,
        })
      }

      let handle: ContextHost.Span
      try {
        handle = host.startSpan(name)
      } catch {
        return new Tracer.NativeSpan({
          name,
          parent,
          annotations,
          links: [...links],
          startTime,
          kind,
          sampled,
        })
      }

      let spanId = ZERO_SPAN_ID
      let traceId = ZERO_TRACE_ID
      try {
        const ctx = host.currentContext()
        spanId = ctx.spanId()
        traceId = ctx.traceId()
      } catch {
        // host failure: keep zero ids but still produce a working span
      }

      return new GolemSpan(
        handle,
        name,
        spanId,
        traceId,
        parent,
        annotations,
        links,
        kind,
        sampled,
        startTime,
      )
    },
  })

// ---------------------------------------------------------------------------
// Layer
// ---------------------------------------------------------------------------

/**
 * Provide the Golem host tracer for the wrapped program. Every
 * `Effect.withSpan` (and the built-in span events emitted by
 * `Logger.tracerLogger`, when enabled) flows through it.
 *
 * @since 0.1.0
 * @category layers
 */
export const layer: Layer.Layer<never, never, TracingHost> = Layer.effect(
  Tracer.Tracer,
  Effect.gen(function* () {
    const host = yield* TracingHost
    return makeGolemTracer(host)
  }),
)

// ---------------------------------------------------------------------------
// Invocation-context helpers
// ---------------------------------------------------------------------------

/**
 * A snapshot of `golem:api/context.currentContext()`.
 *
 * @since 0.1.0
 * @category models
 */
export interface InvocationContextSnapshot {
  readonly traceId: string
  readonly spanId: string
  readonly traceContextHeaders: ReadonlyArray<readonly [string, string]>
}

const snapshotOf = (host: TracingHostShape): InvocationContextSnapshot => {
  const ctx = host.currentContext()
  return {
    traceId: ctx.traceId(),
    spanId: ctx.spanId(),
    traceContextHeaders: ctx.traceContextHeaders().map(([k, v]) => [k, v] as const),
  }
}

/**
 * Read the host's current invocation context, wrapped in Effect.
 *
 * @since 0.1.0
 * @category getters
 */
export const currentContext: Effect.Effect<
  InvocationContextSnapshot,
  TracingHostError,
  TracingHost
> = Effect.gen(function* () {
  const host = yield* TracingHost
  return yield* Effect.try({
    try: () => snapshotOf(host),
    catch: (e) => new TracingHostError(e),
  })
})

/**
 * Read the W3C Trace Context headers for the current invocation.
 *
 * @since 0.1.0
 * @category getters
 */
export const traceContextHeaders: Effect.Effect<
  ReadonlyArray<readonly [string, string]>,
  TracingHostError,
  TracingHost
> = Effect.gen(function* () {
  const host = yield* TracingHost
  return yield* Effect.try({
    try: () =>
      host
        .currentContext()
        .traceContextHeaders()
        .map(([k, v]) => [k, v] as const),
    catch: (e) => new TracingHostError(e),
  })
})

/**
 * Toggle the host setting that controls whether outgoing HTTP requests
 * carry W3C Trace Context headers. Returns the previous setting.
 *
 * @since 0.1.0
 * @category operations
 */
export const allowForwardingTraceContextHeaders = (
  allow: boolean,
): Effect.Effect<boolean, TracingHostError, TracingHost> =>
  Effect.gen(function* () {
    const host = yield* TracingHost
    return yield* Effect.try({
      try: () => host.allowForwardingTraceContextHeaders(allow),
      catch: (e) => new TracingHostError(e),
    })
  })

/**
 * Scoped variant of {@link allowForwardingTraceContextHeaders} — flips
 * the host setting for the surrounding `Scope`'s lifetime, then
 * restores the previous value on close.
 *
 * @since 0.1.0
 * @category operations
 */
export const useForwardedHeaders = (
  allow: boolean,
): Effect.Effect<boolean, TracingHostError, Scope.Scope | TracingHost> =>
  Effect.gen(function* () {
    const host = yield* TracingHost
    return yield* Effect.acquireRelease(
      Effect.try({
        try: () => host.allowForwardingTraceContextHeaders(allow),
        catch: (e) => new TracingHostError(e),
      }),
      (previous) =>
        Effect.try({
          try: () => host.allowForwardingTraceContextHeaders(previous),
          catch: () => undefined,
        }).pipe(Effect.ignore),
    )
  })

/**
 * Run `effect` with the host's outgoing-trace-header forwarding flag
 * temporarily set to `allow`; restores the previous value on success,
 * failure, or interruption.
 *
 * @since 0.1.0
 * @category combinators
 */
export const withForwardedHeaders = <A, E, R>(
  allow: boolean,
  effect: Effect.Effect<A, E, R>,
): Effect.Effect<A, E | TracingHostError, Exclude<R, Scope.Scope> | TracingHost> =>
  Effect.scoped(useForwardedHeaders(allow).pipe(Effect.andThen(effect)))

// ---------------------------------------------------------------------------
// Invocation-root parent injection
// ---------------------------------------------------------------------------

/**
 * Capture the host's current invocation context and wrap `effect` so
 * that every internally-created Effect span has the host root as its
 * parent. This makes the in-Effect span tree align with the live
 * invocation context without producing an extra child host span.
 *
 * Best-effort: if reading the host fails, we just run `effect`.
 *
 * Effect-typed: requires {@link TracingHost} so that the host read
 * happens *inside* the layer scope (the dispatcher applies this
 * combinator before `Effect.provide(userRuntimeLayer)` strips the
 * dependency).
 *
 * @since 0.1.0
 * @category combinators
 */
export const withInvocationParent = <A, E, R>(
  effect: Effect.Effect<A, E, R>,
): Effect.Effect<A, E, R | TracingHost> =>
  Effect.gen(function* () {
    const host = yield* TracingHost
    let snap: InvocationContextSnapshot
    try {
      snap = snapshotOf(host)
    } catch {
      return yield* effect
    }
    if (snap.traceId === ZERO_TRACE_ID || snap.spanId === ZERO_SPAN_ID) {
      return yield* effect
    }
    const external = Tracer.externalSpan({
      traceId: snap.traceId,
      spanId: snap.spanId,
      sampled: true,
    })
    return yield* Effect.withParentSpan(effect, external)
  })
