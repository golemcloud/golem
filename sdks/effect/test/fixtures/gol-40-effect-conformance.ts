import type * as Host from "golem:tool/host@0.1.0"
import type * as Streams from "golem:tool/streams@0.1.0"
import { Effect, Schema, Stream } from "effect"
import * as BridgeTool from "../../src/BridgeTool.js"
import { ToolClient } from "../../src/host/ToolClient.js"
import * as ToolSchema from "../../src/Schema.js"
import {
  compileDefinition,
  err,
  toolDefinition,
  type ToolTransport,
  type TransportInvocation,
} from "../../src/Tool.js"
import { invokeRegistered } from "../../src/internal/tool/runtime.js"
import {
  schemaGraphFromWit,
  schemaValueFromWit,
  schemaValueToWit,
} from "../../src/internal/schema-model/wit.js"
import { compile, type CompiledWitCodec } from "../../src/WitCodec.js"
import { Uint64 } from "../../src/WitTypes.js"

const Request = Schema.Struct({
  source: Schema.String,
  labels: ToolSchema.Map(Schema.String, Schema.String),
})
const Report = Schema.Struct({
  artifactId: Uint64,
  digest: Schema.String,
  warnings: Schema.Array(Schema.String),
})
const ValidationFailure = Schema.Struct({
  field: Schema.String,
  reason: Schema.String,
  retryable: Schema.Boolean,
})

export const artifactDefinition = toolDefinition("artifact").command(
  "render",
  (render) =>
    render
      .command("execute", (execute) =>
        execute.body((body) =>
          body
            .option("region", Schema.String, { default: "eu-west-1" })
            .flag("trace")
            .option("profile", Schema.String, { default: "release" })
            .positional("request", Request)
            .tail("inputs", Schema.String, { min: 1, max: 3 })
            .option("format", Schema.String, { default: "json" })
            .input()
            .output({ required: true })
            .stderr()
            .returns(Report)
            .error("invalid-request", ValidationFailure),
        ),
      )
      .command("status", (status) =>
        status.body((body) =>
          body
            .option("region", Schema.String, { default: "eu-west-1" })
            .flag("trace")
            .option("profile", Schema.String, { default: "release" })
            .positional("artifact-id", Uint64)
            .returns(Schema.String),
        ),
      ),
  { aliases: ["build"] },
)

export interface ProviderObservation {
  readonly path: string
  readonly principal: unknown
  readonly input: unknown
  readonly stdinClosed: boolean
}

export const installArtifactProvider = (observations: ProviderObservation[]) =>
  artifactDefinition.implement({
    render: {
      execute: (input, context) =>
        Effect.gen(function* () {
          let stdinClosed = false
          const stdin = context.stdin
            ? context.stdin.pipe(Stream.ensuring(Effect.sync(() => (stdinClosed = true))))
            : Stream.empty
          const chunks = yield* Stream.runCollect(stdin)
          const payload = new TextDecoder().decode(
            Uint8Array.from(chunks.flatMap((chunk) => [...chunk])),
          )
          yield* context.stdout!(
            Stream.succeed(new TextEncoder().encode(`compiled:${input.inputs.length}:${payload}`)),
          )
          yield* context.stderr!(Stream.succeed(Uint8Array.of(0, 1, 2)))
          observations.push({
            path: "render/execute",
            principal: context.principal,
            input,
            stdinClosed,
          })
          if (input.request.source === "invalid")
            return yield* Effect.fail(
              err("invalid-request", {
                field: "request.source",
                reason: "unsupported module",
                retryable: false,
              }),
            )
          return {
            artifactId: 18446744073709551614n,
            digest: "deadbeef",
            warnings: ["unsigned metadata"],
          }
        }),
      status: ({ "artifact-id": artifactId }, context) =>
        Effect.sync(() => {
          observations.push({
            path: "render/status",
            principal: context.principal,
            input: { artifactId },
            stdinClosed: true,
          })
          return artifactId === 9223372036854775809n ? ("ready" as const) : ("failed" as const)
        }),
    },
  })

type Channel = {
  readonly writer: Pick<Streams.ToolOutputWriter, "write" | "finish" | "fail">
  readonly output: AsyncIterable<Host.ByteStreamItem>
}

