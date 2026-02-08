import {
    BaseAgent,
    Result,
    agent,
    endpoint,
    UnstructuredBinary,
} from '@golemcloud/golem-ts-sdk';
import { createWebhook } from 'golem:agent/host';

@agent({
  mount: '/http-agents/{agentName}',
  webhookSuffix: '/http-agent'
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

  // Webhook callback dance
  @endpoint({
    post: "/test-webhook",
  })
  testWebhook(): { received: string } {
    let webhook_url = createWebhook();
    return { received: body.name };
  }
}
