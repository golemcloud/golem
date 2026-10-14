/**
 * Layer-build-once probe. Establishes two architectural contracts:
 *
 * 1. **Per-call-site memoisation (Effect's default).** Inside a
 *    single `Effect.runPromise` call, `Effect.provide(eff, layer)`
 *    builds the layer at most once even if `eff` consumes the layer
 *    multiple times.
 *
 * 2. **Cross-dispatch sharing via `ManagedRuntime`.** The dispatcher
 *    in `src/agent.ts` backs its host-services layer with a
 *    module-level `ManagedRuntime`. Multiple `runtime.runPromise(eff)`
 *    calls share the same built `Context.Context`, so a `Layer.effect`
 *    (or `Layer.scoped`) acquire fires exactly once across every
 *    `dispatch*` entry.
 *
 * The second contract is what the `notes/layer-mocks-refactor-plan.md`
 * §8 question 1 mandates ("count ends at 1 across multiple
 * dispatches"). This file proves both contracts mechanically with a
 * counter `Ref`.
 */
import { describe, expect, it } from "@effect/vitest"
import { Context, Effect, Layer, ManagedRuntime, Ref } from "effect"

class Probe extends Context.Service<Probe, { readonly value: number }>()(
  "effect-golem-test/Probe",
) {}

const makeProbeLayer = (counter: Ref.Ref<number>): Layer.Layer<Probe> =>
  Layer.effect(
    Probe,
    Effect.gen(function* () {
      yield* Ref.update(counter, (n) => n + 1)
      return Probe.of({ value: 42 })
    }),
  )

describe("Host runtime — Layer-build-once probe", () => {
  it.effect(
    "per-call-site memoisation: many `yield* Probe` inside one `Effect.runPromise` build the layer ONCE",
    () =>
      Effect.gen(function* () {
        const counter = yield* Ref.make(0)
        const probeLayer = makeProbeLayer(counter)

        const program = Effect.gen(function* () {
          for (let i = 0; i < 3; i++) {
            const p = yield* Probe
            expect(p.value).toBe(42)
          }
        }).pipe(Effect.provide(probeLayer))

        yield* program

        const built = yield* Ref.get(counter)
        expect(built).toBe(1)
      }),
  )

  it.effect(
    "different `Effect.runPromise` calls each rebuild the layer (per-runPromise MemoMap)",
    () =>
      Effect.gen(function* () {
        const counter = yield* Ref.make(0)
        const probeLayer = makeProbeLayer(counter)

        const oneRun = Effect.gen(function* () {
          const p = yield* Probe
          return p.value
        }).pipe(Effect.provide(probeLayer))

        for (let i = 0; i < 3; i++) {
          const v = yield* Effect.promise(() => Effect.runPromise(oneRun))
          expect(v).toBe(42)
        }

        const built = yield* Ref.get(counter)
        // Confirms the per-runPromise rebuild, which is what
        // `ManagedRuntime` is meant to fix (next test).
        expect(built).toBe(3)
      }),
  )

  it.effect(
    "ManagedRuntime: 3 separate `runtime.runPromise` calls build the layer EXACTLY ONCE (the dispatcher contract)",
    () =>
      Effect.gen(function* () {
        const counter = yield* Ref.make(0)
        const probeLayer = makeProbeLayer(counter)
        const runtime = ManagedRuntime.make(probeLayer)

        const oneRun = Effect.gen(function* () {
          const p = yield* Probe
          return p.value
        })

        for (let i = 0; i < 3; i++) {
          const v = yield* Effect.promise(() => runtime.runPromise(oneRun))
          expect(v).toBe(42)
        }

        const built = yield* Ref.get(counter)
        expect(built).toBe(1)

        yield* Effect.promise(() => runtime.dispose())
      }),
  )
})
