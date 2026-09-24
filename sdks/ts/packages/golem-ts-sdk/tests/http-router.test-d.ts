// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1

import {
  defineHttpRouter,
  defineAgent,
  http,
  AgentStream,
  withRawHeaders,
} from '../dist/index.mjs';
import {
  compileFileMappings,
  compileRouterMount,
  serializeOpenApi,
  type HttpRequest,
} from '../dist/http-router.mjs';
import { z } from 'zod';

defineHttpRouter('TypedRouter', { config: { message: z.string() } })
  .mount('/web', { auth: true, cors: ['https://example.test'] })
  .static('/*', '/files/$1')
  .openApi(({ config }) => ({
    openapi: '3.1.0',
    info: { title: config.message, version: '1' },
    paths: {},
  }))
  .implement((request, context) => {
    request satisfies Request;
    context.config.message satisfies string;
    context.rawRequest.query satisfies string | undefined;
    // @ts-expect-error no duplicate body reader on the original head
    context.rawRequest.body;
    // @ts-expect-error configuration keys are inferred
    context.config.missing;
    return withRawHeaders(new Response(request.body), [
      { name: 'x-byte', value: new Uint8Array([255]) },
    ]);
  });

defineHttpRouter('Raw')
  .mount('/')
  .implementRaw((request) => ({ status: 200, headers: request.headers, body: request.body }));
defineHttpRouter('Empty').mount('/').implement();
// @ts-expect-error routers have no ordinary client
defineHttpRouter('NoClient').client;
// @ts-expect-error no dependency tracking API
defineHttpRouter('NoDependencies', { dependencies: [] });
// @ts-expect-error routers cannot be durable
defineHttpRouter('NoMode', { mode: 'durable' });
// @ts-expect-error live file exposure is not a router mount option
defineHttpRouter('NoLiveFiles').mount('/', { exposeFiles: [] });
defineHttpRouter('NoString')
  .mount('/')
  // @ts-expect-error Web handlers return Response, not string
  .implement(() => 'hello');

defineAgent({
  name: 'Files',
  id: { name: z.string() },
  methods: {},
  http: http.mount('/files/{name}', { exposeFiles: [{ route: '/*', path: '/files/$1' }] }),
});
defineAgent({
  name: 'MissingBinding',
  id: { name: z.string(), region: z.string() },
  methods: {},
  // @ts-expect-error every constructor field must be bound by the mount
  http: http.mount('/files/{name}', { exposeFiles: [{ route: '/*', path: '/files/$1' }] }),
});

const mappings = compileFileMappings([{ route: '/assets/*', path: '/assets/$1' }] as const);
compileRouterMount({ mount: '/', staticBindings: mappings });
serializeOpenApi({});
declare const arbitraryBody: HttpRequest<{ readonly effectStream: true }>;
arbitraryBody.body satisfies { readonly effectStream: true };
declare const canonical: HttpRequest<AgentStream<Uint8Array>>;
canonical.body satisfies AgentStream<Uint8Array>;
