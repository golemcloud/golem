/**
 * SqliteCounter case — exercises the `node:sqlite` + multipart
 * snapshot envelope path. Asserts that the SNAPSHOT entry appears in
 * the oplog (proves multipart/mixed envelope was written) and that
 * state survives a `golem agent update --await`.
 */
import { defineCase } from "../harness/case.ts"
import { driveSqliteCounter } from "./_counter-helper.ts"

export const case_ = defineCase(
  "sqlite-counter",
  "SqliteCounter (node:sqlite + multipart/mixed snapshot envelope): basic + snapshot drill",
  driveSqliteCounter(),
)
