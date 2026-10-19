import { Context, Effect, Exit, Stream } from "effect"

/** Eager ownership must also be released when a producer is never polled. */
export const streamDisposals = new WeakMap<
  object,
  (exit: Exit.Exit<unknown, unknown>) => Promise<void>
>()

/** Acquired endpoints remain owned across arbitrary lazy Stream composition. */
export const HttpStreamOwner = Context.Reference<Set<() => Promise<void>> | undefined>(
  "golem/http/StreamOwner",
  { defaultValue: () => undefined },
)

export function ownStream<A, E, R>(
  stream: Stream.Stream<A, E, R>,
  close: (exit: Exit.Exit<unknown, unknown>) => Promise<void>,
) {
  const owned = stream.pipe(Stream.onExit((exit) => Effect.promise(() => close(exit))))
  streamDisposals.set(owned, close)
  return owned
}
