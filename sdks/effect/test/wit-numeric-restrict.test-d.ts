/**
 * Type-level contract for `WitTypes.restrict`: it accepts ONLY numeric schemas
 * (`number` / `bigint`) — applying it to a String / Boolean / record schema is a
 * compile error.
 *
 * Negative cases use `// @ts-expect-error` so a regression — i.e. `restrict`
 * silently accepting a non-numeric schema again — surfaces as
 * `TS2578: Unused '@ts-expect-error' directive`.
 *
 * Checked by `npm run typecheck` (tsc over `test/**`), not by `vitest run`.
 */
import { Schema } from "effect"
import { restrict, Uint8, Uint64 } from "../src/WitTypes.js"

// OK — numeric pins and a plain numeric schema.
Uint8.pipe(restrict({ min: 0, max: 200 }))
Uint64.pipe(restrict({ min: 0n, max: 10n }))
Schema.Number.pipe(restrict({ max: 9 }))
Schema.BigInt.pipe(restrict({ min: 1n }))

// @ts-expect-error restrict is numeric-only — String is rejected
Schema.String.pipe(restrict({ min: 1 }))
// @ts-expect-error restrict is numeric-only — Boolean is rejected
Schema.Boolean.pipe(restrict({ max: 1 }))
// @ts-expect-error restrict is numeric-only — a record schema is rejected
Schema.Struct({ x: Schema.Number }).pipe(restrict({ min: 0 }))
