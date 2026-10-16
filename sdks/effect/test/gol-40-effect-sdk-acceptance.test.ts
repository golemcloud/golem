import { beforeEach, describe, expect, it, vi } from "vitest"
import { Effect, Fiber, Result, Stream } from "effect"
import { ToolTransport } from "../src/Tool.js"
import { ToolType } from "../src/ToolReflection.js"
import { compileDefinition, resetTools } from "../src/internal/tool/model.js"
import { invokeRegistered } from "../src/internal/tool/runtime.js"
import { ToolClient } from "../src/host/ToolClient.js"
import {
  artifactDefinition,
  GeneratedArtifactClient,
  installArtifactProvider,
  loopbackTransport,
  noHost,
  type ProviderObservation,
} from "./fixtures/gol-40-effect-conformance.js"
import {
  emptyInput,
  lifecycleDefinition,
  lifecycleWriter,
  trackedBytes,
} from "./fixtures/gol-40-effect-sdk-acceptance.js"

const principal = { tag: "oidc", val: { subject: "effect-acceptance", claims: [] } } as const
const request = {
  region: "us-east-1",
  trace: true,
  profile: "release",
  request: {
    source: "src/acceptance.wasm",
    labels: new Map([
      ["suite", "gol-40"],
      ["sdk", "effect"],
    ]),
  },
  inputs: ["src/one.wasm", "src/two.wat"],
  format: "json",
} as const
const stdin = () => Stream.succeed(new TextEncoder().encode("effect"))

