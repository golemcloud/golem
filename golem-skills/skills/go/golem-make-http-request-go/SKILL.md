---
name: golem-make-http-request-go
description: "Making outgoing HTTP requests from a Go agent. Use when the user wants an agent to call an external API/service, fetch a URL, or make an outbound HTTP/HTTPS request in a Go Golem project."
---

# Outgoing HTTP Requests from a Go Agent

## Overview

Use the **standard library `net/http`** package directly. The SDK routes `net/http` through Golem's durable `wasi:http` transport, so each request is recorded in the oplog and **served from it on replay** after a restart rather than re-issued — outbound calls are effectively exactly-once across crashes/restarts. No manual wiring or custom transport is needed.

## Steps

1. **Import `net/http`** (and `io` to read the body).
2. **Make the request** — `http.Get(url)`, or build an `*http.Request` and use `http.DefaultClient.Do`.
3. **Read the response body**, then `Close` it.
4. Let failures propagate (panic) — Golem retries transparently; do **not** add manual retry loops.

## Example

```go
package impl

import (
	"io"
	"net/http"
	"os"

	"myapp/agents/fetcher"

	"github.com/golemcloud/golem/sdks/go/golem"
)

type state struct{}

var agent = golem.Implement(fetcher.Agent, func(fetcher.ID) *state { return &state{} })

func init() {
	golem.Handle(agent, fetcher.Fetch, func(_ *golem.Context[state], in fetcher.FetchIn) string {
		url := "https://api.example.com/thing?q=" + in.Query
		// golem.Must panics on error (the SDK turns the panic into an agent-error
		// and retries per policy). Use it for calls that shouldn't fail normally.
		resp := golem.Must(http.Get(url))
		defer resp.Body.Close()
		return string(golem.Must(io.ReadAll(resp.Body)))
	})
}
```

`golem.Must(v, err)` returns `v` or panics on `err` — the fail-loud idiom for operations Golem should retry. To surface an HTTP failure to the caller as a *value* instead, return a `golem.Result` and inspect the error yourself (see `golem-add-agent-go` → Returning Failures).

## POST / custom requests

```go
req := golem.Must(http.NewRequest("POST", url, strings.NewReader(body)))
req.Header.Set("Content-Type", "application/json")
resp := golem.Must(http.DefaultClient.Do(req))
defer resp.Body.Close()
```

## Durability & retries

- The HTTP call is a durable operation: on replay the recorded response is returned, so the external service is **not** hit again.
- **Do not** write manual retry/backoff loops — Golem retries failed operations automatically (a default policy applies; customize with the retry-policy helpers only when the *strategy* must change).

## Key Constraints

- Target is **WASM**: there are no raw sockets. `net.Dial`, most database drivers, and custom transports won't work — outgoing traffic goes through the SDK's WASI-backed `net/http` transport.
- **Async handles cannot outlive an invocation**: read/consume the response within the same handler; don't stash an unresolved response/body in agent state to use later.

### Related Skills

| Skill | When to Load |
|-------|--------------|
| `golem-add-http-endpoint-go` | Expose *this* agent over HTTP (incoming requests) |
| `golem-call-another-agent-go` | Call another Golem agent instead of an external HTTP service |
| `golem-add-agent-go` | Define the agent making the request |
