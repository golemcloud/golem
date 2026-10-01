# Built-in Bash acceptance

`app::builtin_bash::bound_bash_executes_scripts_tools_and_recovers` starts an isolated local
server, builds the small Rust fixture from the in-tree SDK, deploys it with the protected
`bash@0.2.0` release, creates its owner agents, and calls `golem tool invoke --agent`.
Build the embedded Bash artifact and matching CLI/server binaries first:

```sh
cargo make build-builtin-tools
cargo build -p golem -p golem-cli
cargo test -p golem-cli --test integration -- app::builtin_bash --report-time
```

The test verifies shell stdout, shell stderr, exit codes, incremental local pipelines,
completion-based sibling streams, peer progress during a delayed call, attachment limits,
named errors, shared owner files, filesystem denial, discovery isolation, fresh shells that carry
only the passed cwd (and settings brought back from a file), invalid-cwd rejection, local HTTP
requests, streaming (an endless response piped into
`head -c 5` must end, which only a client that forwards each chunk allows), completed-call crash
recovery, idempotency, and subsequent clean invocations. Each invocation prints its elapsed time. Its stderr
assertions concern the shell's captured stderr; the commands it calls declare no stderr of their own.

`app::builtin_bash::bound_bash_routes_a_siblings_stderr` calls the fixture's `interleave`, which
declares a stderr channel and writes to it and stdout in turn. It checks that the sibling's stdout
and stderr reach the command's fd 1 and fd 2 as they do for a local command (`2>/dev/null`, `2>&1`,
`|&`, a pipe that changes only stdout, `2>file`), with stdout first, and that a declared error
follows the sibling's own stderr with its declared status.

`app::builtin_bash::bound_bash_recovers_a_crash_while_a_sibling_is_pending` crashes the owner
while the fixture's `checkpoint` sibling is parked on a GET to an in-test HTTP endpoint, once with
`cat` and once with `head -c 0` as the reader. It checks the recovered results, that the re-sent
request carries the original idempotency key, that each owner-file effect happens exactly once,
and that a later crash replays completed invocations without another request. It is quarantined
(`#[ignore]`): since #3992, replaying the owner after that crash sometimes fails with `Unexpected oplog
entry during replay`, so the in-flight request is never re-sent.

`app::builtin_bash::bound_bash_background_jobs_signal_and_cancel_siblings` covers the process
model in Golem. It checks that each call gets a new `$$`, and that `kill` and the end of a run
cancel a background job's pending sibling call, whose `after` effect is never written, and that
several sibling calls in flight at once each keep their own status and effects.
`bound_bash_recovers_a_crash_while_a_background_job_waits`
checks that a crash while a background job waits on a sibling recovers the same result with each
effect once; it is quarantined (`#[ignore]`) because the owner is sometimes not reconstructed after
`simulate-crash`.

`bound_bash_http_crash_recovery_resends_an_incomplete_request` crashes the owner while `curl` waits
on a request the test's server holds: a GET is re-sent with its original `idempotency-key`, and a
POST runs again from the start with a new one. It is quarantined too: since #3967 the owner's
recovery never re-sends the request, GET or POST.

Run the quarantined scenarios with `-- app::builtin_bash --include-ignored`. The filter above runs
the other three.

The standalone conformance matrix, which compares the shell with Bash 5 and GNU tools without a
Golem server, lives in [builtin-tools/bash/conformance](../../../../builtin-tools/bash/conformance/README.md).