const channel = (): Channel => {
  const items: Host.ByteStreamItem[] = []
  let terminal: Host.ByteStreamItem | undefined
  let done = false
  let wake: (() => void) | undefined
  const signal = () => {
    wake?.()
    wake = undefined
  }
  const output: AsyncIterable<Host.ByteStreamItem> = {
    [Symbol.asyncIterator]: async function* () {
      while (true) {
        if (items.length > 0) {
          yield items.shift()!
        } else if (done) {
          if (terminal) yield terminal
          return
        } else {
          await new Promise<void>((resolve) => (wake = resolve))
        }
      }
    },
  }
  return {
    output,
    writer: {
      write: async (bytes) => {
        items.push({ tag: "ok", val: bytes })
        signal()
      },
      finish: async () => {
        done = true
        signal()
      },
      fail: async (failure) => {
        terminal = { tag: "err", val: failure }
        done = true
        signal()
      },
    },
  }
}

export const loopbackTransport = (principal: unknown): ToolTransport => ({
  start: (tool, path, input, stdin, withStdout, withStderr) =>
    Effect.sync(() => {
      const stdout = withStdout ? channel() : undefined
      const stderr = withStderr ? channel() : undefined
      return {
        result: Effect.tryPromise({
          try: () =>
            invokeRegistered(
              tool,
              [...path],
              input,
              stdin,
              stdout?.writer as Streams.ToolOutputWriter | undefined,
              stderr?.writer as Streams.ToolOutputWriter | undefined,
              principal,
            ),
          catch: (error) => error,
        }),
        stdout: stdout?.output,
        stderr: stderr?.output,
        cancel: Effect.void,
      } satisfies TransportInvocation
    }),
})

const _executeInput = Schema.Struct({
  region: Schema.String,
  trace: Schema.Boolean,
  profile: Schema.String,
  request: Request,
  inputs: Schema.Array(Schema.String),
  format: Schema.String,
})
const _statusInput = Schema.Struct({
  region: Schema.String,
  trace: Schema.Boolean,
  profile: Schema.String,
  "artifact-id": Uint64,
})

const compiledDefinition = compileDefinition(artifactDefinition)
const executeCodec = compiledDefinition.bodies.get("render/execute")!.input
const statusCodec = compiledDefinition.bodies.get("render/status")!.input
const reportCodec = Effect.runSync(compile(Report))
const statusResultCodec = Effect.runSync(compile(Schema.String))
const failureCodec = Effect.runSync(compile(ValidationFailure))

const bridgeValue = async (codec: CompiledWitCodec<any>, value: unknown) => ({
  graph: schemaGraphFromWit(codec.schemaGraph),
  value: schemaValueFromWit(await Effect.runPromise(codec.encodeAsync(value as never) as never)),
})

export class GeneratedArtifactClient {
  static create(runtime = BridgeTool.createToolClientRuntime("artifact")) {
    return new GeneratedArtifactClient(runtime)
  }

  constructor(private readonly runtime: BridgeTool.ToolClientRuntime) {}

  execute(input: Schema.Schema.Type<typeof _executeInput>, stdin: Stream.Stream<Uint8Array>) {
    const runtime = this.runtime
    return Effect.gen(function* () {
      const started = yield* runtime.start<{ name: "invalid-request"; value: unknown }>(
        ["render", "execute"],
        yield* Effect.promise(() => bridgeValue(executeCodec, input)),
        stdin,
        true,
        true,
      )
      const result = started.result.pipe(
        Effect.flatMap(
          (value): Effect.Effect<unknown, unknown> =>
            value.result
              ? reportCodec.decode(value.result.value)
              : Effect.fail({
                  tag: "rpc",
                  error: { tag: "protocol-error", val: "missing result" },
                }),
        ),
        Effect.catch((error: any) => {
          if (error.tag !== "rpc") return Effect.fail(error)
          try {
            return Effect.fail(
              BridgeTool.splitToolRpcError(error.error, (name, payload) => ({
                name: name as "invalid-request",
                value: Effect.runSync(failureCodec.decode(schemaValueToWit(payload.value))),
              })),
            )
          } catch (decodeError) {
            return Effect.fail(decodeError as BridgeTool.ToolRuntimeError<never>)
          }
        }),
      )
      return BridgeTool.startedToolInvocation(
        started.stdout,
        started.stderr,
        result as Effect.Effect<
          unknown,
          BridgeTool.ToolRuntimeError<{ name: "invalid-request"; value: unknown }>
        >,
        started.cancel,
      )
    })
  }

  status(input: Schema.Schema.Type<typeof _statusInput>) {
    const runtime = this.runtime
    return Effect.gen(function* () {
      const started = yield* runtime.start(
        ["render", "status"],
        yield* Effect.promise(() => bridgeValue(statusCodec, input)),
        undefined,
        false,
        false,
      )
      const result = yield* started.result
      if (!result.result) return yield* Effect.die("missing status result")
      return yield* statusResultCodec.decode(result.result.value)
    })
  }
}

export const noHost = ToolClient.of({} as never)
