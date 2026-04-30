/**
 * Test-case registry. Add a new agent case by importing it here.
 *
 * Order matters: cases run sequentially in the order listed below.
 * Long-running cases (RDBMS counters; the inventory-saga rewind drill
 * which deliberately loops for ~20s) come last.
 */
import type { TestCase } from "../harness/case.ts"
import { case_ as counter } from "./counter.ts"
import { case_ as caller } from "./caller.ts"
import { case_ as hostFeatures } from "./host-features.ts"
import { case_ as bookingSaga } from "./booking-saga.ts"
import { case_ as quota } from "./quota.ts"
import { case_ as kv } from "./kv.ts"
import { case_ as blob } from "./blob.ts"
import { case_ as webhook } from "./webhook.ts"
import { case_ as websocket } from "./websocket.ts"
import { case_ as sqliteCounter } from "./sqlite-counter.ts"
import { case_ as pgCounter } from "./pg-counter.ts"
import { case_ as mysqlCounter } from "./mysql-counter.ts"
import { case_ as igniteCounter } from "./ignite-counter.ts"
import { case_ as inventorySaga } from "./inventory-saga.ts"

export const allCases: ReadonlyArray<TestCase> = [
  counter,
  caller,
  hostFeatures,
  bookingSaga,
  quota,
  kv,
  blob,
  webhook,
  websocket,
  sqliteCounter,
  pgCounter,
  mysqlCounter,
  igniteCounter,
  inventorySaga,
]
