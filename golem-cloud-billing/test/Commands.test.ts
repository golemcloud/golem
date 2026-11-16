import { ConfigProvider, Effect, Exit, Logger } from "effect"
import { describe, expect, it } from "vitest"
import * as Commands from "../src/Commands.ts"

describe("push-usage", () => {
  it("logs the missing setting before it fails, without the secrets", async () => {
    const lines: Array<string> = []
    const env = {
      GOLEM__DB__CONFIG__DATABASE: "golem",
      GOLEM__DB__CONFIG__USERNAME: "golem",
      GOLEM__DB__CONFIG__PASSWORD: "database-password-for-no-log",
      POLAR_API_URL: "https://sandbox-api.polar.test",
      POLAR_ACCESS_TOKEN: "polar-token-for-no-log"
    }

    const exit = await Effect.runPromiseExit(
      Commands.pushUsage.pipe(
        Effect.provide(ConfigProvider.layer(ConfigProvider.fromEnvRecord(env))),
        Effect.provide(Logger.layer([Logger.map(Logger.formatJson, (line) => void lines.push(line))]))
      )
    )

    expect(Exit.isFailure(exit)).toBe(true)
    expect(lines.map((line) => JSON.parse(line))).toEqual([
      expect.objectContaining({
        level: "ERROR",
        message: "The usage push failed",
        annotations: { error: expect.stringContaining("GOLEM__DB__CONFIG__HOST") }
      })
    ])
    expect(lines.join("\n")).not.toContain("for-no-log")
  })
})

describe("migrate", () => {
  it("logs the missing setting before it fails, without the secrets", async () => {
    const lines: Array<string> = []
    const env = {
      GOLEM__DB__CONFIG__DATABASE: "golem",
      GOLEM__DB__CONFIG__USERNAME: "golem_cloud_billing",
      GOLEM__DB__CONFIG__PASSWORD: "database-password-for-no-log"
    }

    const exit = await Effect.runPromiseExit(
      Commands.migrate.pipe(
        Effect.provide(ConfigProvider.layer(ConfigProvider.fromEnvRecord(env))),
        Effect.provide(Logger.layer([Logger.map(Logger.formatJson, (line) => void lines.push(line))]))
      )
    )

    expect(Exit.isFailure(exit)).toBe(true)
    expect(lines.map((line) => JSON.parse(line))).toEqual([
      expect.objectContaining({
        level: "ERROR",
        message: "The database migrations failed",
        annotations: { error: expect.stringContaining("GOLEM__DB__CONFIG__HOST") }
      })
    ])
    expect(lines.join("\n")).not.toContain("for-no-log")
  })
})

describe("cleanup-deleted-accounts", () => {
  it("logs the missing setting before it fails, without the secrets", async () => {
    const lines: Array<string> = []
    const env = {
      GOLEM__DB__CONFIG__HOST: "localhost",
      GOLEM__DB__CONFIG__DATABASE: "golem",
      GOLEM__DB__CONFIG__USERNAME: "golem",
      GOLEM__DB__CONFIG__PASSWORD: "database-password-for-no-log",
      POLAR_ACCESS_TOKEN: "polar-token-for-no-log"
    }

    const exit = await Effect.runPromiseExit(
      Commands.cleanupDeletedAccounts.pipe(
        Effect.provide(ConfigProvider.layer(ConfigProvider.fromEnvRecord(env))),
        Effect.provide(Logger.layer([Logger.map(Logger.formatJson, (line) => void lines.push(line))]))
      )
    )

    expect(Exit.isFailure(exit)).toBe(true)
    expect(lines.map((line) => JSON.parse(line))).toEqual([
      expect.objectContaining({
        level: "ERROR",
        message: "The cleanup of deleted accounts failed",
        annotations: { error: expect.stringContaining("POLAR_API_URL") }
      })
    ])
    expect(lines.join("\n")).not.toContain("for-no-log")
  })
})
