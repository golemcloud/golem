---
name: golem-retry-policies-go
description: "Configuring semantic retry policies for a Go Golem agent. Use when the user asks about retry policies, retry strategies, exponential backoff, transient error recovery, retry predicates, the retry subpackage, Named/Set/With, Policy or Predicate builders, jitter, MaxRetries, Clamp, AndThen, or customizing how failures are retried in a Go Golem project."
---

# Configuring Semantic Retry Policies (Go)

Golem retries retriable operations (RPC, HTTP, RDBMS, and whole-invocation traps)
according to **named policy rules**. Each rule is a `retry.NamedPolicy`: a
`retry.Policy` (the strategy) plus a `retry.Predicate` deciding when it applies and
a priority. Per operation, the runtime resolves the highest-priority rule whose
predicate matches and applies its policy.

Rules come from three tiers, and the Go `retry` subpackage
(`github.com/golemcloud/golem/sdks/go/golem/retry`) works with all of them:

- **Manifest** (`golem.yaml`) or CLI — operator-tuned base rules; the code reads
  them back.
- **Code-owned** — `retry.Set(...)` registers a rule that persists for the agent.
- **Per-call** — `retry.With(...)` overrides a rule for one call, restored on return.

## Steps

1. **Declare base rules** in `golem.yaml` (`retryPolicyDefaults`), scoped per environment.
2. **Build strategies in code** from a base policy plus fluent combinators.
3. **Apply** them: `retry.Set` for code-owned rules (often in the constructor), `retry.With` for per-call overrides.
4. **Query** active rules with `retry.GetByName` / `retry.GetPolicies` / `retry.PolicyNames`.

## 1. Define Retry Policies in the Application Manifest

Add definitions under `retryPolicyDefaults` in `golem.yaml`, scoped per
environment. This is platform-level config (shared with the other language SDKs):

```yaml
retryPolicyDefaults:
  prod:
    http-transient:
      priority: 10
      predicate:
        and:
          - propEq: { property: "error-type", value: "transient" }
          - propEq: { property: "uri-scheme", value: "https" }
      policy:
        countBox:
          maxRetries: 5
          inner:
            jitter:
              factor: 0.15
              inner:
                clamp:
                  minDelay: "100ms"
                  maxDelay: "5s"
                  inner:
                    exponential:
                      baseDelay: "200ms"
                      factor: 2.0
    catch-all:
      priority: 0
      predicate: true
      policy:
        countBox:
          maxRetries: 3
          inner:
            exponential:
              baseDelay: "100ms"
              factor: 3.0
```

Policies are evaluated in **descending priority order**; the first matching
predicate's policy wins. If none match, the built-in `default` (3 retries,
exponential backoff, clamped to [100ms, 1s], 15% jitter) applies.

## 2. Build and Apply Policies in Code

Use the `retry` subpackage. A `Policy`/`Predicate`/`NamedPolicy` is an **immutable
value**; combinators return a new value, so malformed graphs are unrepresentable.

```go
import (
	"time"

	"github.com/golemcloud/golem/sdks/go/golem/retry"
)

pol := retry.Exponential(200*time.Millisecond, 2.0).
	Clamp(100*time.Millisecond, 5*time.Second).
	WithJitter(0.15).
	OnlyWhen(retry.ErrorType.Eq("transient")).
	MaxRetries(5)

rule := retry.Named("http-transient", pol).
	WithPriority(10).
	AppliesWhen(retry.URIScheme.Eq("https"))
```

### Base policies

| Constructor | Description |
|---|---|
| `retry.Periodic(d)` | Fixed delay between attempts |
| `retry.Exponential(base, factor)` | `base × factor^attempt` — exponentially growing delays (factor finite, > 0) |
| `retry.Fibonacci(first, second)` | Fibonacci-growing delay seeded by `first`, `second` |
| `retry.Immediate()` | Retry with no delay |
| `retry.Never()` | Do not retry |

### Combinators (methods on `Policy`)

