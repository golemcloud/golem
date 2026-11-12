/**
 * Fiber-local custom durability scopes.
 *
 * Every fiber carries the token of the custom invocation scope it runs in. A composed
 * `Tracer.context` hook installs a fiber's token on the first step of each run segment and
 * restores the caller's token when the segment ends (a step returns `Yield` or throws). A
 * segment can run synchronously inside another fiber's segment, so frames form a stack.
 * Host calls that start inside a segment are attributed to that fiber's scope; the runtime
 * carries the token across the native Promise jobs, timers and host bridges they create.
 */
import { Context, Deferred, Effect, type Fiber, Scheduler, type Tracer } from "effect"
import {
  CustomInvocationContextClient,
  type CustomInvocationContextClientShape,
  type CustomScopeToken,
} from "../host/CustomInvocationContextClient.js"

/** The custom scope of the current fiber. Forks inherit it; `undefined` outside an entry point. */
export const CurrentCustomScope = Context.Reference<CustomScopeToken | undefined>(
  "effect-golem/CurrentCustomScope",
  { defaultValue: () => undefined },
)

// The run loop's segment-end marker, a registry symbol shared by every Effect copy.
const Yield = Symbol.for("effect/Effect/Yield")
const evaluate = "~effect/Effect/evaluate" as const
const scopeClient = Symbol.for("effect-golem/custom-scope-client")

interface Frame {
  readonly fiber: Fiber.Fiber<unknown, unknown>
  previous: CustomScopeToken | undefined
  token: CustomScopeToken | undefined
}

const frames: Frame[] = []

type ComposedTracer = Tracer.Tracer & {
  readonly [scopeClient]: CustomInvocationContextClientShape
}

const isComposed = (tracer: Tracer.Tracer): tracer is ComposedTracer => scopeClient in tracer

// The parent of each scope that the SDK closed.
const closedScopes = new WeakMap<CustomScopeToken, CustomScopeToken>()

/**
 * Closes the scope `token`, entered under `parent`. A segment that runs the close can have
 * started inside a Promise job of that scope, so its frame's previous token is then closed.
 */
export const closeScope = (
  client: CustomInvocationContextClientShape,
  token: CustomScopeToken,
  parent: CustomScopeToken,
): void => {
  client.close(token)
  closedScopes.set(token, parent)
}

// The nearest open ancestor, so that restoring a frame never installs a closed scope. The runtime
// resolves a closed token the same way but reports it as escaped work.
const swapBack = (client: CustomInvocationContextClientShape, token: CustomScopeToken): void => {
  let open = token
  let parent = closedScopes.get(open)
  while (parent !== undefined) {
    open = parent
    parent = closedScopes.get(open)
  }
  client.swap(open)
}

const leave = (client: CustomInvocationContextClientShape, frame: Frame): void => {
  frames.pop()
  if (frame.previous !== undefined) swapBack(client, frame.previous)
}

/** Wraps `inner` so that every fiber step runs in the fiber's custom scope. */
export const composeTracer = (
  inner: Tracer.Tracer,
  client: CustomInvocationContextClientShape,
): Tracer.Tracer => {
  if (isComposed(inner)) return inner
  const step = <X>(primitive: Tracer.EffectPrimitive<X>, fiber: Fiber.Fiber<any, any>): X =>
    inner.context === undefined
      ? (primitive as unknown as Record<typeof evaluate, (fiber: unknown) => X>)[evaluate](fiber)
      : inner.context(primitive, fiber)
  const composed: ComposedTracer = {
    [scopeClient]: client,
    span: (options) => inner.span(options),
    context: <X>(primitive: Tracer.EffectPrimitive<X>, fiber: Fiber.Fiber<any, any>): X => {
      const token = fiber.getRef(CurrentCustomScope)
      let frame = frames[frames.length - 1]
      if (frame === undefined || frame.fiber !== fiber) {
        frame = { fiber, previous: token === undefined ? undefined : client.swap(token), token }
        frames.push(frame)
      } else if (frame.token !== token) {
        if (token !== undefined) {
          const previous = client.swap(token)
          frame.previous ??= previous
        } else if (frame.previous !== undefined) {
          swapBack(client, frame.previous)
        }
        frame.token = token
      }
      let result: X
      try {
        result = step(primitive, fiber)
      } catch (error) {
        leave(client, frame)
        throw error
      }
      if ((result as unknown) === Yield) leave(client, frame)
      return result
    },
  }
  return composed
}