describe("GOL-40 Effect SDK acceptance", () => {
  const observations: ProviderObservation[] = []
  const transport = loopbackTransport(principal)

  beforeEach(() => {
    resetTools()
    observations.length = 0
  })

  it("SDK-PROXY preserves generated-client inputs, outputs, principal, and stream cleanup", async () => {
    installArtifactProvider(observations)
    const generated = GeneratedArtifactClient.create()
    const started = await Effect.runPromise(
      generated
        .execute(request as never, stdin())
        .pipe(
          Effect.provideService(ToolTransport, transport),
          Effect.provideService(ToolClient, noHost),
          Effect.scoped,
        ),
    )
    const [result, stdout, stderr] = await Effect.runPromise(
      Effect.all(
        [started.result, Stream.runCollect(started.stdout!), Stream.runCollect(started.stderr!)],
        { concurrency: "unbounded" },
      ),
    )

    expect(result).toEqual({
      artifactId: 18446744073709551614n,
      digest: "deadbeef",
      warnings: ["unsigned metadata"],
    })
    expect(
      new TextDecoder().decode(Uint8Array.from([...stdout].flatMap((chunk) => [...chunk]))),
    ).toBe("compiled:2:effect")
    expect([...stderr].flatMap((chunk) => [...chunk])).toEqual([0, 1, 2])
    expect(observations).toMatchObject([{ path: "render/execute", principal, stdinClosed: true }])
  })

  it("SDK-REFLECTION matches generated success, streams, principal, and cleanup", async () => {
    installArtifactProvider(observations)
    const generated = GeneratedArtifactClient.create()
    const generatedStarted = await Effect.runPromise(
      generated
        .execute(request as never, stdin())
        .pipe(
          Effect.provideService(ToolTransport, transport),
          Effect.provideService(ToolClient, noHost),
          Effect.scoped,
        ),
    )
    const generatedResult = await Effect.runPromise(
      Effect.all(
        [
          generatedStarted.result,
          Stream.runCollect(generatedStarted.stdout!),
          Stream.runCollect(generatedStarted.stderr!),
        ] as const,
        { concurrency: "unbounded" },
      ),
    )
    const registration = {
      lookupName: "artifact",
      definition: compileDefinition(artifactDefinition).wire,
      implementedBy: { uuid: { highBits: 0n, lowBits: 40n } },
    }
    const reflected = new ToolType(registration).client.command(["render", "execute"])
    const reflectedRequest = {
      ...request,
      request: { ...request.request, labels: [...request.request.labels] },
    }
    const reflectedResult = await Effect.runPromise(
      Effect.scoped(
        reflected
          .startJson(reflectedRequest as never, stdin())
          .pipe(Effect.flatMap((x) => x.collect)),
      ).pipe(
        Effect.provideService(ToolTransport, transport),
        Effect.provideService(ToolClient, noHost),
      ),
    ).then(
      (value) =>
        [
          value.result.pipe(Result.getOrThrow),
          value.stdout.pipe(Result.getOrThrow),
          value.stderr.pipe(Result.getOrThrow),
        ] as const,
    )

    expect(reflectedResult[0]).toEqual({
      artifactId: "18446744073709551614",
      digest: "deadbeef",
      warnings: ["unsigned metadata"],
    })
    expect(new TextDecoder().decode(reflectedResult[1])).toBe(
      new TextDecoder().decode(
        Uint8Array.from([...generatedResult[1]].flatMap((chunk) => [...chunk])),
      ),
    )
    expect([...reflectedResult[2]!]).toEqual([...generatedResult[2]].flatMap((chunk) => [...chunk]))
    expect(observations).toHaveLength(2)
    expect(observations.every((entry) => entry.principal === principal && entry.stdinClosed)).toBe(
      true,
    )
  })

  it.each([
    ["success", "finish", "success"],
    ["declared", "finish", "declared"],
    ["trap", "fail", "trap"],
  ] as const)(
    "SDK-TERMINALS settles both outputs and stdin for %s",
    async (scenario, terminal, outcome) => {
      lifecycleDefinition(scenario)
      const stdout = lifecycleWriter()
      const stderr = lifecycleWriter()
      const input = trackedBytes()
      const invocation = invokeRegistered(
        `gol40-lifecycle-${scenario}`,
        [],
        emptyInput(),
        input.iterable,
        stdout.writer as never,
        stderr.writer as never,
        principal,
      )
      if (outcome === "success") await expect(invocation).resolves.toHaveProperty("result")
      else if (outcome === "declared")
        await expect(invocation).rejects.toMatchObject({ tag: "custom-error" })
      else await expect(invocation).rejects.toThrow("provider trap")

      expect(stdout.events).toEqual(["write:1", terminal])
      expect(stderr.events).toEqual(["write:2", terminal])
      expect(input.returned.mock.calls.length).toBeGreaterThanOrEqual(1)
    },
  )

  it("SDK-TERMINALS separates result-observer interruption, reader cancellation, and invocation cancellation", async () => {
    const stdout = trackedBytes(1, 2)
    const cancel = vi.fn()
    const customTransport = {
      start: () =>
        Effect.succeed({
          result: Effect.never,
          stdout: stdout.iterable,
          stderr: undefined,
          cancel: Effect.sync(cancel),
        }),
    }
    await Effect.runPromise(
      Effect.scoped(
        Effect.gen(function* () {
          const started = yield* GeneratedArtifactClient.create().execute(request as never, stdin())
          const observer = yield* Effect.forkChild(started.result)
          yield* Fiber.interrupt(observer)
          expect(cancel).not.toHaveBeenCalled()
          yield* Stream.runDrain(started.stdout!.pipe(Stream.take(1)))
          expect(stdout.returned).toHaveBeenCalledOnce()
          expect(cancel).not.toHaveBeenCalled()
          yield* started.cancel
          expect(cancel).toHaveBeenCalledOnce()
        }),
      ).pipe(
        Effect.provideService(ToolTransport, customTransport as never),
        Effect.provideService(ToolClient, noHost),
      ),
    )
    expect(cancel).toHaveBeenCalledTimes(2)
  })

  it("SDK-TERMINALS leaves a dropped result observer pending while stdout completes", async () => {
    const stdout = trackedBytes(7, 8)
    const resultObserved = vi.fn()
    const cancel = vi.fn()
    const customTransport = {
      start: () =>
        Effect.succeed({
          result: Effect.sync(() => {
            resultObserved()
            return {}
          }),
          stdout: stdout.iterable,
          stderr: undefined,
          cancel: Effect.sync(cancel),
        }),
    }

    await Effect.runPromise(
      Effect.scoped(
        Effect.gen(function* () {
          const started = yield* GeneratedArtifactClient.create().execute(request as never, stdin())
          expect(yield* Stream.runCollect(started.stdout!)).toEqual([
            Uint8Array.of(7),
            Uint8Array.of(8),
          ])
          expect(resultObserved).not.toHaveBeenCalled()
          expect(cancel).not.toHaveBeenCalled()
        }),
      ).pipe(
        Effect.provideService(ToolTransport, customTransport as never),
        Effect.provideService(ToolClient, noHost),
      ),
    )

    expect(resultObserved).not.toHaveBeenCalled()
    expect(cancel).toHaveBeenCalledOnce()
    expect(stdout.returned.mock.calls.length).toBeGreaterThanOrEqual(1)
  })

  it("SDK-EFFECT-FINISH fails a successful invocation when implicit output finish fails", async () => {
    lifecycleDefinition("success")
    const stdout = lifecycleWriter(new Error("stdout finish failed"))
    const stderr = lifecycleWriter()

    await expect(
      invokeRegistered(
        "gol40-lifecycle-success",
        [],
        emptyInput(),
        trackedBytes().iterable,
        stdout.writer as never,
        stderr.writer as never,
        principal,
      ),
    ).rejects.toThrow("stdout finish failed")
    expect(stdout.events).toEqual(["write:1", "finish"])
    expect(stderr.events).toEqual(["write:2", "finish"])
  })

  it("SDK-EFFECT-FINISH preserves a declared error after attempting every output finish", async () => {
    lifecycleDefinition("declared")
    const stdout = lifecycleWriter(new Error("stdout finish failed"))
    const stderr = lifecycleWriter()
    await expect(
      invokeRegistered(
        "gol40-lifecycle-declared",
        [],
        emptyInput(),
        trackedBytes().iterable,
        stdout.writer as never,
        stderr.writer as never,
        principal,
      ),
    ).rejects.toMatchObject({ tag: "custom-error" })
    expect(stdout.events).toContain("finish")
    expect(stderr.events).toContain("finish")
  })
})