| Method | Description |
|---|---|
| `.MaxRetries(n)` | Cap the number of retries |
| `.Within(d)` | Stop retrying once a wall-clock budget elapses |
| `.Clamp(minD, maxD)` | Bound each computed delay to `[minD, maxD]` (`minD <= maxD`) |
| `.AddDelay(d)` | Add a fixed offset to each delay |
| `.WithJitter(factor)` | Randomize each delay by up to `factor` (finite, >= 0) |
| `.OnlyWhen(pred)` | Apply the strategy only when `pred` matches; otherwise give up |
| `.AndThen(next)` | Fall back to `next` once this policy stops retrying |
| `.Union(other)` | Retry if either would retry (shorter delay wins) |
| `.Intersect(other)` | Retry only if both would retry (longer delay wins) |

```go
// Exponential, clamped, jittered, capped:
retry.Exponential(200*time.Millisecond, 2.0).
	Clamp(100*time.Millisecond, 5*time.Second).
	WithJitter(0.15).
	MaxRetries(5)

// Periodic with a wall-clock budget:
retry.Periodic(time.Second).Within(60 * time.Second)

// Immediate retries, then fall back to exponential:
retry.Immediate().MaxRetries(3).
	AndThen(retry.Exponential(time.Second, 2.0).MaxRetries(5))

// Never retry:
retry.Never()
```

### Predicates

Build a predicate from a property (`retry.PropName`) using a comparison method,
then compose with `.And` / `.Or` / `.Not`:

```go
// A property comparison:
retry.ErrorType.Eq("transient")
retry.StatusCode.OneOf(500, 502, 503, 504)

// Compose:
retry.ErrorType.Eq("transient").And(retry.URIScheme.Eq("https"))

// Always / never:
retry.MatchAlways()
retry.MatchNever()
```

`PropName` comparison methods: `.Eq`, `.Neq`, `.Gt`, `.Gte`, `.Lt`, `.Lte`,
`.Exists()`, `.OneOf(...)`, `.MatchesGlob(pat)`, `.StartsWith(prefix)`,
`.Contains(sub)`. Comparison values may be a string, any integer type, or a bool.

### Available properties

Exported `retry.PropName` constants (use `retry.Prop("name")` for any other):

| Constant | Populated in |
|---|---|
| `retry.Verb` | every context (HTTP method, RDBMS/RPC verb, or `"trap"`) |
| `retry.NounURI`, `retry.URIScheme`, `retry.URIHost`, `retry.URIPort`, `retry.URIPath` | every context (decomposed resource URI) |
| `retry.StatusCode` | outgoing HTTP **response** only |
| `retry.ErrorType` | outgoing HTTP **response** only (`"transient"`, `"http-status"`) |
| `retry.Function` | worker-to-worker RPC call |
| `retry.TargetComponentID`, `retry.TargetAgentType` | worker-to-worker RPC call |
| `retry.DBType` | RDBMS operations (`postgres`, `mysql`, …) |
| `retry.TrapType` | guest WASM trap (`transient-error`, `unknown`, …) |

A policy keyed on a property **not present in the current context is silently
skipped** for that decision. So a `StatusCode`-keyed policy fires only for HTTP
responses, a `TrapType`-keyed policy only from the trap path — design one policy
per context (or use `.Or`/`.And` to match several) rather than expecting one policy
to cover everything.

### Apply: `Set` (code-owned) and `With` (per-call)

`retry.Set` registers/overwrites a rule and **persists** it for the agent (it
takes precedence over a manifest/CLI rule of the same name). Reach for it when the
code owns the rule — often in the constructor. Both `Set` and `With` **panic** on
an invalid policy (check first with `NamedPolicy.Validate()`).

