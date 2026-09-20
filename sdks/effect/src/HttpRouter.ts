/** @since 1.6.0 */
import { Context, Effect, Schema, Scope, Stream } from "effect"
import type * as HttpServerRequest from "effect/unstable/http/HttpServerRequest"
import type * as HttpServerResponse from "effect/unstable/http/HttpServerResponse"
import {
  compileFileMappings,
  compileRouterMount,
  HttpRouterError,
  serializeOpenApi,
  type FileExposure,
  type HttpHeader,
  type HttpRequest,
  type HttpResponse,
} from "@golemcloud/golem-ts-sdk/http-router"
import type { ConfigFields } from "./Config.js"
import type { MountDef } from "./Http.js"
import { registerAgent, type ConfigDef, type CfgTagOf, type Handlers } from "./internal/agent.js"
import { method } from "./internal/method.js"
import type { HostServices } from "./host/HostLive.js"
import type { Principal } from "./Principal.js"
import { AgentStream, Uint16, Uint8ArraySchema } from "./WitTypes.js"
import {
  CanonicalRequest,
  handleApplication,
  withRawHeaders as rawResponseHeaders,
} from "./internal/httpResponse.js"

/** Incremental byte chunks carried by the HTTP envelope. @since 1.6.0 @category models */
export type Body = Stream.Stream<Uint8Array, unknown>

/** Original public request, including ordered byte headers. @since 1.6.0 @category services */
export const request: Context.Service<HttpRequest<Body>, HttpRequest<Body>> = CanonicalRequest

/** Shared canonical envelope types. @since 1.6.0 @category models */
export type { HttpHeader, HttpRequest, HttpResponse, FileExposure }

/**
 * Replace named normalized response fields with ordered byte occurrences.
 * Apply after ordinary Effect response combinators; subsequent cloning does not retain this metadata.
 * @since 1.6.0
 * @category headers
 */
export const withRawHeaders: (
  response: HttpServerResponse.HttpServerResponse,
  headers: readonly HttpHeader[],
) => HttpServerResponse.HttpServerResponse = rawResponseHeaders

const Header = Schema.Struct({ name: Schema.String, value: Uint8ArraySchema })
const Request = Schema.Struct({
  method: Schema.String,
  scheme: Schema.String,
  authority: Schema.String,
  path: Schema.String,
  query: Schema.UndefinedOr(Schema.String),
  headers: Schema.Array(Header),
  body: AgentStream(Uint8ArraySchema),
})
const Response = Schema.Struct({
  status: Uint16,
  headers: Schema.Array(Header),
  body: AgentStream(Uint8ArraySchema),
})

type Services<F extends ConfigFields> = HostServices | Principal | CfgTagOf<F>
type Application<F extends ConfigFields> = Effect.Effect<
  HttpServerResponse.HttpServerResponse,
  unknown,
  Services<F> | Scope.Scope | HttpServerRequest.HttpServerRequest | HttpRequest<Body>
>

/** Router mount, provisioning mappings, and optional lazy provider. @since 1.6.0 @category models */
export interface Options<F extends ConfigFields = never> {
  readonly mount: MountDef<string, string>
  readonly static?: readonly FileExposure[]
  readonly config?: ConfigDef<F>
  readonly openApi?: Effect.Effect<unknown, unknown, Services<F> | Scope.Scope>
  readonly handlerMethod?: string
  readonly openapiProviderMethod?: string
}

/** Router definition without an ordinary callable client. @since 1.6.0 @category models */
export interface Definition<F extends ConfigFields> {
  readonly name: string
  readonly implement: (
    makeApplication: Effect.Effect<Application<F>, unknown, Services<F> | Scope.Scope>,
  ) => void
  readonly register: () => void
}

/**
 * Define a parameterless ephemeral router. Implement with HttpRouter.toHttpEffect(routes),
 * or Effect.succeed(application). Use register() for handlerless routers.
 * @since 1.6.0
 * @category constructors
 */
export function define<F extends ConfigFields = never>(
  name: string,
  options: Options<F>,
): Definition<F> {
  const mount = options.mount
  if (
    mount.phantomAgent ||
    mount.webhookSuffix.length ||
    mount.exposeFiles?.length ||
    mount.pathPrefix.some((segment) => segment._tag !== "Literal")
  ) {
    throw new HttpRouterError("router-mount")
  }
  const path = `/${mount.pathPrefix.map((segment) => (segment._tag === "Literal" ? segment.value : "")).join("/")}`
  const staticBindings = compileFileMappings(options.static ?? [])
  const config = options.config
  const provider = options.openApi
  const handlerMethod = options.handlerMethod ?? "handle"
  const providerMethod = provider ? (options.openapiProviderMethod ?? "openApi") : undefined
  const mountOptions = {
    mount: path,
    auth: mount.authRequired,
    cors: [...mount.cors],
    staticBindings,
    openapiProviderMethod: providerMethod,
  }
  const register = (
    makeApplication?: Effect.Effect<Application<F>, unknown, Services<F> | Scope.Scope>,
  ) => {
    const compiled = compileRouterMount({
      ...mountOptions,
      handlerMethod: makeApplication ? handlerMethod : undefined,
    })
    const handlerSpec = method({ input: { request: Request }, success: Response })
    const providerSpec = method({ input: {}, success: Schema.String })
    const methods = {
      ...(makeApplication ? { [handlerMethod]: handlerSpec } : {}),
      ...(provider ? { [providerMethod!]: providerSpec } : {}),
    }
    const application = makeApplication ? Effect.flatten(makeApplication) : undefined
    Effect.runSync(
      registerAgent(
        { name, id: {}, mode: "ephemeral", methods, config },
        {
          init: () => Effect.void,
          methods: () =>
            ({
              ...(application
                ? {
                    [handlerMethod]: ({ request }: { request: HttpRequest<Body> }) =>
                      handleApplication(request, application, path),
                  }
                : {}),
              ...(provider
                ? {
                    [providerMethod!]: () =>
                      Effect.flatMap(provider, (document) =>
                        Effect.try({
                          try: () => serializeOpenApi(document),
                          catch: (error) => error,
                        }),
                      ),
                  }
                : {}),
            }) as Handlers<typeof methods, CfgTagOf<F>>,
        },
        {
          mount: compiled.mount,
          endpoints: new Map(makeApplication ? [[handlerMethod, [compiled.handlerBinding!]]] : []),
        },
      ),
    )
  }
  return Object.freeze({ name, implement: register, register: () => register() })
}
