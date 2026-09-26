---
name: golem-call-from-external-go
description: "Invoking Golem agents from outside the platform in Go — a generated, typed Go bridge client, the CLI, HTTP endpoints, or the worker REST API. Use when the user wants to call, invoke, or drive agents from outside Golem: a Go CLI, a server, a script, curl, or any external application, in a Go Golem project."
---

# Calling Go Agents from Outside Golem

## Overview

Agents are reached from outside the platform in four ways:

1. **A generated Go bridge client** — a typed Go module per agent type, calling the Golem REST API. The usual choice for a Go program.
2. **The `golem` CLI** — `golem agent invoke` calls a method and waits for the result.
3. **HTTP endpoints** — if the agent declares an HTTP mount, any HTTP client calls its methods over plain HTTP.
4. **The worker REST API** — invoke methods and complete promises directly over HTTP against the Golem server.

The bridge client's language is independent of the agent's, so a Go program can call agents written in any language.

Throughout, the example is a `CounterAgent` whose constructor takes a `name` string, with methods `increment` (returns a count), `add`, and `value`.

## Approach 1: a generated Go bridge client

### Step 1: enable bridge generation

Add a `bridge` section to `golem.yaml`:

```yaml
bridge:
  go:
    external:
      agents: "*"                   # Generate for all agents
      # Or list specific agents:
      # agents:
      #   - CounterAgent
      #   - my-app:billing
      outputDir: ./bridge-sdk/go    # Optional custom output directory
```

`golem build --yes` then writes one Go module per agent type — e.g. `counter-agent-client/` — into the output directory (default `golem-temp/bridge-sdk/go/`), and keeps it in sync as agents change. Prefer this over running `golem generate-bridge` by hand.

### Step 2: require the generated module

Each client is its own module, named `golem.local/bridge/<client-dir>`. That path can never resolve on a module proxy, so the consuming program points a `replace` at the generated directory:

```
require golem.local/bridge/counter-agent-client v0.0.0

replace golem.local/bridge/counter-agent-client => ../golem-temp/bridge-sdk/go/counter-agent-client
```

Then `go mod tidy`. The client depends only on `github.com/golemcloud/golem/sdks/go/bridge` (the REST runtime) and `github.com/golemcloud/golem/sdks/go/core` (the shared value types) — **not** on the guest SDK, so it builds as an ordinary native Go program.

### Step 3: call the agent

```go
import (
    "context"
    "fmt"
    "time"

    "github.com/golemcloud/golem/sdks/go/bridge"
    counter "golem.local/bridge/counter-agent-client"
)

func run(ctx context.Context) error {
    // Once per process: which server, app and environment to talk to.
    if err := bridge.Configure(bridge.Configuration{
        Server:  bridge.Local(), // or bridge.Cloud(token), bridge.Custom(url, token)
        AppName: "my-app",
        EnvName: "local",
    }); err != nil {
        return err
    }

    // The id struct holds the constructor arguments that identify an instance.
    c, err := counter.GetCounterAgent(counter.CounterAgentId{Name: "my-counter"})
    if err != nil {
        return err
    }

    n, err := c.Increment(ctx) // awaits the result
    if err != nil {
        return err
    }
    fmt.Println("count:", n)

    // Fire-and-forget, or scheduled for later: both return a bridge.Receipt.
    if _, err := c.TriggerAdd(ctx, 5); err != nil {
        return err
    }
    _, err = c.ScheduleAdd(ctx, time.Now().Add(time.Hour), 5)
    return err
}
```

Every method has three forms — `M(ctx, …)`, `TriggerM(ctx, …)` and `ScheduleM(ctx, when, …)` — and every call returns an `error`: a transport failure, an error status (`*bridge.Error`, carrying the status and body) and a result that does not decode are all reported, never panicked.

`GetCounterAgent` takes options:

| Option | Effect |
|--------|--------|
| `bridge.WithConfiguration(cfg)` | Use this server/app/env instead of the one set with `bridge.Configure` |
| `bridge.WithPhantomID(id)` | Address the phantom instance with this id |
| `bridge.WithNewPhantomID()` | Address a fresh phantom instance, under a random id |
| `bridge.WithConfig(bridge.ConfigEntry{Path: []string{"retries"}, Value: 3})` | Override agent configuration; values are plain JSON |

### Generated types

Parameter and return types are generated in the client package, using the same Go spelling a Go agent would: records are structs, enums are `uint32` constants (`StatusInTransit`), flags are structs of `bool`, and variants and unions are sealed interfaces with one struct per case (`EventNote{Value: "hi"}`; a case without a payload is an empty struct). Options, results and tuples are `values.Option[T]`, `values.Result[T, E]` and `values.Tuple2[A, B]`… from `github.com/golemcloud/golem/sdks/go/core/values`; `text`, `char`, `binary`, `path` and `url` are `values.Text`, `values.Char`, `values.Binary`, `values.Path`, `values.URL`; datetimes and durations are `time.Time` and `time.Duration`.

Methods that take or return streams are not generated for Go yet — generation fails with an error naming the agent.

