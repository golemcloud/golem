import { z } from 'zod';
import {
    defineAgent,
    method,
    http,
    s,
    Result,
    createWebhook,
} from '@golemcloud/golem-ts-sdk';

// ---------------------------------------------------------------------------
// NOTE (fluent port): the decorator-era `PrincipalAgent` (echo-principal /
// echo-principal-mid / echo-principal-last / authed-principal) is NOT ported.
// It relied on TWO decorator-only capabilities the fluent (Standard Schema)
// surface cannot express:
//   1. auto-injecting the caller `Principal` into a method parameter bound at
//      HTTP-dispatch time (there is no `Principal` input schema / marker), and
//   2. returning a WIT `Principal` value (no `Principal` return schema).
// `this.getPrincipal()` exposes the principal, but it cannot be declared as a
// method input or serialized as a schema-typed return value. See the report.
// The `custom_api/agent_http_principal_ts.rs` tests exercise this agent.
// ---------------------------------------------------------------------------

// Response schemas for comprehensive HTTP method testing
const ResourceUpdate = z.object({
  name: z.string().optional(),
  description: z.string().optional(),
  enabled: z.boolean().optional(),
});

const ResourceResponse = z.object({
  id: z.string(),
  updated: z.boolean(),
  method: z.string(),
});

export const HttpAgent = defineAgent({
  name: 'HttpAgent',
  id: { agentName: z.string() },
  http: http.mount('/http-agents/{agentName}'),
  methods: {
    stringPathVar: method({
      input: { pathVar: z.string() },
      returns: z.object({ value: z.string() }),
      http: http.get('/string-path-var/{pathVar}'),
    }),

    multiPathVars: method({
      input: { first: z.string(), second: z.string() },
      returns: z.object({ joined: z.string() }),
      http: http.get('/multi-path-vars/{first}/{second}'),
    }),

    remainingPath: method({
      input: { tail: z.string() },
      returns: z.object({ tail: z.string() }),
      http: http.get('/rest/{*tail}'),
    }),

    pathAndQuery: method({
      input: { itemId: z.string(), limit: z.number() },
      returns: z.object({ id: z.string(), limit: z.number() }),
      http: http.get('/path-and-query/{itemId}?limit={limit}'),
    }),

    pathAndHeader: method({
      input: { resourceId: z.string(), requestId: z.string() },
      returns: z.object({ resourceId: z.string(), requestId: z.string() }),
      http: http.get('/path-and-header/{resourceId}', { headers: { 'x-request-id': 'requestId' } }),
    }),

    jsonBody: method({
      input: { id: z.string(), name: z.string(), count: z.number() },
      returns: z.object({ ok: z.boolean() }),
      http: http.post('/json-body/{id}'),
    }),

    unrestrictedUnstructuredBinary: method({
      input: { bucket: z.string(), payload: s.unstructuredBinary() },
      returns: z.number(),
      http: http.post('/unrestricted-unstructured-binary/{bucket}'),
    }),

    restrictedUnstructuredBinary: method({
      input: { bucket: z.string(), payload: s.unstructuredBinary({ mimeTypes: ['image/gif'] }) },
      returns: z.number(),
      http: http.post('/restricted-unstructured-binary/{bucket}'),
    }),

    noContent: method({
      input: {},
      returns: z.void(),
      http: http.get('/resp/no-content'),
    }),

    jsonResponse: method({
      input: {},
      returns: z.object({ value: z.string() }),
      http: http.get('/resp/json'),
    }),

    optionalResponse: method({
      input: { found: z.boolean() },
      returns: z.object({ value: z.string() }).optional(),
      http: http.get('/resp/optional/{found}'),
    }),

    resultOkOrErr: method({
      input: { ok: z.boolean() },
      returns: s.result(z.object({ value: z.string() }), z.object({ error: z.string() })),
      http: http.get('/resp/result-json-json/{ok}'),
    }),

    resultVoidErr: method({
      input: {},
      returns: s.result(z.void(), z.object({ error: z.string() })),
      http: http.post('/resp/result-void-json'),
    }),

    resultJsonVoid: method({
      input: {},
      returns: s.result(z.object({ value: z.string() }), z.void()),
      http: http.get('/resp/result-json-void'),
    }),

    binaryResponse: method({
      input: {},
      returns: s.unstructuredBinary(),
      http: http.get('/resp/binary'),
    }),

    patchResource: method({
      input: { id: z.string(), update: ResourceUpdate },
      returns: ResourceResponse,
      http: http.patch('/resource/{id}'),
    }),

    patchPartial: method({
      input: { id: z.string() },
      returns: ResourceResponse,
      http: http.patch('/resource/{id}/partial'),
    }),
  },
});

