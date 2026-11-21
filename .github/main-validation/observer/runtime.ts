import { Effect, Schema } from "effect"
import { randomUUID } from "node:crypto"
import * as fs from "node:fs/promises"
import { dirname, resolve } from "node:path"

/** Only these deliberately authored messages may cross the CLI's secret-safe error boundary. */
export class OperatorError extends Error {
  readonly _tag = "OperatorError"
}
export class OperationError
  extends Schema.TaggedError<OperationError>()("OperationError", { message: Schema.String, cause: Schema.Defect() })
{}
export const attempt = <A>(operation: () => A) =>
  Effect.try({
    try: operation,
    catch: (cause) =>
      cause instanceof OperatorError ? cause : new OperationError({ message: "Observer validation failed", cause })
  })
export const io = <A>(operation: () => Promise<A>) =>
  Effect.tryPromise({ try: operation, catch: (cause) => new OperationError({ message: "Observer I/O failed", cause }) })
export const parseJson = (body: string) => attempt((): unknown => JSON.parse(body))
export const readJson = Effect.fnUntraced(function*(path: string) {
  return yield* parseJson(yield* io(() => fs.readFile(path, "utf8")))
})
export const ObjectSchema = Schema.Record(Schema.String, Schema.Unknown)
const decodeObject = Schema.decodeUnknownSync(ObjectSchema)
export function object(value: unknown): Record<string, unknown> {
  if (value == null) return {}
  return decodeObject(value)
}
export const errorCode = (error: unknown): unknown =>
  error instanceof OperationError
    ? errorCode(error.cause)
    : typeof error === "object" && error !== null && "code" in error
    ? error.code
    : undefined
export const errorName = (error: Error): string =>
  error instanceof OperationError && error.cause instanceof Error ? errorName(error.cause) : error.name
export function array(value: unknown): unknown[] {
  if (value == null) return []
  if (!Array.isArray(value)) throw new Error("expected array")
  return value
}
export const objects = (value: unknown) => array(value).map(object)
export const text = (value: unknown, limit = 4000) => String(value ?? "").slice(0, limit)
export const sha = (value: unknown): value is string =>
  typeof value === "string" && value.length === 40 && /^[0-9a-f]{40}$/.test(value)
export const now = () => new Date().toISOString()
export const stateDirectory = () =>
  process.env.OBSERVER_STATE_DIR || resolve(import.meta.dir, "../../../.local/ci-observer")
export const exists = (path: string) =>
  io(async () => {
    try {
      await fs.stat(path)
      return true
    } catch (error) {
      if (errorCode(error) === "ENOENT") return false
      throw error
    }
  })
export const atomicJson = Effect.fnUntraced(function*(path: string, data: unknown, mode = 0o600) {
  yield* io(() => fs.mkdir(dirname(path), { recursive: true }))
  const temporary = `${path}.${randomUUID()}.tmp`
  const handle = yield* Effect.acquireRelease(
    io(() => fs.open(temporary, "wx", mode)),
    (handle) =>
      io(() => handle.close()).pipe(
        Effect.ensuring(io(() => fs.rm(temporary, { force: true })).pipe(Effect.orDie)),
        Effect.orDie
      )
  )
  yield* io(async () => {
    await handle.writeFile(JSON.stringify(data))
    await handle.sync()
    await fs.rename(temporary, path)
  })
}, Effect.scoped)

export const command = Effect.fnUntraced(
  function*(
    args: string[],
    options: {
      cwd?: string | undefined
      input?: string | undefined
      timeout?: number | undefined
      env?: NodeJS.ProcessEnv | undefined
    } = {}
  ) {
    const child = yield* Effect.acquireRelease(
      attempt(() =>
        Bun.spawn(args, {
          ...(options.cwd === undefined ? {} : { cwd: options.cwd }),
          ...(options.env === undefined ? {} : { env: options.env }),
          stdin: options.input === undefined ? "ignore" : new TextEncoder().encode(options.input),
          stdout: "pipe",
          stderr: "pipe"
        })
      ),
      (child) =>
        io(async () => {
          if (child.exitCode === null) child.kill("SIGKILL")
          await child.exited
        }).pipe(Effect.orDie)
    )
    return yield* io(async () => {
      const [stdout, , code] = await Promise.all([
        new Response(child.stdout).text(),
        new Response(child.stderr).text(),
        child.exited
      ])
      return { stdout, code }
    }).pipe(Effect.timeout(`${options.timeout ?? 30} seconds`))
  },
  Effect.scoped
)
export const checkedCommand = (args: string[], options: Parameters<typeof command>[1] = {}) =>
  command(args, options).pipe(
    Effect.flatMap((result) =>
      result.code === 0
        ? Effect.succeed(result.stdout)
        : Effect.fail(
          new OperationError({ message: "Observer command failed", cause: { command: args[0], code: result.code } })
        )
    )
  )

export type Send = (
  url: string,
  init?: RequestInit,
  seconds?: number
) => Effect.Effect<{ status: number; body: string }, Error>
export const request = (url: string, init: RequestInit = {}, seconds = 30) =>
  io(async () => {
    const response = await fetch(url, { ...init, redirect: "error", signal: AbortSignal.timeout(seconds * 1000) })
    return { status: response.status, body: await response.text() }
  })
export function publicOrigin(value: string): string {
  let url: URL
  try {
    url = new URL(value)
  } catch {
    throw new OperatorError("CI_DASHBOARD_URL must be configured as a public HTTPS URL")
  }
  if (
    url.protocol !== "https:" || !url.hostname || url.username || url.password || url.search || url.hash ||
    /\s/.test(value)
  ) throw new OperatorError("CI_DASHBOARD_URL must be an HTTPS URL without credentials or query parameters")
  return value.replace(/\/+$/, "")
}
export function rejectCredentials(value: unknown, secrets: (string | undefined)[]) {
  const body = JSON.stringify(value)
  if (secrets.some((secret) => secret && body.includes(JSON.stringify(secret).slice(1, -1)))) {
    throw new OperatorError("refusing to publish a credential in dashboard evidence")
  }
}
export function cli<A, E>(effect: Effect.Effect<A, E>) {
  // Unexpected defects must also stay behind the CLI's secret-safe boundary.
  const guarded = effect.pipe(
    Effect.catchDefect(() =>
      Effect.fail(new OperationError({ message: "Unexpected observer failure", cause: undefined }))
    )
  )
  void Effect.runPromise(guarded.pipe(Effect.catch((error) =>
    Effect.sync(() => {
      console.error(
        error instanceof OperatorError
          ? error.message
          : "Observer operation failed; inspect configuration and retained evidence. No automatic write retry."
      )
      process.exitCode = 1
    })
  )))
}
