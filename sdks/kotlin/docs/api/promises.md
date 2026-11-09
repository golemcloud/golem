# Promises and webhooks

> `Promises` / `Webhooks` — durable one-shot promises (`golem:api/host@1.5.0`) and webhook URLs that complete them (`golem:agent/host@2.0.0`). **Status:** 🟢 Available; verified on a real executor.

A promise is a value some other party completes with bytes while the creating agent waits for it. The wait is durable: the host suspends the agent and the promise survives restarts.

`PromiseId`, `Promises`, `Webhooks` and `WebhookException` live in `cloud.golem.runtime`.

```kotlin
val promiseId: PromiseId = Promises.create()          // owned by this agent
// hand promiseId to whoever decides: another agent (it is a plain data class, so it can be an RPC argument)…
// …or to an external system through a webhook:
val url: String = Webhooks.create(promiseId)
val bytes: ByteArray = Promises.await(promiseId)      // suspends until completed
```

- `PromiseId(agentId: AgentId, oplogIdx: Long)` is a schema-mappable data class (`oplogIdx` is an unsigned 64-bit index, held in a Kotlin `Long`), so it can be a method parameter or return value of an agent and a
  `@RemoteAgent` interface.
- `Promises.complete(promiseId, data): Boolean` completes a promise from any agent; `false` means it was already completed.
- `Promises.await(promiseId)` can only be called by the agent that created the promise.
- `Webhooks.create(promiseId)` returns a URL; a POST to it completes the promise with the request body. The agent type must be mounted (`@Agent(mount = …, webhookSuffix = "/incoming")`), deployed through an
  HTTP API, and must be the type that created the promise, otherwise the host refuses (`WebhookException`; its `permissionDenied` flag is true when the host denied permission).

The `human-in-the-loop` template (`golem new` → Kotlin) is a complete example: a `WorkflowAgent` creates a promise, passes it to a `HumanAgent` over RPC and awaits it; the human's
`decideApproval` completes it.

Verified on a real 1.6.0 executor with that template: the workflow stayed suspended until the human decided, and both an `approved` and a `rejected` decision reached the workflow. Webhook creation needs an
HTTP deployment; it was verified separately on a real executor (`WebhookCaller` with `@Agent(mount = …, webhookSuffix = "/incoming")` deployed under an `httpApi` deployment: the URL from `Webhooks.create` was POSTed with `curl` and `Promises.await` returned the body).

See also: [Host API](host-api.md) · [RPC](rpc.md) · [SDK README](../../README.md).
