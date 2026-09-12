import type * as Common from "golem:tool/common@0.1.0"
import type * as Host from "golem:tool/host@0.1.0"
import { Effect, Layer, Stream } from "effect"
import { HostLive } from "../../host/HostLive.js"
import { schemaShapesMatch } from "../schema-model/model.js"
import { validateSchemaGraph } from "../schema-model/validation.js"
import { schemaGraphFromWit } from "../schema-model/wit.js"
import { findCommand, implementationAt, registeredTools, ToolInvokeError } from "./model.js"

const asError = (cause: Common.ToolError) => new ToolInvokeError(cause)
const bytes = (source: AsyncIterator<Host.ByteStreamItem> | undefined) =>
  source === undefined
    ? undefined
    : Stream.fromAsyncIterable({ [Symbol.asyncIterator]: () => source }, (e) =>
        asError({ tag: "invalid-input", val: String(e) }),
      ).pipe(
        Stream.mapEffect((item) =>
          item.tag === "ok"
            ? Effect.succeed(item.val)
            : Effect.fail(asError({ tag: "invalid-input", val: `stdin ${item.val.tag}` })),
        ),
      )

export async function invokeRegistered(
  toolName: string,
  path: string[],
  input: Common.TypedSchemaValue,
  stdin: AsyncIterable<Host.ByteStreamItem> | undefined,
  stdout: Host.ToolStdoutWriter | undefined,
  principal: unknown,
): Promise<Common.InvocationResult> {
  const stdinIterator = stdin?.[Symbol.asyncIterator]()
  let stdoutCompleted = false
  const completeStdout = () => {
    if (!stdout || stdoutCompleted) return Promise.resolve()
    stdoutCompleted = true
    return stdout.finish()
  }
  const failStdout = (error: unknown) => {
    if (!stdout || stdoutCompleted) return Promise.resolve()
    stdoutCompleted = true
    return stdout.fail({ tag: "failed", val: String(error) })
  }
  try {
    const registered = registeredTools().find((x) => x.definition.name === toolName)
    if (!registered) throw { tag: "invalid-tool-name", val: toolName } satisfies Common.ToolError
    const body = findCommand(registered, path)
    if (!body) throw { tag: "invalid-command-path", val: path } satisfies Common.ToolError
    const handler = implementationAt(registered.implementation, toolName, path)
    if (!handler) throw { tag: "invalid-command-path", val: path } satisfies Common.ToolError

    try {
      const graph = schemaGraphFromWit(input.graph)
      const invalid = validateSchemaGraph(graph)[0]
      if (invalid) throw new Error(invalid.message)
      if (!schemaShapesMatch(graph, body.input.graph)) {
        throw new Error("tool input schema does not match the command canonical input schema")
      }
    } catch (error) {
      throw { tag: "invalid-input", val: String(error) } satisfies Common.ToolError
    }

    const encodeFailure = (failure: { readonly name: string; readonly value: unknown }) => {
      const declared = body.errors.find((x) => x.spec.name === failure.name)
      if (!declared)
        return Effect.fail(
          asError({ tag: "invalid-result", val: `undeclared error '${failure.name}'` }),
        )
      return Effect.flatMap(declared.codec.encodeAsync(failure.value), (value) =>
        Effect.fail(
          asError({
            tag: "custom-error",
            val: { graph: declared.codec.schemaGraph, value },
          }),
        ),
      )
    }
    const isFailure = (
      value: unknown,
    ): value is { _tag: "ToolFailure"; name: string; value: unknown } =>
      typeof value === "object" && value !== null && "_tag" in value && value._tag === "ToolFailure"
    const isSuccess = (value: unknown): value is { _tag: "ToolSuccess"; value: unknown } =>
      typeof value === "object" && value !== null && "_tag" in value && value._tag === "ToolSuccess"

    const program = Effect.gen(function* () {
      if (body.model.stdin?.required && !stdinIterator)
        return yield* Effect.fail(
          asError({
            tag: "invalid-input",
            val: "tool invocation did not contain declared stdin stream",
          }),
        )
      if (!body.model.stdin && stdinIterator)
        return yield* Effect.fail(
          asError({
            tag: "invalid-input",
            val: "tool invocation contained undeclared stdin stream",
          }),
        )
      if (body.model.stdout?.required && !stdout)
        return yield* Effect.fail(
          asError({
            tag: "invalid-input",
            val: "tool invocation did not contain declared stdout stream",
          }),
        )
      if (!body.model.stdout && stdout)
        return yield* Effect.fail(
          asError({
            tag: "invalid-input",
            val: "tool invocation contained undeclared stdout stream",
          }),
        )

      const decoded = yield* body.input
        .decode(input.value)
        .pipe(Effect.mapError((cause) => asError({ tag: "invalid-input", val: String(cause) })))
      const invocation = handler(decoded, {
        principal,
        stdin: bytes(stdinIterator),
        stdout:
          stdout &&
          ((source) =>
            Stream.runForEach(source, (chunk) =>
              stdoutCompleted
                ? Effect.fail(
                    asError({ tag: "invalid-result", val: "stdout is already completed" }),
                  )
                : Effect.tryPromise({
                    try: () => stdout.write(chunk),
                    catch: (e) => asError({ tag: "invalid-result", val: String(e) }),
                  }),
            )),
      }).pipe(Effect.catchIf(isFailure, encodeFailure))
      const result = yield* invocation
      if (isFailure(result)) return yield* encodeFailure(result)
      const value = isSuccess(result) ? result.value : result
      const encoded = body.output ? yield* body.output.encodeAsync(value) : undefined
      return {
        result:
          body.output && encoded ? { graph: body.output.schemaGraph, value: encoded } : undefined,
      } satisfies Common.InvocationResult
    })
    const runtimeLayer = registered.layer
      ? registered.layer.pipe(Layer.provideMerge(HostLive))
      : HostLive
    const result = await Effect.runPromise(
      Effect.scoped(Effect.provide(program, runtimeLayer)) as Effect.Effect<
        Common.InvocationResult,
        unknown,
        never
      >,
    )
    await completeStdout()
    return result
  } catch (error) {
    try {
      if (error instanceof ToolInvokeError || isToolError(error)) await completeStdout()
      else await failStdout(error)
    } catch {
      // The original invocation failure wins terminal arbitration.
    }
    if (error instanceof ToolInvokeError) throw error.cause
    throw error
  } finally {
    try {
      await stdinIterator?.return?.()
    } catch {
      // Input cleanup cannot replace the invocation result.
    }
  }
}

const isToolError = (value: unknown): value is Common.ToolError =>
  typeof value === "object" && value !== null && "tag" in value

export const toolGuest = {
  discoverTools: (): Common.Tool[] => registeredTools().map((x) => x.wire),
  getTool: (name: string): Common.Tool => {
    const value = registeredTools().find((x) => x.definition.name === name)
    if (!value) throw { tag: "invalid-tool-name", val: name } satisfies Common.ToolError
    return value.wire
  },
  invoke: invokeRegistered,
}
