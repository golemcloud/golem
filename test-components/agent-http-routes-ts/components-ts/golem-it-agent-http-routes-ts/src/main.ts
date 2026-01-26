import {
    BaseAgent,
    Result,
    agent,
    endpoint,
    UnstructuredBinary,
} from '@golemcloud/golem-ts-sdk';

@agent({ mount: '/agents/{agentName}' })
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

  @endpoint({ post: "/unstructured-binary/{bucket}" })
  unstructuredBinary(
    bucket: string,
    payload: UnstructuredBinary
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
