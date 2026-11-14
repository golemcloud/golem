---
name: golem-streaming-agent-go
description: "Declares, implements, and calls streaming Go agent methods. Use for golem.AgentStream input or output, nested streams, ProduceStream, back-pressure, cancellation, forwarding, generated guest clients, or CLI streaming in a Go Golem project."
---

# Streaming Agent Methods in Go

A `golem.AgentStream[T]` is a sequence of values streamed between agents. It is an ordinary schema value: a method input field, a result, a struct field, an `Option`, a slice element, or the item of another stream.

```go
package pipe

import "github.com/golemcloud/golem/sdks/go/golem"

type ID struct{ Name string }

type ValuesIn struct {
	Values golem.AgentStream[int32]
}

type Batch struct {
	Label  string
	Values golem.AgentStream[int32]
}

var Agent = golem.DefineAgent[ID](golem.Spec{Name: "PipeAgent"})

var (
	Sum     = Agent.Method[ValuesIn, int64]("sum")
	Doubled = Agent.Method[ValuesIn, golem.AgentStream[int32]]("doubled")
	Forward = Agent.Method[ValuesIn, golem.AgentStream[int32]]("forward")
	Batches = Agent.Method[golem.Unit, golem.AgentStream[Batch]]("batches")
)
```

A stream input is a field of the input struct, like any other parameter. `golem.AgentStream[byte]` is a byte stream; `golem.AgentStream[golem.Binary]` streams binary chunks.

## Consume

```go
agent.Handle(pipe.Sum, func(_ *golem.Context[state], in pipe.ValuesIn) int64 {
	var total int64
	for v, err := range in.Values.All() {
		golem.Must0(err)
		total += int64(v)
	}
	return total
})
```

`Next()` returns `(value, error)`: `io.EOF` is a clean end, and a terminal failure is a `*golem.StreamError`. `All()` returns `iter.Seq2[T, error]` and yields one error before ending. `Collect()` returns the accepted prefix and an error; use it only for bounded streams. `Close()` abandons a stream you stop reading without waiting for its outcome; the producer sees it on its next write. Reading a stream that was closed or handed on, and an item that does not match the stream's schema, panic: they are bugs, not conditions to handle.

## Produce

```go
agent.Handle(pipe.Doubled, func(_ *golem.Context[state], in pipe.ValuesIn) golem.AgentStream[int32] {
	return golem.ProduceStream(func(w *golem.AgentStreamWriter[int32]) error {
		for v, err := range in.Values.All() {
			if err != nil {
				return err
			}
			if err := w.Write(v * 2); err != nil {
				return nil // the reader went away (golem.ErrReaderGone)
			}
		}
		return nil // clean end of stream
	})
})
```

- `golem.ProduceStream` runs the producer on its own goroutine and returns the reading end at once.
- Returning `golem.ErrReaderGone`, the error `Write` reports when the reader went away, stops production. Any other returned error produces a declared `StreamFailed` outcome with a typed string payload. Return `&golem.StreamError{Kind: golem.StreamFailed, Value: golem.EncodeTypedValue(value)}` to preserve a typed payload. A panic still traps the body.
- Stopping early does not close the input you were reading; call `in.Values.Close()` so its producer learns that nobody reads it.
- `Write` blocks until the consumer has room: that is the back-pressure, so nothing piles up in memory. Its only error is `golem.ErrReaderGone`; writing to a closed writer, or a value that cannot be encoded, panics.
- `golem.NewAgentStream[T]()` returns a writer and a stream for producing by hand; `defer w.Close()` ends it cleanly.
- Call `w.Finish(error)` for a declared failure. Dropping an unsettled writer is not success: normal body completion selects `StreamAbandoned`, and a trapped body selects `StreamTrapped`.
- `golem.StreamOf(items...)` is a stream of fixed values, handy for small results and tests.

## Forward

Return or pass on an unread stream to move its endpoint; no items pass through this agent:

```go
agent.Handle(pipe.Forward, func(_ *golem.Context[state], in pipe.ValuesIn) golem.AgentStream[int32] {
	return in.Values
})
```

A stream is affine: once returned, passed to a call or sent as an item, reading it again panics. A stream that was already read from cannot be forwarded: passing or returning it fails the call.

## Ordering and Errors

- Items of one stream arrive in order. Sibling streams have no ordering between them; read them from separate goroutines when one producer could block on the other's back-pressure.
- Item EOF is not proof of success: the reader waits for the completion outcome. Use `errors.As` to inspect `*golem.StreamError` and its `Kind`, `Value`, and `Message`. `errors.Is(err, context.Canceled)` matches `StreamCancelled`. Handle the error to preserve unrelated sibling streams.
- The host selects `StreamTrapped`, `StreamCancelled`, `StreamAbandoned`, `StreamInterrupted`, and `StreamResourceExhausted`; guests may author only success or `StreamFailed`. Per-item `golem.Result[T, E]` values remain ordinary items and do not end the stream.
- A decode failure panics in `Next`/`All`; it is never reported as a clean end.

## Calling Streaming Methods

Same-component calls use the method descriptors as usual:

```go
target := pipe.Agent.Get(pipe.ID{Name: "main"})
out := pipe.Doubled.MustCall(target, pipe.ValuesIn{Values: golem.StreamOf[int32](1, 2, 3)})
for v, err := range out.All() {
	golem.Must0(err)
	fmt.Println(v)
}
```

`CallAsync` works too. `Trigger` and `Schedule` panic for a method that carries a stream anywhere in its input or output: both return before the call completes, so no stream could be handed over.

Guest clients that `golem build` generates for `dependencies.agents` (see `golem-call-another-agent-go`) include stream-bearing methods, spelling each stream as `golem.AgentStream[T]`, including nested ones:

```go
import provider "golem.local/bridge/stream-provider-guest-client"

remote := provider.MustGetStreamProvider(provider.StreamProviderID{Name: "main"})
total := remote.MustSum(golem.StreamOf[int32](1, 2, 3))
doubled := remote.MustDoubled(golem.ProduceStream(func(w *golem.AgentStreamWriter[int32]) error {
	return w.WriteAll(1, 2, 3)
}))
```

The external Go client generated for `bridge.go.external` (see `golem-call-from-external-go`) calls stream-bearing methods over an invocation session, spelling each stream `bridge.AgentStream[T]`.

## CLI and HTTP

`golem agent invoke` accepts `-` as the value of exactly one direct stream parameter to read it from stdin, one value per line (`--stdin-format raw` for a direct byte stream). Streamed results are printed as they arrive; `--stdout-format raw` writes a direct byte stream as bytes. Ctrl-C detaches without cancelling the durable invocation; `--save-session` and `--resume-session` reconnect.

Over HTTP, a stream-bearing endpoint is served through the Durable Streams protocol (see `golem-durable-streams-go`). MCP does not expose methods with streams; add a bounded, stream-free method for MCP callers.

### Related Skills

| Skill | When to Load |
|-------|--------------|
| `golem-durable-streams-go` | Exposing stream-bearing methods over HTTP |
| `golem-call-another-agent-go` | Typed RPC and generated guest clients |
| `golem-invoke-agent-go` | Invoking agents from the CLI |
