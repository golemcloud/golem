---
name: golem-http-router-go
description: "Handling raw HTTP requests in Go with an HTTP router — mounting a net/http handler (ServeMux or any router), streaming request and response bodies, serving static files, and providing an OpenAPI document. Use when the user wants custom HTTP handling, to port an existing net/http service, to serve a website or files, or to stream HTTP bodies in a Go Golem project."
---

# HTTP Routers in Go

## Overview

An **HTTP router** handles raw requests under a mount, the way an HTTP server does. Unlike an agent's HTTP endpoints (see `golem-add-http-endpoint-go`), where the platform maps a request onto a method call, a router receives the request exactly as the client sent it and streams back its own response.

A router is its own kind of agent: it has no constructor parameters, a fresh instance serves each request, and it takes no snapshots. It cannot be called like an ordinary agent, but it can call ordinary agents.

## Mounting a `net/http` handler

Any `http.Handler` works — `http.ServeMux`, a third-party router, or an existing service:

```go
package routers

import (
    "io"
    "net/http"

    "github.com/golemcloud/golem/sdks/go/golem"
)

var Site = golem.DefineHTTPRouter(golem.RouterSpec{
    Name:  "Website",
    Mount: "/web",                    // literal segments only
    Auth:  false,                     // auth and CORS belong to the mount
    CORS:  []string{"*"},
})

func init() {
    mux := http.NewServeMux()
    mux.HandleFunc("POST /web/echo", func(w http.ResponseWriter, r *http.Request) {
        io.Copy(w, r.Body)            // streams in both directions
    })
    Site.Handle(mux)
}
```

Blank-import the package from the component's `main.go`, and add the router to an `httpApi` deployment in `golem.yaml` (see `golem-configure-api-domain`):

```yaml
httpApi:
  deployments:
    local:
      - domain: localhost:9006
        scheme: http
        agents:
          Website: {}
```

### What the handler sees

- `r.URL.Path` is the **full public path**, mount included (`/web/echo`). To route relative to the mount, wrap the mux: `Site.Handle(http.StripPrefix("/web", mux))`.
- `r.URL.RawPath`, `r.URL.RawQuery` and `r.RequestURI` keep the original escapes; `r.Host` is the authority; `r.TLS` is non-nil for https.
- `r.Body` streams the request body; it is read at most once.

### Writing the response

- The status and headers are sent on the first `Write`, `WriteHeader` or `Flush`, or when the handler returns. Header changes after that are ignored, as in `net/http`.
- The handler may keep writing after the head is sent; each `Write` streams out with backpressure.
- Repeated headers stay separate (`w.Header().Add("Set-Cookie", …)` twice). Names are sent lowercase.
- A missing `Content-Type` is sniffed from the first bytes; `204`, `304` and HEAD responses carry no body.
- A panic **before** the head is sent fails the request. After it, the body ends where the handler stopped — set `Content-Length` when a client must detect truncation.
- The host owns framing: never set `Transfer-Encoding`.

## The raw envelope

When `net/http`'s view loses something you need — exact header bytes, header order, or an absent vs empty query — use `HandleRaw`:

```go
Site.HandleRaw(func(ctx context.Context, req golem.HTTPRequest) golem.HTTPResponse {
    // req.Method, req.Path (full, still escaped), req.Query (golem.Option[string]),
    // req.Headers ([]golem.HTTPHeader{Name, Value []byte}), req.Body (golem.AgentStream[[]byte])
    return golem.HTTPResponse{Status: 200, Headers: nil, Body: req.Body}
})
```

A router has one handler: either `Handle` or `HandleRaw`.

## Configuration

Declare a config type with `DefineConfiguredHTTPRouter` and read it from the request's context:

```go
type SiteConfig struct {
    Greeting string
    ApiKey   golem.Secret[string]
}

var Site = golem.DefineConfiguredHTTPRouter[SiteConfig](golem.RouterSpec{Name: "Website", Mount: "/web"})

mux.HandleFunc("GET /web/hello", func(w http.ResponseWriter, r *http.Request) {
    io.WriteString(w, Site.Config(r.Context()).Greeting)
})
```

In a raw handler or OpenAPI provider, pass its `ctx`. Calling `Config` with any other context panics. Config values are set in `golem.yaml` under `agents: <RouterName>: config:` (see `golem-add-config-go`).

## Static files

`StaticFiles` serves files of the component — provisioned with the manifest's `files` (see `golem-add-initial-files`) — before the handler is consulted:

```go
var Site = golem.DefineHTTPRouter(golem.RouterSpec{
    Name:  "Website",
    Mount: "/web",
    StaticFiles: []golem.FileMapping{
        {Route: "/", Path: "/site/index.html"},
        {Route: "/assets/*", Path: "/site/assets/$1"},
    },
})
```

A matching GET or HEAD is answered from the file; a missing file falls through to the handler. A router may have static files and no handler at all. To serve files an ordinary agent writes at runtime, use `Mount.ExposeFiles` instead (see `golem-add-http-endpoint-go`).

## OpenAPI

Register a provider returning an OpenAPI 3.1.0 JSON document whose paths are relative to the mount; the host places it under the mount and merges it into the deployment's `openapi.json`:

```go
Site.OpenAPI(func(ctx context.Context) string {
    return `{"openapi":"3.1.0","info":{"title":"Site","version":"1"},"paths":{"/echo":{"post":{"responses":{"200":{"description":"echo"}}}}}}`
})
```

## Key Constraints

- `Mount` has literal segments only — a router has no identity to capture in the path.
- One handler per router (`Handle` or `HandleRaw`) and at most one OpenAPI provider.
- Auth and CORS are set on the `RouterSpec`, not per route.
- Code-first endpoints of ordinary agents and file mappings take precedence over a router's handler for the paths they match.

### Related Skills

| Skill | When to Load |
|-------|--------------|
| `golem-add-http-endpoint-go` | Map requests onto ordinary agent methods instead; expose an agent's live files |
| `golem-configure-api-domain` | Deploy routers under an `httpApi` domain |
| `golem-add-config-go` | Declare and set config values |
| `golem-call-another-agent-go` | Call ordinary agents from a router |
