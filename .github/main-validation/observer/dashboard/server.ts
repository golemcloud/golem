/** Public read model. No GitHub access, workload execution or analysis credentials. */
import { Effect } from "effect"
import { timingSafeEqual } from "node:crypto"
import { cli, io, rejectCredentials } from "../runtime.ts"
import { publicProgress, publicRun } from "./model.ts"
import { Store } from "./store.ts"
import type { Update } from "./store.ts"

const headers = {
  "Cache-Control": "no-store",
  "X-Content-Type-Options": "nosniff",
  "Referrer-Policy": "no-referrer",
  "Content-Security-Policy":
    "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; connect-src 'self'; img-src 'self' data:; base-uri 'none'; frame-ancestors 'none'; form-action 'none'"
}
const json = (status: number, data: unknown) => Response.json(data, { status, headers })
const authenticated = (supplied: string, token: string) => {
  const a = Buffer.from(supplied), b = Buffer.from(`Bearer ${token}`)
  return !!token && a.length === b.length && timingSafeEqual(a, b)
}
export function handler(store: Store, token: string, progressToken = "") {
  return (request: Request): Promise<Response> =>
    Effect.runPromise(
      Effect.gen(function*() {
        const path = new URL(request.url).pathname
        if (request.method === "GET") {
          if (path === "/data.json") return json(200, store.snapshot())
          if (path === "/healthz") return json(200, { status: "ok" })
          if (path === "/" || path === "/brand.png") {
            return new Response(
              yield* io(() =>
                Bun.file(`${import.meta.dir}/${path === "/" ? "index.html" : "brand.png"}`).arrayBuffer()
              ),
              { headers: { ...headers, "Content-Type": path === "/" ? "text/html; charset=utf-8" : "image/png" } }
            )
          }
          return json(404, { error: "not found" })
        }
        if (request.method !== "POST") return json(405, { error: "method not allowed" })
        const progress = path === "/api/progress", credential = progress ? progressToken : token
        if (!authenticated(request.headers.get("Authorization") ?? "", credential)) {
          return json(401, { error: "unauthorized" })
        }
        if (!["/api/run", "/api/heartbeat", "/api/progress"].includes(path)) return json(404, { error: "not found" })
        const parsed = yield* Effect.result(
          io(async (): Promise<Update | { readonly _tag: "TooLarge" }> => {
            const length = Number(request.headers.get("Content-Length") ?? "0")
            if (!Number.isSafeInteger(length) || length < 0 || length > 2 * 1024 * 1024) return { _tag: "TooLarge" }
            // Bound chunked bodies too, without blocking other HTTP requests.
            const reader = request.body?.getReader()
            const chunks: Uint8Array[] = []
            let size = 0
            if (reader) {
              try {
                while (true) {
                  const chunk = await reader.read()
                  if (chunk.done) break
                  size += chunk.value.length
                  if (size > 2 * 1024 * 1024) {
                    await reader.cancel()
                    return { _tag: "TooLarge" }
                  }
                  chunks.push(chunk.value)
                }
              } finally {
                reader.releaseLock()
              }
            }
            const payload: unknown = path === "/api/heartbeat"
              ? null
              : JSON.parse(Buffer.concat(chunks).toString("utf8"))
            rejectCredentials(payload, [token, progressToken])
            if (progress) return { _tag: "Progress", progress: publicProgress(payload) }
            return payload === null ? { _tag: "Heartbeat" } : { _tag: "Observation", run: publicRun(payload) }
          }).pipe(Effect.timeout("15 seconds"))
        )
        if (parsed._tag === "Failure") return json(400, { error: "invalid observation" })
        if (parsed.success._tag === "TooLarge") return json(413, { error: "payload too large" })
        const saved = yield* Effect.result(store.update(parsed.success))
        if (saved._tag === "Failure") {
          return json(saved.failure._tag === "PersistenceError" ? 503 : 400, { error: "could not store observation" })
        }
        return json(200, { status: "stored" })
      }).pipe(Effect.catchCause(() => Effect.succeed(json(503, { error: "service unavailable" }))))
    )
}
export const serve = (directory: string, token: string, progressToken = "", port = 8080, hostname = "0.0.0.0") =>
  Effect.gen(function*() {
    const store = yield* Store.open(directory)
    return Bun.serve({
      hostname,
      port,
      idleTimeout: 15,
      maxRequestBodySize: 2 * 1024 * 1024,
      fetch: handler(store, token, progressToken)
    })
  })
if (import.meta.main) {
  cli(Effect.gen(function*() {
    const directory = process.env.AMP_DEPLOYMENT_DATA_DIRECTORY || process.env.DASHBOARD_DATA_DIR
    if (!directory) throw new Error("set DASHBOARD_DATA_DIR or AMP_DEPLOYMENT_DATA_DIRECTORY")
    const server = yield* serve(
      directory,
      process.env.CI_DASHBOARD_TOKEN ?? "",
      process.env.CI_DASHBOARD_PROGRESS_TOKEN ?? "",
      Number(process.env.PORT ?? 8080)
    )
    console.log(`Dashboard listening on port ${server.port}`)
  }))
}
