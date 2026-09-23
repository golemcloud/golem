import { it } from "@effect/vitest"
import { Effect } from "effect"
import * as fc from "fast-check"

type Arbitraries = Record<string, fc.Arbitrary<unknown>>

type Values<Arbs extends Arbitraries> = {
  [K in keyof Arbs]: Arbs[K] extends fc.Arbitrary<infer A> ? A : never
}

const record = <const Arbs extends Arbitraries>(arbitraries: Arbs): fc.Arbitrary<Values<Arbs>> =>
  fc.record(arbitraries) as fc.Arbitrary<Values<Arbs>>

export const prop = <const Arbs extends Arbitraries>(
  name: string,
  arbitraries: Arbs,
  self: (values: Values<Arbs>) => boolean | void,
): void => {
  it(name, () => fc.assert(fc.property(record(arbitraries), self)))
}

export const effectProp = <const Arbs extends Arbitraries, A, E>(
  name: string,
  arbitraries: Arbs,
  self: (values: Values<Arbs>) => Effect.Effect<A, E>,
): void => {
  it(name, () =>
    fc.assert(
      fc.asyncProperty(record(arbitraries), (values) =>
        Effect.runPromise(self(values)).then((value) => (value as unknown) !== false),
      ),
    ),
  )
}