export const HttpAgentImpl = HttpAgent.implement({
  init: ({ id }) => ({ agentName: id.agentName }),
  methods: {
    stringPathVar({ pathVar }) {
      return { value: pathVar };
    },
    multiPathVars({ first, second }) {
      return { joined: `${first}:${second}` };
    },
    remainingPath({ tail }) {
      return { tail };
    },
    pathAndQuery({ itemId, limit }) {
      return { id: itemId, limit };
    },
    pathAndHeader({ resourceId, requestId }) {
      return { resourceId, requestId };
    },
    jsonBody({ id, name, count }) {
      return { ok: true };
    },
    unrestrictedUnstructuredBinary({ bucket, payload }) {
      if (payload.tag === 'url') {
        return -1;
      } else {
        return payload.val.byteLength;
      }
    },
    restrictedUnstructuredBinary({ bucket, payload }) {
      if (payload.tag === 'url') {
        return -1;
      } else {
        return payload.val.byteLength;
      }
    },
    noContent() { },
    jsonResponse() {
      return { value: 'ok' };
    },
    optionalResponse({ found }) {
      return found ? { value: 'yes' } : undefined;
    },
    resultOkOrErr({ ok }) {
      return ok
        ? Result.ok({ value: 'ok' })
        : Result.err({ error: 'boom' });
    },
    resultVoidErr() {
      return Result.err({ error: 'fail' });
    },
    resultJsonVoid() {
      return Result.ok({ value: 'ok' });
    },
    binaryResponse() {
      return { tag: 'inline' as const, val: new Uint8Array([1, 2, 3, 4]), mimeType: 'application/octet-stream' };
    },
    patchResource({ id, update }) {
      return {
        id: id,
        updated: true,
        method: 'PATCH',
      };
    },
    patchPartial({ id }) {
      return {
        id: id,
        updated: true,
        method: 'PATCH',
      };
    },
  },
});

export const CorsAgent = defineAgent({
  name: 'CorsAgent',
  id: { agentName: z.string() },
  http: http.mount('/cors-agents/{agentName}', { cors: ['https://mount.example.com'] }),
  methods: {
    // GET endpoint adds additional CORS on top of mount
    wildcard: method({
      input: {},
      returns: z.object({ ok: z.boolean() }),
      http: http.get('/wildcard', { cors: ['*'] }), // union with mount CORS
    }),

    // GET endpoint inherits mount CORS if empty
    inherited: method({
      input: {},
      returns: z.object({ ok: z.boolean() }),
      http: http.get('/inherited'),
    }),

    // POST endpoint requiring preflight
    preflight: method({
      input: { body: z.object({ name: z.string() }) },
      returns: z.object({ received: z.string() }),
      http: http.post('/preflight-required', { cors: ['https://app.example.com'] }),
    }),
  },
});

export const CorsAgentImpl = CorsAgent.implement({
  init: ({ id }) => ({ agentName: id.agentName }),
  methods: {
    wildcard() {
      return { ok: true };
    },
    inherited() {
      return { ok: true };
    },
    preflight({ body }) {
      return { received: body.name };
    },
  },
});

export const WebhookAgent = defineAgent({
  name: 'WebhookAgent',
  id: { agentName: z.string() },
  http: http.mount('/webhook-agents/{agentName}', { webhookSuffix: '/webhook-agent' }),
  methods: {
    setTestServerUrl: method({
      input: { testServerUrl: z.string() },
      returns: z.void(),
      http: http.post('/set-test-server-url'),
    }),

    // Webhook callback dance
    testWebhook: method({
      input: {},
      returns: z.object({ payloadLength: z.number() }),
      http: http.post('/test-webhook'),
    }),
  },
});

export const WebhookAgentImpl = WebhookAgent.implement({
  init: () => ({ testServerUrl: '' }),
  methods: {
    setTestServerUrl({ testServerUrl }) {
      this.testServerUrl = testServerUrl;
    },
    async testWebhook() {
      const webhook = createWebhook();
      await fetch(this.testServerUrl!, {
        method: 'POST',
        headers: {
          'Accept': 'application/json',
          'Content-Type': 'application/json',
        },
        body: JSON.stringify({ webhookUrl: webhook.getUrl() }),
      });

      const data = await webhook;

      return { payloadLength: data.bytes().byteLength };
    },
  },
});
