import { Effect, Inspectable, Option, Schema, Stream } from "effect"
import {
  Cookies,
  Headers,
  HttpIncomingMessage,
  HttpServerError,
  HttpServerRequest,
  Multipart,
  UrlParams,
} from "effect/unstable/http"
import type { HttpMethod } from "effect/unstable/http/HttpMethod"
import { copyHttpHeaders, type HttpRequest } from "@golemcloud/http-contract"

/** Native request view; inspecting it must never read or print body data. */
export class ServerRequest
  extends Inspectable.Class
  implements HttpServerRequest.HttpServerRequest
{
  readonly [HttpServerRequest.TypeId] = HttpServerRequest.TypeId
  readonly [HttpIncomingMessage.TypeId] = HttpIncomingMessage.TypeId
  readonly source = {}
  readonly originalUrl: string
  readonly method: HttpMethod
  readonly headers: Headers.Headers
  readonly stream: Stream.Stream<Uint8Array, HttpServerError.HttpServerError>
  readonly arrayBuffer: Effect.Effect<ArrayBuffer, HttpServerError.HttpServerError>

  constructor(
    readonly canonical: HttpRequest<Stream.Stream<Uint8Array, unknown>>,
    readonly url: string,
    headers?: Headers.Headers,
    readonly remoteAddress: Option.Option<string> = Option.none(),
    cachedBytes?: Effect.Effect<ArrayBuffer, HttpServerError.HttpServerError>,
  ) {
    super()
    this.originalUrl = `${canonical.scheme}://${canonical.authority}${canonical.path}${canonical.query === undefined ? "" : `?${canonical.query}`}`
    // Effect's method type is narrower than its runtime router. Keep extension
    // tokens unchanged; wildcard routes and the canonical service can read them.
    this.method = canonical.method as HttpMethod
    const normalized: Record<string, string> = Object.create(null)
    for (const { name, value } of copyHttpHeaders(canonical.headers)) {
      let text = ""
      for (const byte of value) text += String.fromCharCode(byte)
      normalized[name] =
        normalized[name] === undefined
          ? text
          : `${normalized[name]}${name === "cookie" ? "; " : ", "}${text}`
    }
    normalized.host = canonical.authority
    this.headers = headers ?? Headers.fromInput(normalized)
    this.stream = canonical.body.pipe(Stream.mapError(() => this.parseError()))
    this.arrayBuffer =
      cachedBytes ??
      Effect.runSync(
        Effect.cached(
          Effect.gen({ self: this }, function* () {
            const limit = yield* HttpIncomingMessage.MaxBodySize
            let size = 0
            const chunks: Uint8Array[] = []
            yield* Stream.runForEach(this.stream, (chunk) =>
              Effect.suspend(() => {
                size += chunk.length
                if (limit !== undefined && BigInt(size) > limit)
                  return Effect.fail(this.parseError())
                chunks.push(chunk.slice())
                return Effect.void
              }),
            )
            const bytes = new Uint8Array(size)
            let offset = 0
            for (const chunk of chunks) {
              bytes.set(chunk, offset)
              offset += chunk.length
            }
            return bytes.buffer
          }),
        ),
      )
  }

  toJSON() {
    return { _id: "GolemHttpRequest", method: this.method }
  }

  private parseError(): HttpServerError.HttpServerError {
    return new HttpServerError.HttpServerError({
      reason: new HttpServerError.RequestParseError({
        request: this,
        description: "Invalid request body",
      }),
    })
  }

  modify(options: Parameters<HttpServerRequest.HttpServerRequest["modify"]>[0]) {
    const view = new ServerRequest(
      this.canonical,
      options.url ?? this.url,
      options.headers ?? this.headers,
      options.remoteAddress ?? this.remoteAddress,
      this.arrayBuffer,
    )
    // Effect associates pre-response hooks with source identity across views.
    Object.defineProperty(view, "source", { value: this.source })
    return view
  }

  get cookies() {
    return Cookies.parseHeader(this.headers.cookie ?? "")
  }
  get text() {
    return Effect.map(this.arrayBuffer, (bytes) => new TextDecoder().decode(bytes))
  }
  get json(): Effect.Effect<Schema.Json, HttpServerError.HttpServerError> {
    return Effect.flatMap(this.text, (text) =>
      Effect.try({
        try: () => JSON.parse(text) as Schema.Json,
        catch: () => this.parseError(),
      }),
    )
  }
  get urlParamsBody() {
    return Effect.map(this.text, (text) => UrlParams.fromInput(new URLSearchParams(text)))
  }
  get multipartStream() {
    return this.stream.pipe(
      Stream.mapError(() => Multipart.MultipartError.fromReason("InternalError")),
      Stream.pipeThroughChannel(Multipart.makeChannel(this.headers)),
    )
  }
  get multipart() {
    return Multipart.toPersisted(this.multipartStream)
  }
  get upgrade() {
    return Effect.fail(this.parseError())
  }
}

/** Remove only the matched literal mount's raw segments, never re-encode the target. */
export function localUrl(request: HttpRequest<unknown>, mount: string): string {
  const count = mount === "/" ? 0 : mount.slice(1).split("/").length
  const path =
    count === 0
      ? request.path
      : `/${request.path
          .split("/")
          .slice(count + 1)
          .join("/")}`
  return `${path}${request.query === undefined ? "" : `?${request.query}`}`
}
