---
name: golem-add-http-auth-go
description: "Requiring authentication on a Go agent's HTTP endpoints. Use when the user asks to add auth, require authentication, or protect HTTP endpoints in a Go Golem project."
---

# Requiring Authentication on Go HTTP Endpoints

## Overview

Golem authenticates HTTP endpoints via OIDC providers. Auth is declared on the agent **definition** — mount-wide with `Mount.Auth`, or per route with `golem.EndpointAuth` — and then wired to a security scheme in the manifest's (`golem.yaml`) `httpApi` deployment. Declaring auth in code without a matching deployment configuration only advertises the requirement; serving it needs the deployment.

## Steps

1. **Require auth** at the mount (`Mount{Auth: true}`) or per endpoint (`golem.EndpointAuth(true)`).
2. **Override where needed** — a per-endpoint `golem.EndpointAuth(...)` beats the mount default.
3. **Configure a security scheme** in `golem.yaml` under the `httpApi` deployment.

## Auth on all endpoints (mount level)

Set `Auth: true` on the `Mount` to require authentication for every endpoint of the agent:

```go
type ID struct{ Name string }

var Agent = golem.DefineAgent[ID](golem.Spec{
	Name: "SecureAgent",
	HTTP: &golem.Mount{Path: "/secure/{name}", Auth: true}, // all endpoints require auth
})
```

## Auth on individual endpoints

With no mount-level default, require auth on a specific route with `golem.EndpointAuth(true)`:

```go
var Agent = golem.DefineAgent[ID](golem.Spec{
	Name: "ApiAgent",
	HTTP: &golem.Mount{Path: "/api/{name}"},
})

var (
	// GET /api/{name}/public  — open
	Public = Agent.Method[golem.Unit, string]("public", golem.HTTP(golem.GET("/public")))

	// GET /api/{name}/private  — requires auth
	Private = Agent.Method[golem.Unit, string]("private", golem.HTTP(golem.GET("/private", golem.EndpointAuth(true))))
)
```

## Overriding the mount default

A per-endpoint `golem.EndpointAuth` overrides the mount's `Auth`. Use `false` to open a single route on an otherwise-protected agent:

```go
var Agent = golem.DefineAgent[ID](golem.Spec{
	Name: "MostlySecureAgent",
	HTTP: &golem.Mount{Path: "/api/{name}", Auth: true}, // default: auth required
})

var (
	// GET /api/{name}/health  — override to open
	Health = Agent.Method[golem.Unit, string]("health", golem.HTTP(golem.GET("/health", golem.EndpointAuth(false))))

	// GET /api/{name}/data  — inherits the mount default (auth required)
	GetData = Agent.Method[golem.Unit, Data]("getData", golem.HTTP(golem.GET("/data")))
)
```

## Reading the caller

Read who is calling with `ctx.Caller()`. `Original` is the principal that started the call chain (the signed-in user for an authenticated request); `Immediate` is the principal that made this call. The caller is not an input: it is never mapped from the path, query, headers or body, and callers send nothing for it.

```go
var Profile = Agent.Method[golem.Unit, string]("profile", golem.HTTP(golem.GET("/profile")))

agent.Handle(Profile, func(ctx *golem.Context[state], _ golem.Unit) string {
	switch p := ctx.Caller().Original.(type) {
	case golem.UserPrincipal:
		return "hello " + p.Pair.Subject
	default:
		return "hello stranger"
	}
})
```

The principal is one of `golem.UserPrincipal` (a signed-in user; `Claims` holds the claim set as JSON text), `golem.AccountPrincipal` (a Golem account, e.g. the CLI), `golem.AgentPrincipal` (another agent), `golem.ToolPrincipal` (a tool) and `golem.AnonymousPrincipal`. `ctx.Caller()` also works in the constructor's `*golem.InitContext`.

To remember who called (for example the creator of the agent), store `ctx.Caller().Original.Ref()`, a `golem.PrincipalRef`, in the state, and compare it later with `EqualsRef`. A `golem.Principal` or `golem.Caller` field in an input struct or the `ID` is a definition error.

## Deployment configuration

After declaring auth in code, configure a security scheme in `golem.yaml` under the `httpApi` deployment (this is where `subdomain` versus `domain` is chosen). Quick reference:

```yaml
httpApi:
  deployments:
    local:
    - subdomain: my-app  # resolves to my-app.localhost:9006 by default
      agents:
        SecureAgent:
          securityScheme: my-oidc            # production OIDC
        # or for development:
        # SecureAgent:
        #   testSessionHeaderName: X-Test-Auth
```

## Key Constraints

- `Mount.Auth` is a plain `bool`; `golem.EndpointAuth(required bool)` takes an explicit value (`true` to require, `false` to open).
- Setting `golem.EndpointAuth` **more than once** on one endpoint is a definition error, not a silent overwrite — set it exactly once per route.
- An endpoint with no `golem.EndpointAuth` inherits the mount's `Auth`.
- Declaring auth requires an `httpApi` deployment with a security scheme to actually enforce it — mounting alone only advertises the requirement.

### Related Skills

| Skill | When to Load |
|---|---|
| `golem-add-http-endpoint-go` | Set up the mount and endpoints before adding auth |
| `golem-add-cors-go` | Allow cross-origin requests to the endpoints |
| `golem-http-params-go` | Bind request path/query/header/body to method inputs |
