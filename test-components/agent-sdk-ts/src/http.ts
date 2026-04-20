import {
    BaseAgent,
    Result,
    agent,
    endpoint,
    UnstructuredBinary,
    Principal,
    createWebhook
} from '@golemcloud/golem-ts-sdk';

// Response interfaces for comprehensive HTTP method testing
interface ResourceUpdate {
  name?: string;
  description?: string;
  enabled?: boolean;
}

interface ResourceResponse {
  id: string;
  updated: boolean;
  method: string;
}

interface ResourceMetadata {
  id: string;
  exists: boolean;
  contentLength?: number;
}

interface OptionsResponse {
  allowedMethods: string[];
  allowedHeaders: string[];
  maxAge: number;
}

interface ApiOptionsResponse {
  version: string;
  endpoints: string[];
}

interface TunnelResponse {
  host: string;
  port: number;
  connected: boolean;
}

interface ProxyResponse {
  target: string;
  proxyActive: boolean;
}

interface TraceResponse {
  path: string;
  receivedHeaders: string[];
  timestamp: number;
}

@agent({
  mount: '/http-agents/{agentName}',
})
class HttpAgent extends BaseAgent {

  constructor(readonly agentName: string) {
      super();
  }

  @endpoint({ get: "/string-path-var/{pathVar}" })
  stringPathVar(pathVar: string): { value: string } {
    return { value: pathVar  }
  }

  @endpoint({ get: "/multi-path-vars/{first}/{second}" })
  multiPathVars(first: string, second: string): { joined: string } {
    return { joined: `${first}:${second}` }
  }

  @endpoint({ get: "/rest/{*tail}" })
  remainingPath(tail: string): { tail: string } {
    return { tail }
  }

  @endpoint({ get: "/path-and-query/{itemId}?limit={limit}" })
  pathAndQuery(itemId: string, limit: number): { id: string; limit: number } {
    return { id: itemId, limit }
  }

  @endpoint({
    get: "/path-and-header/{resourceId}",
    headers: { "x-request-id" : "requestId" }
  })
  pathAndHeader(
    resourceId: string,
    requestId: string
  ): { resourceId: string; requestId: string } {
    return { resourceId, requestId }
  }

  @endpoint({ post: "/json-body/{id}" })
  jsonBody(
    id: string,
    name: string,
    count: number
  ): { ok: boolean } {
    return { ok: true }
  }

  @endpoint({ post: "/unrestricted-unstructured-binary/{bucket}" })
  unrestrictedUnstructuredBinary(
    bucket: string,
    payload: UnstructuredBinary
  ): number {
    if (payload.tag === 'url') {
      return -1
    } else {
      return payload.val.byteLength
    }
  }

  @endpoint({ post: "/restricted-unstructured-binary/{bucket}" })
  restrictedUnstructuredBinary(
    bucket: string,
    payload: UnstructuredBinary<["image/gif"]>
  ): number {
    if (payload.tag === 'url') {
      return -1
    } else {
      return payload.val.byteLength
    }
  }

  @endpoint({ get: "/resp/no-content" })
  noContent() { }

  @endpoint({ get: "/resp/json" })
  jsonResponse(): { value: string } {
    return { value: "ok" };
  }

  @endpoint({ get: "/resp/optional/{found}" })
  optionalResponse(found: boolean): { value: string } | undefined {
    return found ? { value: "yes" } : undefined ;
  }

  @endpoint({ get: "/resp/result-json-json/{ok}" })
  resultOkOrErr(ok: boolean): Result<{ value: string }, { error: string }> {
    return ok
      ? Result.ok({ value: "ok" })
      : Result.err({ error: "boom" });
  }

  @endpoint({ post: "/resp/result-void-json" })
  resultVoidErr(): Result<void, { error: string }> {
    return Result.err({ error: "fail" })
  }

  @endpoint({ get: "/resp/result-json-void" })
  resultJsonVoid(): Result<{ value: string }, void> {
    return Result.ok({ value: "ok" })
  }

  @endpoint({ get: "/resp/binary" })
  binaryResponse(): UnstructuredBinary {
    return UnstructuredBinary.fromInline(new Uint8Array([1, 2, 3, 4]), 'application/octet-stream')
  }

  // PATCH method endpoints
  @endpoint({ patch: "/resource/{id}" })
  patchResource(id: string, update: ResourceUpdate): ResourceResponse {
    return {
      id: id,
      updated: true,
      method: "PATCH"
    };
  }

  @endpoint({ patch: "/resource/{id}/partial" })
  patchPartial(id: string): ResourceResponse {
    return {
      id: id,
      updated: true,
      method: "PATCH"
    };
  }

  // HEAD method endpoints
  @endpoint({ head: "/resource/{id}" })
  headResource(id: string): ResourceMetadata {
    return {
      id: id,
      exists: true,
      contentLength: 1024
    };
  }

