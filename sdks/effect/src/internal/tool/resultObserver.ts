import { Deferred, Effect, Exit, Fiber, Option } from "effect"
import { CollectionOwnership } from "./collectionOwnership.js"

// Generated wrappers preserve the cancel Effect when they map the result.
const detachers = new WeakMap<object, Effect.Effect<void>>()

export const bindResultDetach = (cancel: Effect.Effect<void>, detach: Effect.Effect<void>) => {
  const boundCancel = Effect.suspend(() => cancel)
  detachers.set(boundCancel, detach)
  return boundCancel
}

export const resultDetach = (cancel: Effect.Effect<void>): Effect.Effect<void> =>
  detachers.get(cancel) ?? Effect.die(new Error("tool result detach binding is missing"))

export const wholeToolCall = <S extends { readonly cancel: Effect.Effect<void> }, A, E, R, SE, SR>(
  start: Effect.Effect<S, SE, SR>,
  use: (started: S) => Effect.Effect<A, E, R>,
) =>
  Effect.scoped(
    Effect.uninterruptibleMask((restore) =>
      Effect.gen(function* () {
        const started = yield* start
        const worker = yield* Effect.forkScoped(restore(Effect.suspend(() => use(started))))
        return yield* restore(Fiber.join(worker)).pipe(Effect.onInterrupt(() => started.cancel))
      }),
    ),
  )

export const makeResultObserver = <A>(
  future: { get(): Promise<A>; cancel(): void; [Symbol.dispose]?: () => void },
  discard: (result: A | undefined, error: unknown) => Promise<void>,
) =>
  Effect.gen(function* () {
    const detached = yield* Deferred.make<void>()
    let released = false
    let disposed = false
    let observing = false
    let delivered = false
    let settled = false
    let value: A | undefined
    let error: unknown
    let discarded = false
    let pending: Promise<A> | undefined
    const discardUnclaimed = async () => {
      if (delivered || discarded) return
      discarded = true
      await discard(value, error)
    }
    const dispose = () => {
      if (disposed) return
      disposed = true
      future[Symbol.dispose]?.()
    }
    const detach = Effect.promise(async () => {
      released = true
      if (!pending || settled) {
        try {
          if (settled) await discardUnclaimed()
        } finally {
          dispose()
        }
      }
    }).pipe(Effect.ensuring(Deferred.succeed(detached, undefined)), Effect.asVoid)
    const cancel = Effect.sync(() => {
      if (!disposed) future.cancel()
    })
    detachers.set(cancel, detach)
    const result = Effect.flatMap(Effect.serviceOption(CollectionOwnership), (owner) =>
      Effect.suspend(() => {
        if (released || observing || delivered)
          return Effect.fail({ tag: "protocol-error", val: "tool result observer is unavailable" })
        observing = true
        pending ??= Promise.resolve()
          .then(() => future.get())
          .then(
            async (result) => {
              value = result
              if (released) await discardUnclaimed()
              return result
            },
            async (cause: unknown) => {
              error = cause
              if (released) await discardUnclaimed()
              throw cause
            },
          )
          .finally(async () => {
            settled = true
            if (released) {
              try {
                await discardUnclaimed()
              } finally {
                dispose()
              }
            }
          })
        return Effect.exit(
          Effect.raceFirst(
            Effect.tryPromise({ try: () => pending!, catch: (error) => error }),
            Deferred.await(detached).pipe(
              Effect.andThen(
                Effect.fail({ tag: "protocol-error", val: "tool result observer detached" }),
              ),
            ),
          ),
        ).pipe(
          Effect.flatMap((exit) =>
            Effect.suspend(() => {
              if (Exit.hasInterrupts(exit)) return exit
              if (released)
                return Effect.fail({ tag: "protocol-error", val: "tool result observer detached" })
              delivered = true
              if (Option.isSome(owner)) owner.value.retainDiscard(() => discard(value, error))
              return exit
            }),
          ),
          Effect.onInterrupt(() => detach),
        )
      }),
    )
    return { result, cancel, detach }
  })
