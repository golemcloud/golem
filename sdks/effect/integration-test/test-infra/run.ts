/**
 * Entry point for `npm run test:integration`.
 *
 * Wires up the harness Layers (compose, golem server, deployment) and
 * runs the registered test cases. Layers are scoped: failing OR
 * succeeding, the lifecycle is torn down deterministically (compose
 * down, golem server kill).
 *
 * CLI flags (parsed from process.argv):
 *
 *   --list                      Print the case registry and exit.
 *   --filter <pattern>          Regex applied to case names.
 *   --no-build                  Skip `golem build && deploy`.
 *   --no-infra                  Skip docker compose lifecycle.
 *   --no-server                 Skip the golem server lifecycle (assume external).
 *   --stop-on-first-failure     Bail out on the first case failure.
 *
 * Exit code: 0 if every case passed; 1 otherwise.
 */
import { NodeRuntime, NodeServices } from "@effect/platform-node"
import { Console, Effect, Layer } from "effect"
import * as path from "node:path"
import * as url from "node:url"
import * as Compose from "./harness/compose.ts"
import * as Deployment from "./harness/deployment.ts"
import * as GolemCliMod from "./harness/golem-cli.ts"
import * as Runner from "./harness/runner.ts"
import * as Server from "./harness/server.ts"
import { allCases } from "./cases/index.ts"

const here = path.dirname(url.fileURLToPath(import.meta.url))
const integrationRoot = path.resolve(here, "..")

interface ParsedArgs {
  list: boolean
  filter?: RegExp
  noBuild: boolean
  noInfra: boolean
  noServer: boolean
  stopOnFirstFailure: boolean
}

const parseArgs = (argv: ReadonlyArray<string>): ParsedArgs => {
  const args: ParsedArgs = {
    list: false,
    noBuild: false,
    noInfra: false,
    noServer: false,
    stopOnFirstFailure: false,
  }
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i]
    switch (a) {
      case "--list":
        args.list = true
        break
      case "--filter":
        i++
        args.filter = new RegExp(argv[i] ?? "")
        break
      case "--no-build":
        args.noBuild = true
        break
      case "--no-infra":
        args.noInfra = true
        break
      case "--no-server":
        args.noServer = true
        break
      case "--stop-on-first-failure":
        args.stopOnFirstFailure = true
        break
      default:
        if (a !== undefined) {
          process.stderr.write(`Unknown argument: ${a}\n`)
          process.exit(2)
        }
    }
  }
  return args
}

const main = Effect.gen(function* () {
  const args = parseArgs(process.argv.slice(2))

  if (args.list) {
    yield* Console.log("Available test cases:")
    for (const c of allCases) {
      yield* Console.log(`  ${c.name.padEnd(20)} ${c.description}`)
    }
    return
  }

  // Build the layer composition. NodeServices supplies
  // ChildProcessSpawner / FileSystem / Path; everything below piles on
  // top of it.
  const cliLayer = GolemCliMod.layerWithCwd(integrationRoot)

  // Optional infra layers: pass `Layer.empty` when skipped so the
  // composition still typechecks.
  const composeLayer = args.noInfra
    ? Layer.empty
    : (Compose.layer as Layer.Layer<never, never, never>).pipe(
        // Compose.layer's output is `never`; merge it into the suite
        // and let its tear-down run on scope close.
        (l) => l,
      )
  const serverLayer = args.noServer
    ? Layer.empty
    : (Server.layer as unknown as Layer.Layer<unknown, unknown, never>)

  const baseInfra = Layer.mergeAll(cliLayer, composeLayer, serverLayer).pipe(
    Layer.provide(NodeServices.layer),
  ) as unknown as Layer.Layer<unknown, unknown, never>

  const deployLayer = args.noBuild
    ? (Deployment.layerSkip as unknown as Layer.Layer<unknown, unknown, never>)
    : (Deployment.layer as unknown as Layer.Layer<unknown, unknown, never>)

  // `Layer.provideMerge(that)` semantics: self.requirements come from
  // that.outputs; result outputs = self.A | that.A. So the consumer
  // (deployLayer needs GolemCli) must be `self`, and the provider
  // (baseInfra) must be `that`. Reversing this swallows GolemCli into
  // the requirements set and gives "Service not found: GolemCli" at
  // runtime.
  const fullLayer = deployLayer.pipe(Layer.provideMerge(baseInfra)) as Layer.Layer<
    unknown,
    unknown,
    never
  >

  const program = Runner.runCases(allCases, {
    filter: args.filter,
    stopOnFirstFailure: args.stopOnFirstFailure,
  }).pipe(
    Effect.flatMap((outcomes) =>
      Effect.gen(function* () {
        const summary = yield* Runner.summarize(outcomes)
        yield* Console.log(
          `\n=== ${summary.passed}/${summary.total} passed (${summary.failed} failed) ===`,
        )
        if (summary.failed > 0) {
          process.exitCode = 1
        }
      }),
    ),
  )

  yield* (program as Effect.Effect<unknown, unknown, unknown>).pipe(Effect.provide(fullLayer))
})

NodeRuntime.runMain(main as Effect.Effect<unknown, unknown>)
