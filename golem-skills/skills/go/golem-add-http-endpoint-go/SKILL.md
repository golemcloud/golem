---
name: golem-add-http-endpoint-go
description: "Exposing a Go agent's methods over HTTP. Use when the user wants to add an HTTP endpoint/route to an agent, mount an agent under a URL path, make agent methods callable over HTTP, or map path/query/header parameters in a Go Golem project."
---

# Exposing a Go Agent over HTTP

## Overview

HTTP mounting is **metadata only**: you declare, on the agent's definition, a URL prefix for the agent and a route for each method. The platform routes matching requests to the right instance and method — there is no incoming-request handler to write. To handle raw requests yourself (a `net/http` handler, streaming bodies, a website), use an HTTP router instead (see `golem-http-router-go`).

Two pieces, both on the **definition**:

1. `Spec.HTTP = &golem.Mount{Path: "…"}` — the agent-level prefix. On a key mount, its `{var}` segments bind the agent's user-supplied constructor (`ID`) fields, so a request URL selects the instance. **Every user-supplied `ID` field must appear as a `{var}` on a key mount; exclude auto-injected `golem.Principal` fields.** An ID mount instead binds only the required agent ID.
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
	Value = Agent.Method[golem.Unit, int64]("value", golem.Description("Read the current value"), golem.HTTP(golem.GET("/value")))

	// POST /counters/{name}/add  — AddIn arrives as the JSON body
	Add = Agent.Method[AddIn, int64]("add", golem.Description("Add to the count"), golem.HTTP(golem.POST("/add")))
)
```

The handlers are ordinary handlers (see `golem-add-agent-go`) — nothing HTTP-specific in the impl.

## Required Agent ID Mounts and Creation

Use an ID mount to address an existing durable, non-router agent without constructor inputs in the URL:

```go
HTTP: &golem.Mount{
    Path:        "/tasks/{instance}",
    AgentID:     golem.AgentIDPath("instance"),
    CreateRoute: "/tasks/create",
},
```

For query addressing, use `Path: "/tasks"` and `AgentID: golem.AgentIDQuery("instance")`. The ID is required; there is no optional binding or keyed fallback. ID mounts cannot bind constructor fields or combine with `PhantomAgent` or ephemeral mode. The query ID name is reserved from explicit and implicit method query bindings and protocol controls. Keep fresh-per-request `PhantomAgent` and ephemeral lifecycle policies on key mounts.

`CreateRoute` declares a full exact POST path, not a mount suffix, and requires `AgentID`. Only literals and system variables are allowed in this path. Send a JSON object with one field per user-supplied constructor parameter, as an endpoint body has one field per method parameter, for example `{"name": "first"}`; an optional parameter may be left out. Other fields, such as `env`, `config` or `keyed`, are rejected with `400`. The new agent has no key. Route authentication does not grant configuration authority. Use authorized operator REST creation or SDK creation for overrides. Creation inherits mount authentication/CORS, waits for the constructor, and returns `201 Created` with `agentId`/`agentFingerprint` JSON and an ID-mounted `Location`. `Idempotency-Key` is scoped by the signed-in caller of the route: a retry with the same key from the same user returns the agent of the first attempt, and another user with the same key gets its own agent. An anonymous caller has no scope, so its key is ignored and each retry creates a new unkeyed agent.

ID selection never creates an agent. File serving supports both key and ID mounts but is lookup-only: missing keys and unknown, deleted, wrong-type or wrong-environment IDs return terminal `404`, without creation or handler fallback. Missing files on an existing target retain normal mapping fallback.

## Binding request data to input fields

- **Path variables** — `{sku}` in a route suffix binds the input field `SKU`: `golem.GET("/items/{sku}")`.
- **Query parameters** — `golem.Query("detailed", "detailed")` binds `?detailed=` to the `Detailed` field:
  ```go
  Lookup = Agent.Method[LookupIn, ItemInfo]("lookup", golem.HTTP(golem.GET("/items/{sku}", golem.Query("detailed", "detailed"))))
  ```
- **Headers** — `golem.Header("X-Tenant", "tenant")` binds the header to the `Tenant` field.
- **Body** — for body-carrying verbs (`POST`/`PUT`), user-supplied input fields not bound to path/query/header come from the JSON request body.
- **Principal** — the host injects `golem.Principal` fields in `ID` and method inputs. Do not send them in the body or bind them to path/query/header; explicit principal bindings are rejected.

## Auth & CORS

- `Mount{Auth: true}` requires auth on every endpoint; `golem.EndpointAuth(true|false)` overrides it per route:
  ```go
  golem.HTTP(golem.GET("/items/{sku}", golem.EndpointAuth(true)))
  ```
- `Mount{CORS: []string{"*"}}` sets allowed origins for the agent; `golem.EndpointCORS(patterns...)` adds patterns for one route.

## Verbs

`golem.GET`, `golem.POST`, `golem.PUT`, `golem.DELETE`, and `golem.Custom("VERB", "/path")`. `GET`/`HEAD` are bodyless — bind all user-supplied inputs from path/query/header, excluding auto-injected `golem.Principal`.

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
        MaxReadersPerStream: 8,
    }))))
```

