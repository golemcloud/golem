/** Private, interruptible Effect seam for finite Durable Streams host attempts. */
import { Context, Effect, Layer, Scope } from "effect"
import * as Host from "golem:agent/durable-streams@2.0.0"
import type { Secret } from "golem:core/types@2.0.0"

export interface Reader {
  readonly read: (
    request: Host.DurableStreamReadRequest,
  ) => Effect.Effect<Host.DurableStreamBatch, Host.DurableStreamError>
}

export interface Writer {
  readonly append: (
    request: Host.DurableStreamAppendRequest,
  ) => Effect.Effect<Host.DurableStreamAppendReceipt, Host.DurableStreamError>
}

export interface DurableStreamsClientShape {
  readonly makeReader: (
    options: Host.DurableStreamReaderOptions,
    auth: Secret | undefined,
  ) => Effect.Effect<Reader, never, Scope.Scope>
  readonly makeWriter: (
    options: Host.DurableStreamWriterOptions,
    auth: Secret | undefined,
  ) => Effect.Effect<Writer, never, Scope.Scope>
}

export class DurableStreamsClient extends Context.Service<
  DurableStreamsClient,
  DurableStreamsClientShape
>()("effect-golem/host/DurableStreams") {}

const attempt = <A>(run: () => Promise<A>): Effect.Effect<A, Host.DurableStreamError> =>
  Effect.tryPromise({ try: run, catch: (cause) => cause }).pipe(
    Effect.catch((cause) =>
      typeof cause === "object" && cause !== null && "kind" in cause && "message" in cause
        ? Effect.fail(cause as Host.DurableStreamError)
        : Effect.die(cause),
    ),
  )

export const DurableStreamsLive = Layer.succeed(
  DurableStreamsClient,
  DurableStreamsClient.of({
    makeReader: (options, auth) =>
      Effect.map(
        scopedAttempt(
          () => new Host.DurableStreamReader(options, auth),
          (reader, request: Host.DurableStreamReadRequest) => reader.read(request),
        ),
        (read) => ({ read }),
      ),
    makeWriter: (options, auth) =>
      Effect.map(
        scopedAttempt(
          () => new Host.DurableStreamWriter(options, auth),
          (writer, request: Host.DurableStreamAppendRequest) => writer.append(request),
        ),
        (append) => ({ append }),
      ),
  }),
)

// WIT async methods borrow their resource until their Promise settles. Effect interruption
// stops waiting, not the HTTP attempt: scope exit must defer drop while any borrow is live.
const scopedAttempt = <Resource extends object, Request, Response>(
  make: () => Resource,
  run: (resource: Resource, request: Request) => Promise<Response>,
) =>
  Effect.acquireRelease(
    Effect.sync(() => {
      const resource = make()
      let active = 0
      let released = false
      const dispose = () => {
        if (released && active === 0)
          (resource as { [Symbol.dispose]: () => void })[Symbol.dispose]()
      }
      return {
        call: (request: Request) =>
          attempt(async () => {
            if (released) throw new Error("Durable Streams resource scope is closed")
            active++
            try {
              return await run(resource, request)
            } finally {
              active--
              dispose()
            }
          }),
        release: () => {
          released = true
          dispose()
        },
      }
    }),
    (state) => Effect.sync(state.release),
  ).pipe(Effect.map((state) => state.call))
