# Interfaces, cancellation and the execution model

**Layer:** binding: the rules for local tools, wrappers, user-defined providers, streams,
cancellation and the execution model. Reference model: the records and interfaces. Each SDK fixes
its own shape of the wrappable and provider interfaces once, because Memory and user-defined
providers build on it.

## Local tools

A local tool is a closure in the caller's code. It may capture agent state and may call another agent
through its generated client, so offering an agent method to a model needs no separate tool kind.
Other libraries construct local tools too: Memory's `tools(namespace)` returns them.

> The types on this page use the contract's WIT-like notation ([README.md § Notation](README.md#notation)). It is notation only: Golem AI defines no WIT interfaces, and each SDK writes these types by hand in its own idiom. Unless the page's Layer line says otherwise, they are the reference model ([README.md § Layers](README.md#layers)): each SDK exposes their information in its own API design.

```text
record local-tool {
  name: string,                          // the model-facing name, as given (tool-loop.md#names)
  description: string,
  input: schema,                         // the SDK's schema for the argument type
  output: option<schema>,                // the SDK's schema for the result type; json when absent
  handler: func(args: T, signal) -> result<R, tool-error>,
}
```

- `input` and `output` are the SDK's own schema values (the same derivation as structured output).
  Their JSON Schema comes from the SDK's existing renderer (plan D10).
- The handler receives the decoded arguments and the invocation's cancellation signal in the SDK's
  form. It returns a value or a `tool-error` (`{ error, payload }`). Both go back to the model.
- A handler that fails in any other way ends the run with `AiError` `foreign` whose cause is the
  handler's error. Cancellation propagates as cancellation.
- Arguments that do not decode against `input` go back to the model as the tool error
  `{ "error": "invalid-arguments", "payload": <message> }` \[PROP\]; the handler is not called.
- `agentTool(client.method, { name?, description? })` \[PROP\] is sugar for a local tool that calls
  one agent method through its client and forwards the signal.

## The wrappable `Llm` and `Embedding`

`Llm` and `Embedding` are interfaces in every SDK: an interface, trait, protocol or record of
functions, whichever is the SDK's idiom. A wrapper implements the interface and holds an inner value.
The fallback composite and Memory's wrapper are wrappers; code may write its own.

```text
interface llm {
  run:            func(input, llm-options, signal) -> result<llm-result, ai-error>
  run-with:       func(input, llm-options, signal) -> result<llm-result, ai-error>
  stream:         func(input, llm-options, signal) -> stream<llm-event>
  stream-with:    func(input, llm-options, signal) -> stream<llm-event>
  estimate-tokens: func(text: string) -> u64
  count-tokens:   func(input, llm-options, signal) -> result<u64, ai-error>
  describe:       func() -> list<model-ref>     // the models a call may use, in fallback order
}

interface embedding {
  embed:  func(list<embed-input>, embed-options, signal) -> result<embed-result, ai-error>
  rerank: func(rerank-request, signal) -> result<rerank-result, ai-error>
  describe: func() -> list<model-ref>
}
```

Rules:

- Every call form is wrappable: exact and bound calls, structured results, streams, toolsets,
  cancellation and recovery all pass through a wrapper unchanged. A wrapper that does not change a
  call form delegates it.
- `runWith` with an empty toolset is a valid call: plain generation. Memory's internal calls use it.
- A bound-form call through a wrapper offers the bound toolset of the **calling** owner, resolved
  where the call starts, plus the local tools passed. A wrapper adds no tool implicitly.
- `describe` lets the run-start check (Rule 8) and attribution see every model a wrapped call may use.
  A wrapper returns its inner value's list.
- A wrapper that fails on its own raises `AiError` `foreign` with its error as the cause.
- A wrapper sees one logical exchange per call ([operations.md](operations.md#the-model-call-and-the-logical-exchange)).
  Recovery and retries happen below it.
- `Shared<Llm>` fields and `Llm` values are both accepted wherever a model is expected (composition,
  wrappers, the voice package). A field is read at each call; a value stays pinned while held.

## User-defined providers

A user-defined provider implements one or more capability interfaces against the standard request and
stream-event types of this contract. It:

- uses the SDK's ordinary HTTP or WebSocket client;
- receives its credential only as a 5.4 auth value (a `Secret` used through `bearer()`, `disguise()`
  or an auth constructor), never as plaintext (Rule 1);
- reports usage in the result types, which the library commits like any built-in provider's;
- returns `AiError` kinds from [errors.md](errors.md), with `unsupported` for calls it cannot make;
- publishes as an ordinary library.

Code constructs the model from the provider's own config field: `myProvider.llm(this.config.mine.get())`
\[PROP spelling\]. The field's record type holds the credential as a `Secret` and the quota as a
`Quota` (Rule 2). The constructed model is an ordinary model of its capability: pinning, fallbacks,
memory, the tool loop, quota, recovery and every other rule here apply to it. The library, not the
provider, runs the tool loop, the quota calls and recovery.

```text
interface llm-provider {
  // One provider request: one step, no tool execution. The library drives the loop.
  send:   func(provider-request, signal) -> result<provider-response, ai-error>
  stream: func(provider-request, signal) -> stream<provider-event>
  limits: func() -> tool-limits                 // tool-loop.md#provider-limits
}

record provider-request {
  model: string,
  messages: list<message>,
  system: option<string>,
  tools: list<tool-declaration>,                // name, description, JSON Schema of the input
  tool-mode: tool-mode,
  output: option<json-schema>,
  continuation: option<string>,                 // the delivered prefix when this request continues a broken answer
}

record provider-response {
  text: string,
  tool-calls: list<tool-call>,
  finish: finish-reason,
  usage: llm-usage,
}

variant provider-event {
  text-delta(string),
  tool-call(tool-call),                          // complete
  finish(provider-response),                     // text and tool calls repeated in full
}
```

`transcription-provider`, `speech-provider`, `image-provider` and `embedding-provider` follow the same
pattern: one provider request per call, the operation's request and result types from
[operations.md](operations.md), and the library around them.

## Streams

Every `stream<T>` in this contract is the SDK's 4.4 stream type:

| SDK | Stream type | Failure | Cancellation |
| -- | -- | -- | -- |
| Rust | items of `Result<T, E>` (4.4) | an `Err` item carrying the `AiError`, then the end | an `Err` item carrying the SDK's `Cancelled`, then the end |
| MoonBit | items of `Result[T, E]` (4.4) | an `Err` item carrying the `AiError`, then the end | an `Err` item carrying the SDK's `Cancelled`, then the end |
| Go | `iter.Seq2[T, error]` | a non-nil `error`, then the end | an error with `errors.Is(err, context.Canceled)` |
| TypeScript | the SDK's async-iterable stream | a rejection with the `AiError` | a rejection with `AbortError` |
| Scala | the SDK's stream | a failure with the `AiError` | a failure with `AbortError` |
| Effect | `Stream<T, AiError>` | the stream's error channel | fiber interruption |
| Python | from 8.1 | from 8.1 | from 8.1 |

A stream returned from a method is an ordinary Golem stream (4.4): HTTP and Durable Streams consumers
read it, and a tool command can return it to MCP.

## Cancellation

Every operation takes the invocation's cancellation signal in the SDK's form (1.2 TC8, TC16):

| SDK | How the operation gets the signal | How the operation ends when cancelled |
| -- | -- | -- |
| TypeScript | `signal` from the method's context (the second argument), passed in the operation's options | rejects with `signal.reason` or an `AbortError` |
| Rust | the injected `InvocationContext` (`ctx.signal()`), passed to the operation | returns the SDK's `Cancelled` error |
| Scala | the injected `InvocationContext`, passed explicitly (race helper \[PROP\]) | fails with `AbortError` |
| MoonBit | the injected `InvocationContext` | returns the SDK's `Cancelled` error |
| Go | `ctx context.Context`, the first parameter | returns an error with `errors.Is(err, context.Canceled)` |
| Effect | fiber interruption, applied automatically | the fiber is interrupted |
| Python | from 8.1 | from 8.1 |

Rules for every SDK:

- **The library never swallows cancellation** (1.2 O7). A cancelled operation ends in the form above,
  never with a partial result returned as success and never as an `AiError`. In Rust and MoonBit the
  library's error type keeps `Cancelled` apart from `AiError` and converts into any method error type
  `E: From<Cancelled>`, so `?` ends the method as `cancelled`.
- The library passes the signal into its HTTP and WebSocket reads, into quota waits it controls, into
  every bound tool call and into every local tool handler.
- In Rust, MoonBit, Effect and Go, dropping or interrupting an awaited operation also cancels it, and
  an in-flight agent call made by a local tool is cancelled the same way (TC8). In TypeScript and
  Scala, cancellation reaches an in-flight agent call only through the passed signal, which the loop
  and `agentTool` forward.
- A cancel request does not abort an in-flight `wasi:http` or `golem:websocket` host call (1.2 X16).
  The library stops reading and drops it; the provider may still finish and bill the request. The
  call commits the reserved amount ([usage-and-quota.md](usage-and-quota.md#commit-after-the-call)).
- A cancelled stream sends nothing more to the provider and closes its socket.

## Execution model

- All work of an operation runs on the calling invocation's task (1.2 Rule 18): provider requests, SSE
  readers, WebSocket loops, timers, retry back-off, quota calls and tool-loop steps. The host
  attributes each host call to the invocation whose task makes it.
- No reader, socket or stream is shared between invocations or kept in agent state across them.
- A stream that outlives the method's return keeps its attribution but loses lent cards (1.2 TC11).
  A provider call it makes after the return that needs a lent card fails.
- Provider clients are not set up with authority at initialization or Wizer time; initializer host
  calls belong to no invocation. An Effect layer makes no host call while it is built (GOL-1211).
- Each provider call emits one tracing span (7.1 PF11, GOL-1202).
- Libraries ship for serial agents. On a serial agent a background stream or a passive wait for
  WebSocket input holds the next call.
- Tool calls of one step run one after another, in the order the model returned them \[PROP\]. A Go
  library joins every goroutine it starts before the step returns.
- The TypeScript, Effect and Scala libraries install no promise hook of their own (1.2 TC15).
