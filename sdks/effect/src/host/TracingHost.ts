/**
 * Host service for `golem:api/context@1.5.0`. Wraps `startSpan`,
 * `currentContext`, and `allowForwardingTraceContextHeaders` into an
 * Effect-typed surface so SDK code can interact with the host's
 * tracing context via DI rather than a direct ESM specifier import.
 *
 * @internal — not re-exported from `src/index.ts`.
 */
import { Context, Layer } from "effect"
import * as ContextHost from "golem:api/context@1.5.0"

export interface TracingHostShape {
  /** Mirrors `golem:api/context.startSpan`. */
  readonly startSpan: (name: string) => ContextHost.Span
  /** Mirrors `golem:api/context.currentContext`. */
  readonly currentContext: () => ContextHost.InvocationContext
  /** Mirrors `golem:api/context.allowForwardingTraceContextHeaders`. */
  readonly allowForwardingTraceContextHeaders: (allow: boolean) => boolean
}

export class TracingHost extends Context.Service<TracingHost, TracingHostShape>()(
  "effect-golem/host/Tracing",
) {}

export const TracingHostLive: Layer.Layer<TracingHost> = Layer.succeed(
  TracingHost,
  TracingHost.of({
    startSpan: (name) => ContextHost.startSpan(name),
    currentContext: () => ContextHost.currentContext(),
    allowForwardingTraceContextHeaders: (allow) =>
      ContextHost.allowForwardingTraceContextHeaders(allow),
  }),
)
