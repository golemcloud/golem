# Persistent rate-limit middleware

`persistent-rate-limit` is universal tool middleware backed by one durable
`RateLimitBackend` agent per `(policy, limit, windowMilliseconds)` configuration. The backend is
outside the transient middleware instance, serializes admissions from every calling owner, and
persists its state through Golem's ordinary durable-agent execution.

The policy key is the authenticated invocation principal, not the owning agent. Consequently two
different owners acting as the same principal share a limit, while different principals have
independent counters. Anonymous invocations share the `anonymous` key. The deployment environment
already isolates the backend agent, and `policy` separates independent limits within it. The limit
and window are immutable constructor parameters, not admission arguments. Changing either selects
a separate backend with independent capacity; an invocation replaying its pinned old configuration
continues to use the old backend and its recorded decisions.

The policy uses fixed windows aligned to Unix epoch boundaries. `limit: N` admits exactly N new
logical invocations in each `windowMilliseconds` interval; call N+1 is rejected before dispatch.
Each middleware invocation durably generates one UUID and passes it to the backend. The backend
permanently records both admitted and rejected decisions by that UUID, so retrying or replaying the
same logical invocation returns its original decision and an admitted invocation is charged once.

## Binding

Declare the component and middleware, then install it universally or on selected tools:

```yaml
components:
  golem:rate-limit-middleware:
    dir: vendor/rate-limit-middleware
    templates: rust

tools:
  middleware:
    persistent-rate-limit:
      component: golem:rate-limit-middleware

environments:
  production:
    server: cloud
    tools:
      middleware:
        - name: persistent-rate-limit
          parameters:
            policy: outbound-api
            limit: 20
            windowMilliseconds: 1000
```

The included `golem.yaml` is runnable and also publishes version `0.1.0` as a middleware release.
Build it with the repository's Golem CLI:

```sh
golem build -P release --yes
```

`policy` must be non-empty; `limit` and `windowMilliseconds` must be greater than zero. Rate-limit
rejections use the middleware `resource-exhausted` channel and include the remaining wait in
milliseconds.

The backend agent is an implementation detail, but it is visible as an ordinary agent type in the
deployment. Environment invoke permissions must remain operator-controlled: directly invoking its
`admit` method can consume capacity, just as direct writes to any external policy store could.