/**
 * Schedules fiber work as Promise jobs registered in the `root` scope. The default scheduler
 * yields through `setImmediate`, whose timer is a recorded root host call: a fiber resuming in
 * a custom scope would leave a root call that replay of a completed scope cannot reproduce.
 * Each fiber step installs its own scope, so the job's scope only avoids running in a closed one.
 */
const customScopeScheduler = (
  client: CustomInvocationContextClientShape,
  root: CustomScopeToken,
): Scheduler.Scheduler =>
  new Scheduler.MixedScheduler("async", (task) => {
    let cancelled = false
    client.run(root, () =>
      Promise.resolve().then(() => {
        if (!cancelled) task()
      }),
    )
    return () => {
      cancelled = true
    }
  })

/**
 * Runs a dispatcher entry with the scope hook and the runtime's current token as the root
 * scope of every fiber it starts.
 */
export const withCustomScopes = <A, E, R>(
  effect: Effect.Effect<A, E, R>,
): Effect.Effect<A, E, R | CustomInvocationContextClient> =>
  Effect.gen(function* () {
    const client = yield* CustomInvocationContextClient
    const tracer = yield* Effect.tracer
    const root = client.capture()
    return yield* effect.pipe(
      Effect.withTracer(composeTracer(tracer, client)),
      Effect.provideService(CurrentCustomScope, root),
      Effect.provideService(Scheduler.Scheduler, customScopeScheduler(client, root)),
    )
  })

/** Composes `tracer` with the scope hook of the current fiber's tracer, if it has one. */
export const composeWithCurrent = (tracer: Tracer.Tracer): Effect.Effect<Tracer.Tracer> =>
  Effect.map(Effect.tracer, (current) =>
    isComposed(current) ? composeTracer(tracer, current[scopeClient]) : tracer,
  )

export class CustomScopeHookMissingError extends Error {
  readonly _tag = "CustomScopeHookMissingError"
  constructor() {
    super(
      "Durability.wrap needs the Golem tracer scope hook; compose a replacement Tracer with Tracing.compose",
    )
  }
}

/** The current fiber's scope. Dies when the fiber runs without the scope hook. */
export const currentCustomScope: Effect.Effect<CustomScopeToken> = Effect.flatMap(
  Effect.tracer,
  (tracer) =>
    Effect.withFiber((fiber) => {
      const token = fiber.getRef(CurrentCustomScope)
      return token !== undefined && isComposed(tracer)
        ? Effect.succeed(token)
        : Effect.die(new CustomScopeHookMissingError())
    }),
)

const children = new WeakMap<CustomScopeToken, Set<Deferred.Deferred<void>>>()

/** Registers an open child operation of `parent`. */
export const registerChild = (parent: CustomScopeToken): Effect.Effect<Deferred.Deferred<void>> =>
  Effect.map(Deferred.make<void>(), (done) => {
    let open = children.get(parent)
    if (open === undefined) children.set(parent, (open = new Set()))
    open.add(done)
    return done
  })

export const releaseChild = (
  parent: CustomScopeToken,
  done: Deferred.Deferred<void>,
): Effect.Effect<void> =>
  Effect.suspend(() => {
    children.get(parent)?.delete(done)
    return Deferred.succeed(done, undefined)
  })

/** Waits until every child operation registered under `token` has closed. */
export const awaitChildren = (token: CustomScopeToken): Effect.Effect<void> =>
  Effect.suspend(() => {
    const next = children.get(token)?.values().next()
    return next === undefined || next.done === true
      ? Effect.void
      : Effect.andThen(Deferred.await(next.value), awaitChildren(token))
  })
