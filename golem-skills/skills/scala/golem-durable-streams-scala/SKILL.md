---
name: golem-durable-streams-scala
description: "Exposes and consumes Durable Streams in Scala. Use for stream-bearing HTTP methods, Durable Streams session and slot URLs, external readers or writers, replay-safe appends, or protocol-compatible stream services."
---

# Durable Streams in Scala

Durable Streams is the resumable HTTP protocol for typed invocation stream slots. It is separate
from generic agent RPC streaming: `golem.schema.AgentStream[A]` defines the value passed between
agents, while Durable Streams exposes session and slot URLs to external readers and writers.

## Expose a Stream-Bearing Method

```scala
import golem.BaseAgent
import golem.runtime.annotations.{agentDefinition, durableStreamSlot, durableStreams, endpoint}
import golem.schema.AgentStream
import scala.concurrent.Future

@agentDefinition(mount = "/pipes/{name}")
trait Pipe extends BaseAgent:
  class Id(val name: String)

  @endpoint(method = "POST", path = "/uppercase")
  @durableStreamSlot(source = "input", slot = "input", name = "inbox")
  @durableStreamSlot(source = "output", slot = "$result", name = "outbox")
  @durableStreams(
    allowExternalWrites = true,
    allowStreamDelete = true,
    allowInvocationDelete = true,
    maxConcurrentReadersPerStream = 8,
    maxAppendRequestsPerSecondPerStream = 25
  )
  def uppercase(input: AgentStream[String]): Future[AgentStream[String]]
```

Implement lazily with `input.map(_.toUpperCase)`. Slots are top-level stream inputs/outputs.
`$result` selects a direct stream or scalar result; a record made entirely of direct streams gets
one output slot per field. Public names replace canonical URL names. Only direct
`AgentStream[golem.UByte]` slots may set `contentType`; JSON-shaped streams remain JSON. If a
method has several endpoints, set matching `endpointMethod` and `endpointPath` selectors on each
Durable Streams annotation. Add the agent to the application's `httpApi` deployment.

Paths are `<base>`, `<base>/invocations/<session>`, and
`<base>/invocations/<session>/streams/<slot>`; fork paths insert `/forks/<fork>`. PUT
creates/ensures, GET/HEAD reads, POST appends to writable inputs, and DELETE cooperatively cancels.
Create the session first unless all non-stream method arguments are in the URL.

## Protocol Rules

- A JSON POST array is a batch; wrap an array-valued item in an outer array. Byte slots use raw
  bytes. `Stream-Closed: true` appends and closes atomically; only close-only may have an empty body.
- Supply all three `Producer-Id`, `Producer-Epoch`, and `Producer-Seq` headers. After an uncertain
  response, retry exactly that tuple and body. Returned offsets/cursors are opaque strings.
- Read from `offset=-1`, or use `now` once and continue from the concrete returned offset. A 204
  long-poll or empty/up-to-date response is not EOF. Closure is up-to-date plus
  `Stream-Closed: true`; 404/410/cancellation/decoding/transport are errors.
- Slot DELETE tombstones; session DELETE cancels open streams but not arbitrary agent execution.
  Policy may remove these operations. Reader/append/body/fork limits return 429/413; honor
  `Retry-After` and do not retry impossible sizes unchanged.
- Fork creation is PUT with `Stream-Forked-From` and optional offset/sub-offset, body, and closure.
  Its source must have the same route/base, session ID, and public slot name as the target, either
  at the origin or in another fork. A Golem worker fork does not call this API: it copies
  checkpoints and producer identity, so divergent branches can collide unless explicitly given
  independent producers.

## Consume External Compatible URLs

```scala
import golem.streams.*
import scala.concurrent.{ExecutionContext, Future}

def readAll(url: String)(using ec: ExecutionContext): Future[List[String]] =
  val stream = DurableStreams.json[String](url)
  def loop(acc: List[String]): Future[List[String]] = stream.pull().flatMap:
    case Some(value) => loop(value :: acc)
    case None        => Future.successful(acc.reverse)
  loop(Nil)

val writer = DurableStreams.jsonWriter[String](
  "https://streams.example/target",
  DurableStreamProducer("orders-v1", epoch = 1)
)
val receipt = writer.append(List("one", "two"), close = true)
```

Use `DurableStreams.bytes` and `byteWriter` for byte streams. Byte and transport batch boundaries
are not application boundaries. `DurableStreamReadOptions` configures opaque checkpoint/cursor,
long-poll/SSE after catch-up, timeout, idle delay, and retry budget. Readers are ordinary affine
`AgentStream`s; call `close()` when stopping early.

Writers preserve one immutable uncertain request and expose `append`, `close`, `retryPending`,
`cancel`, `dispose`, `hasPending`, and `isClosed`. Cancellation fails the caller but keeps the
pending tuple/body. Resolve it before different data. Exact acknowledgement advances sequence;
`nextOffset = None` remains valid. Disposal does not close or undo the remote stream.

Only timeout, transport, rate-limit, and unavailable errors retry, using durable exponential
backoff and retry-after within a bounded budget. Replay reconstructs checkpoints, buffered data,
producer state, pending requests, and retry budget without repeating completed HTTP effects. Do
not wrap the calls in custom durability or atomic scopes.

Pass configured `golem.config.Secret[String]` directly as `auth = Some(config.token)`. The SDK
borrows the host capability and never reveals plaintext; do not call `get`. HTTPS is required
except when the host is exactly `localhost` or a loopback IP; a name such as `app.localhost` is not
exempt. Redirects and URL userinfo are rejected.

```shell
golem build
golem deploy --yes
```
