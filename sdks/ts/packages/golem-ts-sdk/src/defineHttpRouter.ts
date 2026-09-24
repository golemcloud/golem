// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1

import type { AgentContext, AgentImpl, ConfigSpec, ConfigView, MethodsRecord } from './defineAgent';
import { AgentTypeRegistry } from './internal/registry/agentTypeRegistry';
import { registerAgentInitiator, registerAgentType } from './runtime';
import {
  httpRequestSchema,
  httpResponseSchema,
  httpStringSchema,
} from './internal/http/routerSchema';
import {
  compileFileMappings,
  compileRouterMount,
  copyHttpHeaders,
  HttpRouterError,
  serializeOpenApi,
  type FileExposure,
  type HttpRequest,
  type HttpResponse,
} from './httpRouterContract';
import { AgentStream, disposeAgentStream } from './schema/agentStream';
import { webRequest, webResponse } from './httpRouterWeb';

export interface HttpRouterOptions<Config extends ConfigSpec> {
  readonly config?: Config;
  readonly description?: string;
}

export interface HttpRouterContext<Config extends ConfigSpec = {}> {
  readonly config: ConfigView<Config>;
}

/** Original head alongside the normalized Web Request; the body has one reader. */
export interface WebHttpRouterContext<
  Config extends ConfigSpec = {},
> extends HttpRouterContext<Config> {
  readonly rawRequest: Omit<HttpRequest<never>, 'body'>;
}

export type HttpRouterHandler<Config extends ConfigSpec = {}> = (
  request: Request,
  context: WebHttpRouterContext<Config>,
) => Response | Promise<Response>;

export type RawHttpRouterHandler<Config extends ConfigSpec = {}> = (
  request: HttpRequest<AgentStream<Uint8Array>>,
  context: HttpRouterContext<Config>,
) => HttpResponse<AgentStream<Uint8Array>> | Promise<HttpResponse<AgentStream<Uint8Array>>>;

export interface HttpRouterBuilder<Config extends ConfigSpec> {
  mount(
    path: string,
    options?: { readonly auth?: boolean; readonly cors?: readonly string[] },
  ): HttpRouterBuilder<Config>;
  static(route: string, path: string): HttpRouterBuilder<Config>;
  openApi(
    provider: (context: HttpRouterContext<Config>) => unknown | Promise<unknown>,
    options?: { readonly methodName?: string },
  ): HttpRouterBuilder<Config>;
  /** Finalize registration; omit the handler for static/provider-only or empty routers. */
  implement(
    handler?: HttpRouterHandler<Config>,
    options?: { readonly methodName?: string },
  ): AgentImpl;
  /** Canonical streaming API, without Fetch method, URL, status, or header normalization. */
  implementRaw(
    handler: RawHttpRouterHandler<Config>,
    options?: { readonly methodName?: string },
  ): AgentImpl;
}