  @endpoint({ head: "/resource/{id}/exists" })
  headExists(id: string): void {
    // HEAD method with no response body (204)
  }

  // OPTIONS method endpoints
  @endpoint({ options: "/resource/{id}" })
  optionsResource(id: string): OptionsResponse {
    return {
      allowedMethods: ["GET", "POST", "PATCH", "HEAD", "OPTIONS", "DELETE"],
      allowedHeaders: ["Content-Type", "Authorization", "X-Request-ID"],
      maxAge: 86400
    };
  }

  @endpoint({ options: "/api" })
  optionsApi(): ApiOptionsResponse {
    return {
      version: "1.0.0",
      endpoints: [
        "/resource/{id}",
        "/api",
        "/tunnel/{host}/{port}",
        "/proxy/{target}",
        "/trace/{path}"
      ]
    };
  }

  // CONNECT method endpoints
  @endpoint({ connect: "/tunnel/{host}/{port}" })
  connectTunnel(host: string, port: number): TunnelResponse {
    return {
      host: host,
      port: port,
      connected: true
    };
  }

  @endpoint({ connect: "/proxy/{target}" })
  connectProxy(target: string): ProxyResponse {
    return {
      target: target,
      proxyActive: true
    };
  }

  // TRACE method endpoints
  @endpoint({ trace: "/trace/{path}" })
  tracePath(path: string): TraceResponse {
    return {
      path: path,
      receivedHeaders: [
        "X-Trace-Request",
        "X-Debug-Info",
        "User-Agent: TestClient"
      ],
      timestamp: Math.floor(Date.now() / 1000)
    };
  }

  @endpoint({ trace: "/trace" })
  traceRoot(): TraceResponse {
    return {
      path: "/",
      receivedHeaders: [
        "X-Root-Trace",
        "X-System-Check"
      ],
      timestamp: Math.floor(Date.now() / 1000)
    };
  }
}

@agent({
  mount: '/cors-agents/{agentName}',
  cors: ["https://mount.example.com"]
})
class CorsAgent extends BaseAgent {

  constructor(readonly agentName: string) {
    super();
  }

  // GET endpoint adds additional CORS on top of mount
  @endpoint({
    get: "/wildcard",
    cors: ["*"]  // union with mount CORS
  })
  wildcard(): { ok: boolean } {
    return { ok: true };
  }

  // GET endpoint inherits mount CORS if empty
  @endpoint({
    get: "/inherited"
  })
  inherited(): { ok: boolean } {
    return { ok: true };
  }

  // POST endpoint requiring preflight
  @endpoint({
    post: "/preflight-required",
    cors: ["https://app.example.com"]
  })
  preflight(body: { name: string }): { received: string } {
    return { received: body.name };
  }
}

@agent({
  mount: '/webhook-agents/{agentName}',
  webhookSuffix: '/webhook-agent'
})
class WebhookAgent extends BaseAgent {
  testServerUrl: string = "";

  constructor(readonly agentName: string) {
    super();
  }

  @endpoint({
    post: "/set-test-server-url",
  })
  setTestServerUrl(testServerUrl: string) {
    this.testServerUrl = testServerUrl
  }

  // Webhook callback dance
  @endpoint({
    post: "/test-webhook",
  })
  async testWebhook(): Promise<{ payloadLength: number }> {
    const webhook = createWebhook();
    await fetch(this.testServerUrl!, {
        method: 'POST',
        headers: {
          'Accept': 'application/json',
          'Content-Type': 'application/json'
        },
        body: JSON.stringify({ webhookUrl: webhook.getUrl() })
      });

    const data = await webhook;

    return { payloadLength: data.bytes().byteLength };
  }
}

@agent({
  mount: '/principal-agent/{agentName}',
})
class PrincipalAgent extends BaseAgent {
  constructor(readonly agentName: string) {
    super();
  }

  // only Principal
  @endpoint({ get: "/echo-principal" })
  echoPrincipal(principal: Principal): { value: Principal } {
    return { value: principal }
  }

  // Principal in between
  @endpoint({ get: "/echo-principal-mid/{foo}/{bar}" })
  echoPrincipal2(foo: string, principal: Principal, bar: number): {value: Principal, foo: string, bar: number} {
    return {value: principal,  foo: foo, bar: bar};
  }

  // Principal at the end
  @endpoint({ get: "/echo-principal-last/{foo}/{bar}" })
  echoPrincipal3(foo: string, bar: number, principal: Principal): {value: Principal, foo: string, bar: number} {
    return {value: principal, foo, bar};
  }

  @endpoint({ get: "/authed-principal", auth: true })
  authedPrincipal(principal: Principal): { value: Principal } {
    return { value: principal }
  }
}
