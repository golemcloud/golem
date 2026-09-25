---
name: golem-add-http-endpoint-go
description: "Exposing a Go agent's methods over HTTP. Use when the user wants to add an HTTP endpoint/route to an agent, mount an agent under a URL path, make agent methods callable over HTTP, or map path/query/header parameters in a Go Golem project."
---

# Exposing a Go Agent over HTTP

## Overview

HTTP mounting is **metadata only**: you declare, on the agent's definition, a URL prefix for the agent and a route for each method. The platform routes matching requests to the right instance and method — there is no incoming-request handler to write in the guest.

Two pieces, both on the **definition**:

1. `Spec.HTTP = &golem.Mount{Path: "…"}` — the agent-level prefix. Its `{var}` segments bind the agent's constructor (`ID`) fields, so a request URL selects the instance. **Every `ID` field must appear as a `{var}`.**
2. `golem.HTTP(golem.GET("/suffix"), …)` as a method option — one or more routes per method, with request data bound to the method's input fields.

## Steps

1. **Add a mount** to the agent's `Spec`: `HTTP: &golem.Mount{Path: "/counters/{name}"}`.
2. **Add routes** to methods with the `golem.HTTP(...)` option and a verb constructor.
3. **Bind request data** to input fields (path `{var}`, `golem.Query`, `golem.Header`, or the JSON body).
4. **Build**, then **deploy behind an HTTP API** so the routes are served.

## Example (definition)

```go
package counter

import "github.com/golemcloud/golem/sdks/go/golem"

type ID struct{ Name string } // bound by {name} in the mount path

type AddIn struct{ By int64 }

var Agent = golem.DefineAgent[ID](golem.Spec{
	Name:        "CounterAgent",
	Description: "A counter exposed over HTTP",
	// {name} binds ID.Name; CORS/Auth optional.
	HTTP: &golem.Mount{Path: "/counters/{name}", CORS: []string{"*"}},
})

var (
	// GET /counters/{name}/value  — options compose: Desc + HTTP in one call.
	Value = Agent.Method[golem.Unit, int64]("value", golem.Desc("Read the current value"), golem.HTTP(golem.GET("/value")))

	// POST /counters/{name}/add  — AddIn arrives as the JSON body
	Add = Agent.Method[AddIn, int64]("add", golem.Desc("Add to the count"), golem.HTTP(golem.POST("/add")))
)
```

The handlers are ordinary handlers (see `golem-add-agent-go`) — nothing HTTP-specific in the impl.

## Binding request data to input fields

- **Path variables** — `{sku}` in a route suffix binds the input field `Sku`: `golem.GET("/items/{sku}")`.
- **Query parameters** — `golem.Query("detailed", "detailed")` binds `?detailed=` to the `Detailed` field:
  ```go
  Lookup = Agent.Method[LookupIn, ItemInfo]("lookup", golem.HTTP(golem.GET("/items/{sku}", golem.Query("detailed", "detailed"))))
  ```
- **Headers** — `golem.Header("X-Tenant", "tenant")` binds the header to the `Tenant` field.
- **Body** — for body-carrying verbs (`POST`/`PUT`), input fields not bound to path/query/header come from the JSON request body.

## Auth & CORS

- `Mount{Auth: true}` requires auth on every endpoint; `golem.EndpointAuth(true|false)` overrides it per route:
  ```go
  golem.HTTP(golem.GET("/items/{sku}", golem.EndpointAuth(true)))
  ```
- `Mount{CORS: []string{"*"}}` sets allowed origins for the agent; `golem.EndpointCORS(patterns...)` overrides per route.

## Verbs

`golem.GET`, `golem.POST`, `golem.PUT`, `golem.DELETE`, and `golem.Custom("VERB", "/path")`. `GET`/`HEAD` are bodyless — bind their inputs from path/query/header.

## Durable Stream Route Customization

A method that takes or returns `golem.AgentStream`s is served as a Durable Streams URL family. `golem.DurableStreams` customizes it:

```go
type UploadIn struct {
    Chunks golem.AgentStream[byte]
    File   string
}

Upload = Agent.Method[UploadIn, int64]("upload",
    golem.HTTP(golem.POST("/upload?file={file}", golem.DurableStreams(golem.StreamRoute{
        Slots: []golem.StreamSlot{
            {Input: "chunks", Name: "data", ContentType: "application/vnd.example.media"},
            {Result: true, Name: "size"},
        },
        NoStreamDelete:      true,
        NoInvocationDelete:  true,
        MaxReadersPerStream: 8,
    }))))
```

- A slot is selected by exactly one of `Input` (an `AgentStream` input field, by the same schema name `golem.Query`/`golem.Header` bind), `Output` (an `AgentStream` field of the returned struct) or `Result: true` (the returned value itself).
- `Name` changes the public URL and OpenAPI name; the canonical name is no longer accepted.
- `ContentType` is allowed only for a direct `AgentStream[byte]` slot and must be a concrete non-text, non-JSON MIME type without parameters or wildcards. Other streams stay `application/json`. SSE still uses `text/event-stream` with base64 data.
- `NoExternalWrites`, `NoStreamDelete` and `NoInvocationDelete` remove that protocol operation (`405` with an exact `Allow` header); leaving them `false` keeps the operation.
- `MaxReadersPerStream` is 1 through 16 and applies to long-poll and SSE. `MaxAppendsPerSecond` must not be combined with `NoExternalWrites`. A limit rejection is `429` with `Retry-After: 1`; limits are per route and worker-service node, not cluster-wide.
- A stream input cannot be bound from the path, query or a header.

## Serving the Agent's Own Files

`Mount.ExposeFiles` serves files from the agent's filesystem under its mount, without calling a method:

```go
var Agent = golem.DefineAgent[ID](golem.Spec{
    Name: "MediaAgent",
    HTTP: &golem.Mount{
        Path: "/media/{name}",
        ExposeFiles: []golem.FileMapping{
            {Route: "/latest", Path: "/latest.txt"},       // one file
            {Route: "/files/*", Path: "/public/$1"},       // a directory tree
        },
    },
})
```

- The agent must be durable and not a phantom, and every `ID` field must be a scalar captured exactly once by `Mount.Path` (no `{*rest}`), so a request names exactly one agent.
- Mappings are tried in order; repeating a route with another path gives a fallback. A subtree route ends in `/*` and its path in `/$1`.
- Only `GET` and `HEAD` are served, with ranges and conditional requests. There is no directory listing, implicit index file or symlink following — expose only directories meant for HTTP.
- Files the constructor or a method writes (with `os.WriteFile`) are served as they change.

## Key Constraints

- Every `ID` field must appear as a `{var}` in `Mount.Path` (else the platform can't identify the instance); path vars may also be literals or the trailing catch-all `{*rest}`.
- A method with an HTTP endpoint but no agent-level `Spec.HTTP` mount is a definition error — set the mount.
- Serving the routes requires an HTTP API deployment (see the manifest `httpApi` configuration); mounting alone only advertises the routes.

### Related Skills

| Skill | When to Load |
|-------|--------------|
| `golem-add-agent-go` | Define the agent whose methods you're exposing |
| `golem-make-http-request-go` | Make an *outgoing* HTTP request from agent code |
| `golem-multi-instance-agent-go` | The `{var}` path segments select an instance by ID |
