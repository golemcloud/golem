/**
 * IgniteCounter case — drives the Apache-Ignite-backed counter agent
 * through the standard matrix + snapshot + update drill.
 */
import { defineCase } from "../harness/case.ts"
import { driveRdbmsCounter } from "./_counter-helper.ts"

export const case_ = defineCase(
  "ignite-counter",
  "IgniteCounter (Apache Ignite-backed counter): matrix + snapshot + update drill",
  driveRdbmsCounter({
    agentName: "IgniteCounter",
    slug: "ignite",
    // Apache Ignite's H2 SQL engine does not roll back DML inside
    // transactions, so failingAdd(100) leaves the counter at 110
    // instead of 10.
    afterMatrixValuePattern: /\b(8|10|110)\b/,
  }),
)