- A slot is selected by exactly one of `Input` (an `AgentStream` input field, by the same schema name `golem.Query`/`golem.Header` bind), `Output` (an `AgentStream` field of the returned struct) or `Result: true` (the returned value itself).
- `Name` changes the public URL and OpenAPI name; the canonical name is no longer accepted.
- `ContentType` is allowed only for a direct `AgentStream[byte]` slot and must be a concrete non-text, non-JSON MIME type without parameters or wildcards. Other streams stay `application/json`. SSE still uses `text/event-stream` with base64 data.
- `MaxReadersPerStream` and `MaxAppendsPerSecond` must be positive and no greater than the installation maximums. Zero selects the registry installation defaults. Limits are per route and worker-service node, not cluster-wide.
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

- The agent must be durable and not fresh-per-request. Use either `Mount.AgentID` or a key mount whose user-supplied `ID` fields are scalars captured exactly once by `Mount.Path` (no `{*rest}`). Exclude auto-injected `golem.Principal` fields. File reads look up an existing agent and never create one.
- Mappings are tried in order; repeating a route with another path gives a fallback. A subtree route ends in `/*` and its path in `/$1`.
- Only `GET` and `HEAD` are served, with ranges and conditional requests. There is no directory listing, implicit index file or symlink following — expose only directories meant for HTTP.
- Files the constructor or a method writes (with `os.WriteFile`) are served as they change.
- `Mount.FileResponseHeaders` adds response headers, in order, to every file served: `[]golem.FileResponseHeader{{Name: "content-security-policy", Value: "default-src 'none'"}}`. Headers the platform manages itself (`content-type`, `cache-control`, `etag`, the CORS headers, …) are rejected at deploy.

## Key Constraints

- On a key mount, every user-supplied `ID` field must appear as a `{var}` in `Mount.Path`, excluding auto-injected `golem.Principal`. An ID mount binds only the agent ID. Mount segments may be literals or named variables, but never a catch-all. A trailing `{*rest}` is allowed only in an endpoint suffix.
- A method with an HTTP endpoint but no agent-level `Spec.HTTP` mount is a definition error — set the mount.
- Serving the routes requires an HTTP API deployment (see the manifest `httpApi` configuration); mounting alone only advertises the routes.

### Related Skills

| Skill | When to Load |
|-------|--------------|
| `golem-add-agent-go` | Define the agent whose methods you're exposing |
| `golem-make-http-request-go` | Make an *outgoing* HTTP request from agent code |
| `golem-multi-instance-agent-go` | The `{var}` path segments select an instance by ID |
