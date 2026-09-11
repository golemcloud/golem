/**
 * PgCounter case — drives the Postgres-backed counter agent through
 * the standard matrix + snapshot + update drill.
 */
import { defineCase } from "../harness/case.ts"
import { driveRdbmsCounter } from "./_counter-helper.ts"

export const case_ = defineCase(
  "pg-counter",
  "PgCounter (Postgres-backed counter): matrix + snapshot + update drill",
  driveRdbmsCounter({ agentName: "PgCounter", slug: "pg" }),
)
