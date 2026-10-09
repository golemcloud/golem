# Stalled replay reproducer (GOL-689)

This test-only branch uses the production `ReplayState` and the existing in-memory oplog.
No server, external service, guest component, generated WASM, or production fix is required.

## Run

Controls:

```shell
cargo test --profile dev-ci --locked -p golem-worker-executor --lib -- stalled_replay --report-time
```

Explicit regression expectation (ignored in the default suite):

```shell
cargo test --profile dev-ci --locked -p golem-worker-executor --lib -- stalled_replay_omitted_delivered_call_reports_divergence --include-ignored --report-time
```

The regression has a one-second deadline and fails with the cursor, replay target, and blocking
entry indexes instead of hanging the test process. Its expected correct outcome is
`UnexpectedOplogEntry`, identifying the unclaimed call.

Use `--include-ignored` with this exact test filter. In the pinned test-r version,
`--ignored` alone selects the test but still reports it as ignored without executing it.

Surrounding replay-state unit tests:

```shell
cargo test --profile dev-ci --locked -p golem-worker-executor --lib -- durable_host::replay_state::tests --report-time
```

## Minimal history

```text
1  NoOp (consumed prefix)
2  Start: P3 monotonic-clock.wait-for, ReadLocal, duration=50ms
3  End: Start(2), unit result
4  CompletionDelivered: Start(2)
5  Start: direct monotonic-clock.now, ReadLocal
6  End: Start(5), timestamp=42
7  AgentInvocationFinished
```

Replay omits the timer call and asks for the clock read. Its Start exists and both calls are
closed, but the claim scan stops at the earlier delivery marker. With no timer owner to consume
that marker, there is no task capable of releasing the parked reader. This isolates the general
stalled-replay problem; it is not a reproduction of libc caching or snapshot restoration.

The concurrent-owner control uses precisely the same history. A later-polled timer owner claims
Start(2), resolves its recorded terminal, acknowledges delivery at index 4, and unblocks the
original clock reader. A fix must distinguish this valid wait from a truly quiescent replay.

## Difference from the issue's original hypothesis

GOL-689 described an environment call omitted because snapshot loading primed libc's environment
cache. On the branch base, main `79f778610dca05c465deac0ee63eaf1919dd695d`, direct environment calls
have no completion-delivery marker. The cursor now retains their unclaimed Start/End, allowing
the later clock call to resolve. The direct-environment control characterizes that sequence;
do not attach an invented P3 marker to an environment call to manufacture a hang.

These are cursor-level tests, not evidence that the complete SDK/snapshot chain in GOL-689 or
GOL-740 occurs in an application. A full recovery test would separately need an actual Store
reconstruction and the affected SDK fixture.

## Verified behavior

Compiled and verified on 2026-10-09 against the branch base above:

- Both controls pass: the omitted direct environment call does not stall, and a late concurrent
  timer owner unblocks the original clock reader.
- The explicitly enabled regression fails consistently in three runs, at its one-second
  deadline, with this diagnostic:

  ```text
  replay stalled: the only reader cannot claim Start(5) beyond the unclaimed
  wait-for Start(2) and CompletionDelivered(4); cursor=1, target=7
  ```

- The surrounding replay-state suite passes by default: 190 passed, no failures, and this
  regression ignored. Its ignore attribute keeps the test-only branch green; explicitly enabling
  it demonstrates the unresolved stall rather than asserting that the bug is fixed.

Verification used the commands above with `--offline` as well. The `dev-ci` profile avoids
incremental artifact retention and reuses the locally built test binary on subsequent runs.
