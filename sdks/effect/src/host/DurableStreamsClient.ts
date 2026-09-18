/** Private, interruptible Effect seam for finite Durable Streams host attempts. */
import { Context, Effect, Layer } from "effect"
import * as Host from "golem:agent/durable-streams@2.0.0"
import type { Secret } from "golem:core/types@2.0.0"

export interface DurableStreamsClientShape {
  readonly read: (
    request: Host.DurableStreamReadRequest,
    auth: Secret | undefined,
  ) => Effect.Effect<Host.DurableStreamBatch, Host.DurableStreamError>
  readonly append: (
    request: Host.DurableStreamAppendRequest,
    auth: Secret | undefined,
  ) => Effect.Effect<Host.DurableStreamAppendReceipt, Host.DurableStreamError>
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
    read: (request, auth) => attempt(() => Host.readDurableStreamBatch(request, auth)),
    append: (request, auth) => attempt(() => Host.appendDurableStreamBatch(request, auth)),
  }),
)