## Approach 2: the CLI

`golem agent invoke` calls a method on a deployed agent and blocks for the result. The agent is created automatically on first invocation. Both `golem` and `golem-cli` work for every command.

```shell
golem agent invoke <AGENT_ID> <FUNCTION_NAME> [ARGUMENTS...]
```

### Agent ID format

The agent ID is the agent type name plus its constructor parameters, in declaration order (for a Go agent, the fields of its `ID` struct):

```
AgentTypeName(param1, param2, ...)
```

`CounterAgent` takes one `name`, so its ID is `CounterAgent("my-counter")`. A singleton (`type ID struct{}`) uses empty parentheses: `CounterAgent()`. IDs may be prefixed with `env/`, `app/env/`, or `account/app/env/`.

### Examples

```shell
# No-parameter method
golem agent invoke 'CounterAgent("my-counter")' increment

# add takes one parameter (the By field of AddIn), passed positionally
golem agent invoke 'CounterAgent("my-counter")' add 5

# Read the current value
golem agent invoke 'CounterAgent("my-counter")' value

# In a specific environment
golem agent invoke 'staging/CounterAgent("my-counter")' value

# With an explicit idempotency key
golem agent invoke -i my-unique-key 'CounterAgent("my-counter")' increment

# Machine-readable output
golem agent invoke --format json 'CounterAgent("my-counter")' value
```

### CLI options

| Option | Description |
|--------|-------------|
| `-t, --trigger` | Only trigger the invocation without waiting (fire-and-forget) |
| `-i, --idempotency-key <KEY>` | Set a specific idempotency key; use `"-"` for auto-generated |
| `--no-stream` | Disable live streaming of agent stdout/stderr/log |
| `--schedule-at <DATETIME>` | Schedule the invocation (requires `--trigger`; ISO 8601) |
| `--format json` / `--format yaml` | Machine-readable output |

Method arguments use the CLI's WIT value syntax: strings quoted (`"my-counter"`), integers bare (`5`), booleans `true`/`false`, options `some(v)`/`none`, results `ok(v)`/`err(v)`, records `{ field-one: 1 }`. Each exported field of a Go input struct is one positional parameter, named by lower-camel-casing the Go field name (`AmountCents` → `amountCents`).

## Approach 3: HTTP endpoints

If the agent declares an HTTP mount, its methods are reachable over plain HTTP from any client. Declare the mount on the `Spec` and per-method routes with `golem.HTTP`:

```go
var Agent = golem.DefineAgent[ID](golem.Spec{
    Name: "CounterAgent",
    HTTP: &golem.Mount{Path: "/counters/{name}"}, // {name} binds the ID field
})

var Add = Agent.Method[AddIn, int64]("add", golem.HTTP(golem.POST("/add?by={by}")))
```

The agent must also be declared in the manifest's `httpApi` deployment. External clients then POST to the deployed route — from a Go program with the standard library:

```go
resp, err := http.Post(
    "https://my-golem.example.com/counters/my-counter/add?by=5",
    "application/json", nil,
)
```

See `golem-add-http-endpoint-go` for the full mount/route model.

## Approach 4: the worker REST API

Call methods (or complete promises) directly against the Golem server over HTTP — usable from any language, including external Go:

```shell
# Invoke and await a method
curl -X POST \
  "$GOLEM_URL/v1/components/$COMPONENT_ID/workers/$AGENT_ID/invoke-and-await?function=add" \
  -H 'Content-Type: application/json' \
  -d '{"params": [5]}'

# Complete a promise an agent is awaiting (see golem-wait-for-external-input-go)
curl -X POST \
  "$GOLEM_URL/v1/components/$COMPONENT_ID/workers/$AGENT_ID/complete" \
  -H 'Content-Type: application/json' \
  -d '{"oplogIdx": <oplog_index>, "data": [<bytes>]}'
```

`$AGENT_ID` is the same `AgentTypeName(params)` string the CLI uses. This is the mechanism a promise's `PromiseID` fields (`ComponentID`, `AgentID`, `OplogIndex`) feed into for external completion.

## Key Constraints

- `golem agent invoke` **blocks** until the agent returns; use `-t/--trigger` for fire-and-forget.
- Every invocation carries an idempotency key (auto-generated if not supplied), giving at-most-once execution even if the client retries.
- Constructor arguments in the agent ID must match the Go `ID` struct fields in order.
- A generated Go client is its own module (`golem.local/bridge/<client-dir>`); the consuming `go.mod` needs the `require` and a `replace` pointing at the generated directory.
- Generated clients do not yet cover stream-bearing methods.
- HTTP-endpoint access requires both a `Spec.HTTP` mount and an `httpApi` deployment declaration.

### Related Skills

| Skill | When to Load |
|-------|--------------|
| `golem-invoke-agent-go` | Full `golem agent invoke` reference and options |
| `golem-add-http-endpoint-go` | Expose the agent's methods over HTTP |
| `golem-wait-for-external-input-go` | Complete a promise from outside via REST |
| `golem-add-agent-go` | Define the agent being called |