/** Define a parameterless ephemeral HTTP router using the ordinary agent runtime. */
export function defineHttpRouter<Config extends ConfigSpec = {}>(
  name: string,
  options: HttpRouterOptions<Config> = {},
): HttpRouterBuilder<Config> {
  let mount: { path: string; auth?: boolean; cors?: readonly string[] } | undefined;
  const mappings: FileExposure[] = [];
  let provider: ((context: HttpRouterContext<Config>) => unknown | Promise<unknown>) | undefined;
  let providerName: string | undefined;
  let implemented = false;
  function mutable() {
    if (implemented) throw new HttpRouterError('router-already-implemented');
  }
  function finish(handler?: RawHttpRouterHandler<Config>, methodName = 'handle'): AgentImpl {
    mutable();
    implemented = true;
    try {
      if (!mount) throw new HttpRouterError('router-mount-required');
      const router = {
        mount: mount.path,
        auth: mount.auth,
        cors: mount.cors,
        staticBindings: compileFileMappings(mappings),
        handlerMethod: handler ? methodName : undefined,
        openapiProviderMethod: providerName,
      };
      compileRouterMount(router);
      const methods: MethodsRecord = Object.create(null);
      const implementations: Record<string, (...args: any[]) => unknown> = Object.create(null);
      if (handler) {
        methods[methodName] = {
          input: { request: httpRequestSchema },
          returns: httpResponseSchema,
        };
        implementations[methodName] = async function (
          this: AgentContext<Config>,
          { request }: { request: HttpRequest<AgentStream<Uint8Array>> },
        ) {
          let response: HttpResponse<AgentStream<Uint8Array>> | undefined;
          let cleanup: Promise<void> | undefined;
          const close = () =>
            (cleanup ??= (async () => {
              try {
                if (response) await disposeAgentStream(response.body);
              } finally {
                await disposeAgentStream(request.body);
              }
            })());
          try {
            response = await handler(request, { config: this.config });
            if (
              !Number.isInteger(response.status) ||
              response.status < 200 ||
              response.status > 599
            )
              throw new HttpRouterError('invalid-status');
            const headers = copyHttpHeaders(response.headers);
            if (request.method === 'HEAD' || [204, 205, 304].includes(response.status)) {
              // Dispose before encoding: a native P3 pump may pull immediately on wrap.
              await close();
              return { status: response.status, headers, body: AgentStream.from<Uint8Array>([]) };
            }
            const output = response.body;
            return {
              status: response.status,
              headers,
              body: AgentStream.from<Uint8Array>({
                [Symbol.asyncIterator]: () => ({
                  async next() {
                    try {
                      const next = await output.next();
                      if (next.done) await close();
                      return next;
                    } catch (error) {
                      await close().catch(() => undefined);
                      throw error;
                    }
                  },
                  async return() {
                    await close();
                    return { done: true as const, value: undefined };
                  },
                }),
              }),
            };
          } catch (error) {
            await close().catch(() => undefined);
            throw error;
          }
        };
      }
      if (provider && providerName) {
        const provide = provider;
        methods[providerName] = { input: {}, returns: httpStringSchema };
        implementations[providerName] = async function (this: AgentContext<Config>) {
          return serializeOpenApi(await provide({ config: this.config }));
        };
      }
      const registered = registerAgentType(name, {}, methods, {
        mode: 'ephemeral',
        snapshotting: 'disabled',
        config: options.config,
        description: options.description,
        router,
      });
      registerAgentInitiator(registered, { init: () => ({}), methods: implementations });
    } catch (error) {
      AgentTypeRegistry.recordRegistrationError(
        name,
        `Router definition failed: ${error instanceof Error ? error.message : 'invalid-router'}`,
      );
    }
    return { name };
  }
  const builder: HttpRouterBuilder<Config> = {
    mount(path, mountOptions = {}) {
      mutable();
      if (mount) throw new HttpRouterError('router-multiple-mounts');
      compileRouterMount({ mount: path, ...mountOptions });
      mount = { path, ...mountOptions };
      return builder;
    },
    static(route, path) {
      mutable();
      compileFileMappings([...mappings, { route, path }]);
      mappings.push({ route, path });
      return builder;
    },
    openApi(value, providerOptions = {}) {
      mutable();
      if (provider) throw new HttpRouterError('router-multiple-providers');
      provider = value;
      providerName = providerOptions.methodName ?? 'openApi';
      return builder;
    },
    implement(handler, handlerOptions = {}) {
      return finish(
        handler &&
          (async (raw, context) => {
            const { request, close } = await webRequest(raw);
            let response: Response | undefined;
            try {
              const { body: _body, ...head } = raw;
              response = await handler(request, { ...context, rawRequest: head });
              return webResponse(response, close);
            } catch (error) {
              await response?.body?.cancel().catch(() => undefined);
              await close().catch(() => undefined);
              throw error;
            }
          }),
        handlerOptions.methodName,
      );
    },
    implementRaw(handler, handlerOptions = {}) {
      return finish(handler, handlerOptions.methodName);
    },
  };
  return builder;
}
