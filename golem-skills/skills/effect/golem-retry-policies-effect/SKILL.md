---
name: golem-retry-policies-effect
description: "Configures Golem host-operation retry policies for Effect-based agents and distinguishes them from Effect typed failures and Schedule retries. Use for retry policies, exponential backoff, transient recovery, retry predicates, Retry.withPolicy, Retry.setPolicy, Effect.retry, or retry-policy manifests."
---

# Configuring Retry Policies in Effect Golem Applications

Golem host-operation retry policies and Effect failure handling solve different problems:

- `Retry.setPolicy` and `Retry.withPolicy` configure **Golem host-managed retries** for matching
  operations. These retries participate in durability and the oplog.
- `Effect.fail`, `Effect.catchTag`, and `Effect.retry` handle **application-level typed failures**
  inside the current Effect execution. They do not configure the Golem host.

Use the host policy for durable retries of HTTP, RPC, database, and other host operations. Use the
Effect error channel for expected domain failures and application-specific recovery. Do not add
manual retry loops around host operations.

## Define Policies in `golem.yaml`

Add defaults under `retryPolicyDefaults`, scoped by environment:

```yaml
retryPolicyDefaults:
  local:
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

Policies are checked in descending priority order. The first matching predicate wins. A property
that is absent from the current operation context does not match.

### Manifest Policy Vocabulary

Base policies are `periodic`, `exponential`, `fibonacci`, `immediate`, and `never`. Combinators are
`countBox`, `timeBox`, `clamp`, `addDelay`, `jitter`, `filteredOn`, `andThen`, `union`, and
`intersect`.

Predicates include `true`, `false`, `propEq`, `propIn`, `propGte`, `propLt`, `and`, `or`, and `not`.
Common context properties are:

- Every context: `verb`, `noun-uri`, `uri-scheme`, `uri-host`, `uri-port`, `uri-path`
- HTTP responses: `status-code`, `error-type`
- Agent-to-agent RPC: `function`, `target-component-id`, `target-agent-type`
- RDBMS operations: `db-type`
- Guest traps: `trap-type`

`error-type` is `transient` for transient transport failures and `http-status` for an HTTP status
matched by a status-code policy.

### HTTP Status Retries Are Opt-In

To retry HTTP responses such as 502, 503, and 504, the predicate itself (or a nested
`filteredOn`) must explicitly reference `status-code`. A catch-all policy does not retry HTTP
statuses. Effect `HttpClient` normally returns 4xx and 5xx responses; when an eligible host policy
matches, Golem transparently re-sends the request and `HttpClient` receives the final response.

Status retries require live execution, a reconstructible request, and an idempotent operation.
They are skipped inside an atomic host region. Prefer GET and other naturally idempotent methods
unless the operation has explicitly been made safe to retry.

## Build a Host Policy with the Effect SDK

Import Effect APIs from `effect` and Golem APIs from `@golemcloud/effect-golem`:

```typescript
import { Duration, Effect } from "effect";
import { Retry } from "@golemcloud/effect-golem";

const gatewayFailure = Retry.Predicate.oneOf(
  Retry.Props.statusCode,
  [502, 503, 504],
);

const httpTransient = Retry.NamedPolicy.named(
  "http-transient",
  Retry.Policy.exponential(Duration.millis(200), 2)
    .clamp(Duration.millis(100), Duration.seconds(5))
    .withJitter(0.15)
    .onlyWhen(gatewayFailure)
    .maxRetries(5),
).priority(10);
```

The TypeScript SDK uses camelCase (`statusCode`, `maxRetries`, `withJitter`) even though manifest
and CLI JSON use names such as `status-code`, `countBox`, and `jitter`.

### Policy and Predicate Builders

The Effect SDK exposes these policy builders:

```typescript
Retry.Policy.immediate();
Retry.Policy.never();
Retry.Policy.periodic(Duration.seconds(1));
Retry.Policy.exponential(Duration.millis(200), 2);
Retry.Policy.fibonacci(Duration.millis(100), Duration.millis(200));

Retry.Policy.exponential(Duration.millis(200), 2)
  .maxRetries(5)
  .within(Duration.seconds(60))
  .clamp(Duration.millis(100), Duration.seconds(5))
  .addDelay(Duration.millis(50))
  .withJitter(0.15)
  .onlyWhen(Retry.Predicate.eq(Retry.Props.errorType, "transient"))
  .andThen(Retry.Policy.periodic(Duration.seconds(1)))
  .union(Retry.Policy.immediate())
  .intersect(Retry.Policy.never());
```

Predicate constructors are `always`, `never`, `eq`, `neq`, `gt`, `gte`, `lt`, `lte`, `exists`,
`oneOf`, `matchesGlob`, `startsWith`, and `contains`. Compose a predicate with its `.and(...)`,
`.or(...)`, and `.not()` methods. Use `Retry.Props` constants instead of spelling host property
names in SDK code.

`NamedPolicy.named(name, policy)` defaults to priority 0 and an always-matching outer predicate.
Use `.priority(number)` and `.appliesWhen(predicate)` to change those fields. Use
`Policy.onlyWhen(predicate)` when filtering a policy subtree.

## Choose Persistent or Scoped Host Installation

`Retry.setPolicy` adds or overwrites a policy for the current durable agent and persists the
change to its oplog:

```typescript
const installPolicy = Retry.setPolicy(httpTransient);

