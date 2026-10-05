import { beforeEach, describe, expect, it } from "vitest"
import { Effect, Result, Stream } from "effect"
import { ToolTransport } from "../src/Tool.js"
import { ToolType } from "../src/ToolReflection.js"
import { compileDefinition, resetTools } from "../src/internal/tool/model.js"
import {
  artifactDefinition,
  GeneratedArtifactClient,
  installArtifactProvider,
  loopbackTransport,
  noHost,
  type ProviderObservation,
} from "./fixtures/gol-40-effect-conformance.js"
import { ToolClient } from "../src/host/ToolClient.js"

const principal = { tag: "oidc", val: { subject: "alice", claims: [] } } as const
const input = {
  region: "us-east-1" as const,
  trace: true,
  profile: "release" as const,
  request: {
    source: "src/main.wasm",
    labels: new Map([
      ["team", "runtime"],
      ["tier", "gold"],
    ]),
  },
  inputs: ["src/a.wasm", "src/b.wat"],
  format: "json" as const,
}
const stdin = () => Stream.succeed(new TextEncoder().encode("wasm"))
const text = (chunks: Iterable<Uint8Array>) =>
  new TextDecoder().decode(Uint8Array.from([...chunks].flatMap((chunk) => [...chunk])))

describe("GOL-40 Effect conformance fixture", () => {
  const observations: ProviderObservation[] = []
  const transport = loopbackTransport(principal)

  beforeEach(() => {
    resetTools()
    observations.length = 0
    installArtifactProvider(observations)
  })

  it("matches definition-owned and generated clients for asymmetric success and subcommands", async () => {
    let proxyStdout = ""
    let proxyStderr: Uint8Array[] = []
    const proxyResult = await Effect.runPromise(
      artifactDefinition
        .client({ transport })
        .render.execute(input, {
          stdin: stdin(),
          stdout: (stream) =>
            Stream.runCollect(stream).pipe(
              Effect.tap((chunks) => Effect.sync(() => (proxyStdout = text(chunks)))),
            ),
          stderr: (stream) =>
            Stream.runCollect(stream).pipe(
              Effect.tap((chunks) => Effect.sync(() => (proxyStderr = [...chunks]))),
            ),
        })
        .pipe(Effect.provideService(ToolClient, noHost)),
    )
    const generated = GeneratedArtifactClient.create()
    const generatedCall = await Effect.runPromise(
      generated
        .execute(input, stdin())
        .pipe(
          Effect.provideService(ToolTransport, transport),
          Effect.provideService(ToolClient, noHost),
          Effect.scoped,
        ),
    )
    const [generatedResult, generatedStdout, generatedStderr] = await Effect.runPromise(
      Effect.all(
        [
          generatedCall.result,
          Stream.runCollect(generatedCall.stdout!),
          Stream.runCollect(generatedCall.stderr!),
        ],
        { concurrency: "unbounded" },
      ),
    )
    const statusInput = {
      region: "eu-west-1" as const,
      trace: false,
      profile: "release" as const,
      artifactId: 9223372036854775809n,
    }
    const proxyStatus = await Effect.runPromise(
      artifactDefinition
        .client({ transport })
        .render.status(statusInput)
        .pipe(Effect.provideService(ToolClient, noHost)),
    )
    const generatedStatus = await Effect.runPromise(
      generated
        .status({ ...statusInput, "artifact-id": statusInput.artifactId })
        .pipe(
          Effect.provideService(ToolTransport, transport),
          Effect.provideService(ToolClient, noHost),
          Effect.scoped,
        ),
    )

    expect(generatedResult).toEqual(proxyResult)
    expect(text(generatedStdout)).toBe(proxyStdout)
    expect([...generatedStderr]).toEqual(proxyStderr)
    expect(generatedStatus).toBe(proxyStatus)
    expect(
      observations.map(({ path, principal, stdinClosed }) => ({ path, principal, stdinClosed })),
    ).toEqual([
      { path: "render/execute", principal, stdinClosed: true },
      { path: "render/execute", principal, stdinClosed: true },
      { path: "render/status", principal, stdinClosed: true },
      { path: "render/status", principal, stdinClosed: true },
    ])
  })

  it("matches generated and reflected typed errors, principal, streams, and cleanup", async () => {
    const invalid = { ...input, request: { ...input.request, source: "invalid" } }
    const generated = GeneratedArtifactClient.create()
    const generatedCall = await Effect.runPromise(
      generated
        .execute(invalid, stdin())
        .pipe(
          Effect.provideService(ToolTransport, transport),
          Effect.provideService(ToolClient, noHost),
          Effect.scoped,
        ),
    )
    const generatedExit = await Effect.runPromise(
      Effect.exit(
        Effect.all(
          [
            generatedCall.result,
            Stream.runCollect(generatedCall.stdout!),
            Stream.runCollect(generatedCall.stderr!),
          ],
          { concurrency: "unbounded" },
        ),
      ),
    )

    const registration = {
      lookupName: "artifact",
      definition: compileDefinition(artifactDefinition).wire,
      implementedBy: { uuid: { highBits: 0n, lowBits: 1n } },
    }
    const reflected = new ToolType(registration).client.command(["render", "execute"])
    const reflectedExit = await Effect.runPromise(
      Effect.exit(
        Effect.scoped(
          reflected
            .startJson(
              {
                ...invalid,
                request: {
                  ...invalid.request,
                  labels: [...invalid.request.labels],
                },
              },
              stdin(),
            )
            .pipe(
              Effect.flatMap((started) => started.collect),
              Effect.map(({ result }) => result.pipe(Result.getOrThrow)),
            ),
        ),
      ).pipe(
        Effect.provideService(ToolTransport, transport),
        Effect.provideService(ToolClient, noHost),
      ),
    )

    expect(generatedExit._tag).toBe("Failure")
    expect(reflectedExit._tag).toBe("Failure")
    expect(String(generatedExit)).toContain("invalid-request")
    expect(String(reflectedExit)).toContain("invalid-request")
    expect(observations).toHaveLength(2)
    expect(observations.every((entry) => entry.principal === principal && entry.stdinClosed)).toBe(
      true,
    )
  })
})
