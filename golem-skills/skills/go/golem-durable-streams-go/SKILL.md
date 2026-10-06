---
name: golem-durable-streams-go
description: "Exposes stream-bearing Go agent methods over HTTP through the Durable Streams protocol. Use for golem.DurableStreams, StreamRoute and StreamSlot options, session and slot URLs, external readers or writers appending to and tailing an invocation's streams, or reading and writing any Durable Stream from inside an agent with the durablestreams package, in a Go Golem project."
---

# Durable Streams in Go

Durable Streams is the resumable HTTP protocol for the streams of one invocation. `golem.AgentStream[T]` declares the stream values of a method (see `golem-streaming-agent-go`); when that method is an HTTP endpoint, the platform gives each stream a stable URL that external clients append to, read, resume, cancel and fork.

## Expose a Stream-Bearing Method

Every HTTP endpoint whose method takes or returns an `AgentStream` is served this way. `golem.DurableStreams` customizes the route:

```go
package pipe

import "github.com/golemcloud/golem/sdks/go/golem"

type ID struct{ Name string }

type UppercaseIn struct {
	Input golem.AgentStream[string]
}

var Agent = golem.DefineAgent[ID](golem.Spec{
	Name: "Pipe",
	HTTP: &golem.Mount{Path: "/pipes/{name}"},
})

var Uppercase = Agent.Method[UppercaseIn, golem.AgentStream[string]]("uppercase",
	golem.HTTP(golem.POST("/uppercase", golem.DurableStreams(golem.StreamRoute{
		Slots: []golem.StreamSlot{
			{Input: "input", Name: "inbox"},
			{Result: true, Name: "outbox"},
		},
		MaxReadersPerStream: 8,
		MaxAppendsPerSecond: 25,
	}))))
```

```go
agent.Handle(pipe.Uppercase, func(_ *golem.Context[state], in pipe.UppercaseIn) golem.AgentStream[string] {
	return golem.ProduceStream(func(w *golem.AgentStreamWriter[string]) error {
		for line, err := range in.Input.All() {
			if err != nil {
				return err
			}
			if err := w.Write(strings.ToUpper(line)); err != nil {
				return nil
			}
		}
		return nil
	})
})
```

Add the agent to an `httpApi` deployment in `golem.yaml` (see `golem-add-http-endpoint-go`).

- A slot is selected by exactly one of `Input` (an `AgentStream` input field, by schema name), `Output` (an `AgentStream` field of the returned struct) or `Result: true` (the returned value: a stream, or a plain result published as JSON). A returned struct of streams gets one slot per field.
- `Name` replaces the canonical name in URLs and OpenAPI.
- Only a direct `AgentStream[byte]` slot may set `ContentType` (concrete, non-text, non-JSON); the others use JSON.
- `NoExternalWrites`, `NoStreamDelete` and `NoInvocationDelete` switch those operations off. `MaxReadersPerStream` is 1–16; `MaxAppendsPerSecond` cannot be combined with `NoExternalWrites`.
- A stream input cannot be bound from the path, query or a header; the other inputs can.

Mistakes in a `StreamRoute` are definition errors reported by `golem build`.

## The Protocol

```text
<base>                                                  PUT creates a generated session
<base>/invocations/<session>                            PUT / GET / HEAD / DELETE the session
<base>/invocations/<session>/streams/<slot>             PUT / GET / HEAD / POST / DELETE a slot
<base>/forks/<fork>/invocations/<session>/streams/<slot>
```

Create or ensure the session first when not every method argument comes from the path or query. Input slots accept POST only while external writes are allowed; output slots are read-only.

**Writing**
- A JSON POST body that is an array is a batch; wrap an array-valued message in another array. Byte slots take raw bytes.
- `Stream-Closed: true` appends and closes atomically; an empty body is allowed only for close-only.
- For replay-safe writes send `Producer-Id`, `Producer-Epoch` and `Producer-Seq` together, and retry an uncertain write with the identical tuple and body.

**Reading**
- Start with `GET ...?offset=-1`, or `offset=now` for the current tail. Continue from the returned `Stream-Next-Offset` (opaque; never compute it) and, for long-poll, `Stream-Cursor`.
- `live=long-poll` returns 204 while empty; `live=sse` streams SSE events. Empty or up-to-date is not EOF: up-to-date together with `Stream-Closed: true` is.
- 404, 410, cancellation, decode and transport failures are errors, not a clean close.

**Control**
- DELETE on a slot cooperatively cancels and tombstones it; DELETE on a session cancels its open slots without interrupting arbitrary agent code.
- Limits answer 429 or 413; honor `Retry-After` and do not resend an oversized body unchanged.
- A fork is created by PUT on a fork slot with `Stream-Forked-From` (plus optional `Stream-Fork-Offset`, `Stream-Fork-Sub-Offset`, initial body and closure). Its source must have the same route, session ID and public slot name. This is unrelated to forking a Golem agent.

## Clients

External programs speak the protocol over plain HTTP. The generated external Go bridge (`golem-call-from-external-go`) leaves stream-bearing methods out, so use HTTP directly for these endpoints.

From inside a Go agent, read and write any Durable Streams URL — this deployment's or an external one — with `github.com/golemcloud/golem/sdks/go/golem/durablestreams`. The host does the HTTP and authentication, and the reader's checkpoint and the writer's producer progress survive replay:

```go
type Config struct{ ExternalAuth golem.Secret[string] } // bearer token, never revealed to the agent

r := durablestreams.ReadJSON[Event](url, durablestreams.ReadOptions{
	Auth: golem.Some(ctx.Config(Agent).ExternalAuth),
})
events, err := r.Collect() // or r.Next() item by item; r.AgentStream() to return it

w, err := durablestreams.NewWriter(url, "application/json", durablestreams.WriteOptions{
	ProducerID: golem.Some("orders-1"), // stable id: a repeated append is deduplicated
	Auth:       golem.Some(ctx.Config(Agent).ExternalAuth),
})
receipt, err := durablestreams.AppendJSON(w, []Event{ev}, false) // true closes the stream
```

- `ReadBytes(url, opts)` reads a byte stream. `ReadOptions` sets the starting `Checkpoint` (default `durablestreams.Start`), the `Transport` once caught up (`LongPoll` by default, or `SSE`), the attempt `Timeout`, the `IdleDelay` and `Retry`; `r.Checkpoint()` is where a later reader can resume.
- A `Writer` has one append in flight: after an error, `RetryPending()` resends that exact append before new data is accepted. `w.Close()` closes the stream; `w.AppendBytes` writes bytes.
- Transient failures (timeouts, transport, rate limits, unavailability) are retried per `RetryOptions`; any other `*durablestreams.Error` is returned, and an error never means the end of a stream.

```shell
golem build
golem deploy --yes
```

### Related Skills

| Skill | When to Load |
|-------|--------------|
| `golem-streaming-agent-go` | Declaring, producing and consuming `AgentStream` values |
| `golem-add-http-endpoint-go` | Mounts, endpoints and the `httpApi` deployment |
| `golem-configure-api-domain` | Deploying the HTTP API |