```go
var codeOwned = retry.Named("code-owned",
	retry.Exponential(100*time.Millisecond, 2.0).
		Clamp(50*time.Millisecond, 2*time.Second).
		OnlyWhen(retry.StatusCode.Gte(500)).
		MaxRetries(4)).
	WithPriority(20).
	AppliesWhen(retry.TargetAgentType.Eq("InventoryAgent"))

var agent = golem.Implement(retrying.Agent, func(retrying.ID) *state {
	retry.Set(codeOwned) // register the code-owned rule when the worker starts
	return &state{}
})
```

`retry.With` applies a rule for the **current call only** and returns a `restore`
function that reinstates the previously registered rule (or removes it) — scope it
with `defer`:

```go
golem.Handle(agent, retrying.Burst, func(_ *golem.Context[state], _ golem.Unit) retrying.Snapshot {
	// Override the code-owned rule just for this call, restored on return.
	defer retry.With(retry.Named("code-owned", retry.Immediate().MaxRetries(10)).
		WithPriority(99).
		AppliesWhen(retry.TargetAgentType.Eq("InventoryAgent")))()
	return doWork()
})
```

`retry.Remove(name)` deletes a rule.

> **Concurrency.** `Set`, `Remove`, and `With` apply at the **worker level**, not
> per goroutine. `With` is safe as long as its scope does not await while other
> goroutines run concurrently; don't hold an override open across a `CallAsync`
> fan-out.

## 3. Query Policies at Runtime

```go
// Names only (cheap — does not decode bodies):
names := retry.PolicyNames()

// Fully decoded rules:
policies, err := retry.GetPolicies()
if err == nil {
	for _, p := range policies {
		_ = p.Name()     // string
		_ = p.Priority() // uint32
	}
}

// A specific rule by name:
np, found, err := retry.GetByName("http-transient")
if err == nil && found {
	_ = np.Priority()
}

// What policy the runtime would pick for a hypothetical operation:
pol, matched, err := retry.Resolve("GET", "https://api.example.com/x",
	map[string]any{"status-code": 503})
_ = pol
_ = matched
_ = err
```

`NamedPolicy` accessors: `.Name()`, `.Priority()`, `.Strategy()` (opaque `Policy`),
`.Applicability()` (`Predicate`), and `.Validate()` (returns the first lowering
error, or nil).

## 4. Live-Editing Policies via CLI

Retry policies can be managed at runtime without redeployment (takes effect
immediately for running agents):

```shell
golem retry-policy create http-transient \
  --priority 10 \
  --predicate '{ "and": [{ "propEq": { "property": "error-type", "value": "transient" } }, { "propEq": { "property": "uri-scheme", "value": "https" } }] }' \
  --policy '{ "countBox": { "maxRetries": 5, "inner": { "exponential": { "baseDelay": "200ms", "factor": 2.0 } } } }'

golem retry-policy list
golem retry-policy get http-transient
golem retry-policy update http-transient --priority 15
golem retry-policy delete http-transient
```

## 5. Default Retry Policy

When no user-defined policy matches, Golem uses the built-in `default`: priority
`0`, predicate `true`, up to 3 retries, exponential backoff (factor 3.0), delays
clamped to [100ms, 1s], 15% jitter.

## Key Constraints

- `Policy`/`Predicate`/`NamedPolicy` are immutable values; combinators return new
  values (build once as package data where possible).
- `retry.Set` and `retry.With` **panic** on an invalid policy — validate first with
  `NamedPolicy.Validate()` if the inputs are dynamic.
- Policies are defined **per-environment**; names must be unique within one; higher
  priority is evaluated first.
- A property-keyed policy is silently skipped in a context that lacks that property.
- `retry.With` is scoped to the current call — the previous rule is restored on
  return; it is worker-global, so mind concurrent awaits.

### Related Skills

| Skill | When to Load |
|-------|--------------|
| `golem-make-http-request-go` | The outgoing HTTP calls whose responses drive status-code retries |
| `golem-atomic-block-go` | Idempotence mode and atomic regions interact with retry semantics |
| `golem-call-another-agent-go` | RPC calls retried via `Function` / `TargetAgentType`-keyed rules |
| `golem-add-transactions-go` | Saga transactions layered on durable execution and retries |
