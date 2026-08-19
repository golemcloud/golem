---
name: golem-add-cors-go
description: "Configuring CORS on a Go agent's HTTP endpoints. Use when the user asks to enable CORS, allow cross-origin requests, or configure allowed origins for HTTP endpoints in a Go Golem project."
---

# Configuring CORS on Go HTTP Endpoints

## Overview

CORS allowed-origin patterns are declared on the agent **definition**: mount-wide with `Mount.CORS`, or per route with `golem.EndpointCORS`. Origins declared at both levels are **unioned** for that route. CORS is metadata only — declaring it advertises the origins the platform will allow; the gateway handles preflight automatically.

## Steps

1. **Set mount-level origins** with `Mount.CORS` to cover every endpoint.
2. **Add per-endpoint origins** with `golem.EndpointCORS(patterns...)`; they union with the mount list.
3. **Use `"*"`** to allow all origins.

## Mount-level CORS

`Mount.CORS` is a `[]string` of allowed-origin patterns applied to **all** endpoints:

```go
type ID struct{ Name string }

var Agent = golem.DefineAgent[ID](golem.Spec{
	Name: "MyAgent",
	HTTP: &golem.Mount{
		Path: "/api/{name}",
		CORS: []string{"https://app.example.com"}, // all endpoints
	},
})

// GET /api/{name}/data  — allows https://app.example.com
var GetData = golem.DefineMethod[ID, golem.Unit, Data]("getData",
	golem.HTTP(golem.GET("/data")))
```

## Endpoint-level CORS

`golem.EndpointCORS(patterns...)` adds origins for one route. They are **unioned** with the mount-level list, not replaced:

```go
var Agent = golem.DefineAgent[ID](golem.Spec{
	Name: "MyAgent",
	HTTP: &golem.Mount{Path: "/api/{name}", CORS: []string{"https://app.example.com"}},
})

var (
	// Allows BOTH https://app.example.com AND * (all origins)
	GetData = golem.DefineMethod[ID, golem.Unit, Data]("getData",
		golem.HTTP(golem.GET("/data", golem.EndpointCORS("*"))))

	// Inherits mount-level only: https://app.example.com
	GetOther = golem.DefineMethod[ID, golem.Unit, Data]("getOther",
		golem.HTTP(golem.GET("/other")))
)
```

## Wildcard

Use `"*"` to allow all origins:

```go
var Agent = golem.DefineAgent[ID](golem.Spec{
	Name: "PublicAgent",
	HTTP: &golem.Mount{Path: "/public/{name}", CORS: []string{"*"}},
})
```

## CORS preflight

The platform handles `OPTIONS` preflight requests automatically for endpoints that have CORS configured — you do not declare an `OPTIONS` route yourself. The preflight response carries `Access-Control-Allow-Origin`, `Access-Control-Allow-Methods`, and `Access-Control-Allow-Headers`.

## Key Constraints

- `Mount.CORS` and `golem.EndpointCORS(patterns...)` both take origin patterns; the effective set for a route is the **union** of the two.
- An empty `CORS` means no cross-origin access is allowed for that scope.
- CORS declared in code still requires the agent to be served behind an `httpApi` deployment (see `golem-add-http-endpoint-go`).

### Related Skills

| Skill | When to Load |
|---|---|
| `golem-add-http-endpoint-go` | Set up the mount and endpoints before adding CORS |
| `golem-add-http-auth-go` | Require authentication on the endpoints |
| `golem-http-params-go` | Bind request path/query/header/body to method inputs |
