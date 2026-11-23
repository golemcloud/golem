#!/usr/bin/env bun
import { Schema } from "effect"
import { accessSync, constants, readFileSync, realpathSync, statSync } from "node:fs"
import { join } from "node:path"
import { decode, uniqueJson, ValidationError } from "./tooling-io.ts"

export const targets = {
  lib: { name: "golem_worker_executor", kind: ["lib"], source: "golem-worker-executor/src/lib.rs" },
  integration: { name: "integration", kind: ["test"], source: "golem-worker-executor-tests/tests/lib.rs" }
} as const
const Artifact = Schema.Struct({
  reason: Schema.optionalKey(Schema.String),
  profile: Schema.optionalKey(Schema.Struct({ test: Schema.optionalKey(Schema.Boolean) })),
  target: Schema.optionalKey(
    Schema.Struct({
      name: Schema.optionalKey(Schema.String),
      kind: Schema.optionalKey(Schema.Array(Schema.String)),
      src_path: Schema.optionalKey(Schema.String)
    })
  ),
  executable: Schema.optionalKey(Schema.NullOr(Schema.String))
})
type Artifact = typeof Artifact.Type
const invalid = (message: string) => new ValidationError({ message })
const isLabel = (label: string): label is keyof typeof targets => Object.hasOwn(targets, label)

// Cargo's emitted target identity is authoritative, never the age or name of a file in deps.
export function selectExecutables(
  input: readonly unknown[],
  root: string,
  requested: string[]
): Record<string, string> {
  if (!requested.length || new Set(requested).size !== requested.length) {
    throw invalid("request each test target exactly once")
  }
  const messages = input.map((message) => decode(Artifact, message, "invalid Cargo message"))
  return Object.fromEntries(requested.map((label) => {
    if (!isLabel(label)) throw invalid("unknown test target; expected lib or integration")
    const target = targets[label]
    const matches = new Set(
      messages.filter((message) => {
        if (
          message.reason !== "compiler-artifact" || !message.profile?.test || !message.executable ||
          message.target?.name !== target.name || JSON.stringify(message.target.kind) !== JSON.stringify(target.kind)
        ) return false
        try {
          return realpathSync(message.target.src_path ?? "") === join(root, target.source)
        } catch {
          return false
        }
      }).map((message) => message.executable!)
    )
    if (matches.size !== 1) {
      throw invalid(`expected exactly one ${label} test executable from Cargo, got ${matches.size}`)
    }
    return [label, [...matches][0]!]
  }))
}

export function readMessages(path: string): Artifact[] {
  return readFileSync(path, "utf8").split(/\r?\n/).flatMap((line, index) => {
    if (!line.trim()) return []
    try {
      return [decode(Artifact, uniqueJson(line), "invalid Cargo message")]
    } catch {
      throw invalid(`invalid Cargo JSON message on line ${index + 1}`)
    }
  })
}

export function usableExecutables(messages: string, root: string, requested: string[]): Record<string, string> {
  const selected = selectExecutables(readMessages(messages), realpathSync(root), requested)
  for (const [label, executable] of Object.entries(selected)) {
    try {
      if (!statSync(executable).isFile()) throw invalid("not a file")
      accessSync(executable, constants.X_OK)
    } catch {
      throw invalid(`Cargo emitted a missing or non-executable ${label} test artifact`)
    }
  }
  return selected
}

if (import.meta.main) {
  try {
    const [messages, root, ...requested] = process.argv.slice(2)
    if (!messages || !root || !requested.length) {
      throw invalid("usage: select-test-executables.ts <cargo-json-messages> <repository-root> <lib|integration> ...")
    }
    console.log(
      Object.entries(usableExecutables(messages, root, requested)).map(([label, executable]) =>
        `${label}\t${executable}`
      ).join("\n")
    )
  } catch (error) {
    console.error(error instanceof ValidationError ? error.message : "Unable to read Cargo artifacts")
    process.exitCode = 1
  }
}