// In an Effect.gen block, including defineAgent(...).implement(...) setup:
yield* Retry.setPolicy(httpTransient);
```

Installing during `defineAgent(...).implement(...)` setup makes the policy available to all
handlers of that agent instance. It remains active until overwritten or removed.

Use `Retry.withPolicy` when the policy should apply only while one Effect runs:

```typescript
const result = yield* Retry.withPolicy(httpTransient, doHostWork);
```

`Retry.withPolicy` restores the previous policy with the same name, or removes the temporary
policy when none existed, on success, typed failure, or interruption. A policy introduced only by
`withPolicy` is therefore not visible after the wrapped Effect completes.

Do not wrap implementation setup itself in `Retry.withPolicy` and expect the policy to remain
active for later method calls. Use `Retry.setPolicy` for persistent installation.

### Effect HttpClient Failures

Use the canonical Effect HTTP client so transport failures stay in the Effect typed error channel.
They remain separate from the host policy:

```typescript
import { FetchHttpClient, HttpClient } from "effect/unstable/http";

const fetchStatus = (url: string) =>
  HttpClient.get(url).pipe(
    Effect.map((response) => response.status),
    Effect.catch(() => Effect.succeed(0)),
    Effect.provide(FetchHttpClient.layer),
  );

const status = yield* Retry.withPolicy(httpTransient, fetchStatus(url));
```

Here Golem decides whether to retry matching host HTTP operations. `Effect.catch` only converts a
remaining typed HTTP failure to the method's status-0 fallback. It is not the retry mechanism. Do
not replace `HttpClient` with direct `globalThis.fetch` or `Effect.tryPromise`.

## Query and Manage Active Host Policies

Host interactions are Effects and may fail with `Retry.RetryHostError`; policy conversion may also
fail with `Retry.RetryPolicyValidationError`.

```typescript
const policies = yield* Retry.getPolicies();

const policy = yield* Retry.getPolicyByName("http-transient");

const resolved = yield* Retry.resolvePolicy("GET", "https://api.example.com/items", [
  [Retry.Props.statusCode, 503],
]);

yield* Retry.removePolicy("http-transient");
```

`Retry.getPolicies()` returns raw named policies with `name`, `priority`, `predicate`, and `policy`
fields. Do not hardcode query results. There is no API to mutate the current agent retry counter;
`maxRetries` configures a policy rather than changing runtime metadata.

## Keep Application-Level Retries Separate

Expected method failures need a matching Effect Schema in the method's `error` field and should
use `Effect.fail(...)`. Handle selected tagged failures with `Effect.catchTag(...)`.

`Effect.retry(schedule)` retries an ordinary Effect in-process. It does not install a host policy,
does not create host `RetryAttempt` oplog entries, and should not replace Golem's durable retry
policy for host operations.

When application semantics really require re-running an Effect, use an Effect `Schedule`:

```typescript
import { Duration, Effect, Schedule } from "effect";

const appSchedule = Schedule.exponential(Duration.millis(100), 2).pipe(
  Schedule.intersect(Schedule.recurs(3)),
);

const value = yield* applicationOperation.pipe(
  Effect.retry(appSchedule),
  Effect.catchTag("HttpFailure", (error) => Effect.fail(error)),
);
```

Alternatively, `Retry.toSchedule(policy, { properties })` converts a Golem policy AST into an
Effect Schedule. The conversion itself is an Effect, but the resulting retries are still
application-side:

```typescript
const schedule = yield* Retry.toSchedule<HttpFailure>(
  Retry.Policy.exponential(Duration.millis(100), 2).maxRetries(3),
  {
    properties: (error) => ({
      [Retry.Props.statusCode]: error.status,
    }),
  },
);

const value = yield* applicationOperation.pipe(Effect.retry(schedule));
```

## Live-Edit Environment Policies with the CLI

```shell
golem retry-policy create http-transient \
  --priority 10 \
  --predicate '{ "propIn": { "property": "status-code", "values": [502, 503, 504] } }' \
  --policy '{ "countBox": { "maxRetries": 5, "inner": { "exponential": { "baseDelay": "200ms", "factor": 2.0 } } } }'

golem retry-policy list
golem retry-policy get http-transient
golem retry-policy update http-transient --priority 15
golem retry-policy delete http-transient
```

CLI changes affect running agents immediately in the selected environment. Agent method names and
invocation values in Effect projects use TypeScript casing and syntax, for example
`listRetryPolicies` and `RetryAgent("test")`.

## Default and Constraints

Without a matching user policy, Golem uses the built-in default: up to 3 retries, exponential
factor 3, delays clamped to 100ms–1s, and 15% jitter.

- Retry policy definitions are environment-specific; runtime installations are active for the
  current agent instance.
- Names are unique within the active policy set; setting the same name overwrites it.
- Higher priority wins and only the first matching policy applies.
- Status-code retries require an explicit `status-code` predicate.
- Prefer host-managed retries for durable operations and Effect retries only for intentional
  application-level re-execution.
