/**
 * MySqlCounter case — drives the MySQL-backed counter agent through
 * the standard matrix + snapshot + update drill.
 */
import { defineCase } from "../harness/case.ts"
import { driveRdbmsCounter } from "./_counter-helper.ts"

export const case_ = defineCase(
  "mysql-counter",
  "MySqlCounter (MySQL-backed counter): matrix + snapshot + update drill",
  driveRdbmsCounter({ agentName: "MySqlCounter", slug: "mysql" }),
)
