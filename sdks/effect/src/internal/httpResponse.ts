import { Context, Effect, ErrorReporter, Exit, Scope, Stream } from "effect"
import {
  Cookies,
  HttpEffect,
  HttpServerError,
  HttpServerRequest,
  HttpServerResponse,
} from "effect/unstable/http"
import {
  copyHttpHeaders,
  HttpRouterError,
  type HttpHeader,
  type HttpRequest,
  type HttpResponse,
} from "@golemcloud/golem-ts-sdk/http-router"
import { AbortableStreamIterable } from "./abortableStreamIterable.js"
import { disposeAgentStream } from "./agentStream.js"
import { ServerRequest, localUrl } from "./httpRequest.js"
import { HttpStreamOwner, ownStream, streamDisposals } from "./ownedStream.js"

export type Body = Stream.Stream<Uint8Array, unknown>
export const CanonicalRequest = Context.Service<HttpRequest<Body>>("golem/http/CanonicalRequest")
const rawHeaders = new WeakMap<HttpServerResponse.HttpServerResponse, readonly HttpHeader[]>()

export function withRawHeaders(
  response: HttpServerResponse.HttpServerResponse,
  headers: readonly HttpHeader[],
) {
  const result = HttpServerResponse.setHeaders(response, {})
  rawHeaders.set(result, copyHttpHeaders(headers))
  return result
}

function byteHeader(name: string, value: string): HttpHeader {
  const bytes = new Uint8Array(value.length)
  for (let i = 0; i < value.length; i++) {
    const code = value.charCodeAt(i)
    if (code > 255) throw new HttpRouterError("invalid-header")
    bytes[i] = code
  }
  return { name, value: bytes }
}

function responseHeaders(
  response: HttpServerResponse.HttpServerResponse,
  original?: readonly HttpHeader[],
) {
  const raw = rawHeaders.get(response) ?? original ?? []
  const replaced = new Set(raw.map((header) => header.name))
  return copyHttpHeaders([
    ...Object.entries(response.headers)
      .filter(([name]) => !replaced.has(name))
      .map(([name, value]) => byteHeader(name, value)),
    ...(replaced.has("set-cookie")
      ? []
      : Cookies.toSetCookieHeaders(response.cookies).map((cookie) =>
          byteHeader("set-cookie", cookie),
        )),
    ...raw,
  ])
}

function responseBody(response: HttpServerResponse.HttpServerResponse): Body {
  switch (response.body._tag) {
    case "Empty":
      return Stream.empty
    case "Uint8Array":
      return Stream.succeed(response.body.body)
    case "Stream":
      return response.body.stream
    case "Raw":
      if (response.body.body instanceof Uint8Array) return Stream.succeed(response.body.body)
      throw new HttpRouterError("unsupported-response-body")
    case "FormData":
      throw new HttpRouterError("unsupported-response-body")
  }
}

/** The caller owns this scope until encoding succeeds; then the output owns it. */
export function handleApplication<E, R>(
  canonical: HttpRequest<Body>,
  application: Effect.Effect<HttpServerResponse.HttpServerResponse, E, R>,
  mount: string,
) {
  return Effect.gen(function* () {
    const scope = (yield* Effect.scope) as Scope.Closeable
    const context = yield* Effect.context<never>()
    let closing: Promise<void> | undefined
    const close = (exit: Exit.Exit<unknown, unknown>) =>
      (closing ??= Effect.runPromiseWith(context)(Scope.close(scope, exit)))
    const owner = new Set<() => Promise<void>>()
    yield* Effect.addFinalizer(() =>
      Effect.forEach(owner, (dispose) => Effect.promise(dispose), {
        concurrency: "unbounded",
        discard: true,
      }),
    )
    const input = new AbortableStreamIterable(canonical.body, Context.empty())
    yield* Effect.addFinalizer(() =>
      Effect.promise(async () => {
        try {
          await input.return()
        } finally {
          await disposeAgentStream(canonical.body)
        }
      }),
    )
    let claimed = false
    const body = Stream.suspend(() => {
      if (claimed) return Stream.fail(new HttpRouterError("request-stream-consumed"))
      claimed = true
      return Stream.fromAsyncIterable(input, () => new HttpRouterError("request-stream"))
    })
    const original = { ...canonical, body }
    const request = new ServerRequest(original, localUrl(canonical, mount))
    let response: HttpServerResponse.HttpServerResponse | undefined
    let originalHeaders: readonly HttpHeader[] | undefined
    let responseContext = context
    const app = application.pipe(
      Effect.catch((error) => {
        if (HttpServerError.isHttpServerError(error)) {
          if (error.reason._tag === "RouteNotFound")
            return Effect.succeed(HttpServerResponse.empty({ status: 404 }))
          if (error.reason._tag === "RequestParseError")
            return Effect.succeed(HttpServerResponse.empty({ status: 400 }))
        }
        return Effect.fail(error)
      }),
      Effect.tap((value) =>
        Effect.sync(() => {
          originalHeaders = rawHeaders.get(value)
        }),
      ),
    )
    yield* HttpEffect.toHandled(Effect.interruptible(app), (_, value) =>
      Effect.gen(function* () {
        const requestScope = (yield* Effect.scope) as Scope.Closeable
        HttpEffect.scopeDisableClose(requestScope)
        yield* Scope.addFinalizerExit(scope, (exit) => Scope.close(requestScope, exit))
        responseContext = yield* Effect.context<never>()
        response = value
      }),
    ).pipe(
      Effect.provideService(HttpServerRequest.HttpServerRequest, request),
      Effect.provideService(CanonicalRequest, original),
      Effect.provideService(HttpStreamOwner, owner),
      // The platform boundary propagates failures; it must not print user data.
      Effect.provideService(ErrorReporter.CurrentErrorReporters, new Set()),
    )
    if (!response) return yield* Effect.fail(new HttpRouterError("missing-response"))
    const result = response as HttpServerResponse.HttpServerResponse
    if (result.body._tag === "Stream") {
      const stream = result.body.stream
      yield* Effect.addFinalizer((exit) =>
        Effect.promise(async () => {
          try {
            await disposeAgentStream(stream)
          } finally {
            await streamDisposals.get(stream)?.(exit)
          }
        }),
      )
    }
    if (!Number.isInteger(result.status) || result.status < 200 || result.status > 599) {
      return yield* Effect.fail(new HttpRouterError("invalid-status"))
    }
    let headers = responseHeaders(result, originalHeaders)
    const bodyless = canonical.method === "HEAD" || [204, 205, 304].includes(result.status)
    if (result.status === 204 || result.status === 205) {
      headers = headers.filter((header) => header.name !== "content-length")
      if (result.status === 205) headers.push(byteHeader("content-length", "0"))
    }
    if (bodyless) {
      yield* Effect.promise(() => close(Exit.void))
      return { status: result.status, headers, body: Stream.empty } satisfies HttpResponse<Body>
    }
    const output = responseBody(result).pipe(Stream.provideContext(responseContext))
    return {
      status: result.status,
      headers,
      body: ownStream(output, close),
    } satisfies HttpResponse<Body>
  })
}
