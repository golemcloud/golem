# Built-in Bash acceptance

`app::builtin_bash::bound_bash_executes_scripts_tools_and_recovers` starts an isolated local
server, builds the small Rust fixture from the in-tree SDK, deploys it with the protected
`bash@0.2.1` release, creates its owner agents, and calls `golem tool invoke --agent`.
Fetch the pinned built-in artifacts and build matching CLI/server binaries first:

```sh
cargo make fetch-builtin-artifacts
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

Three tests bind the built-in `read-file`, `write-file` and `edit-file` tools to one owner. Each
test runs its scripts on that owner and on an owner without the tools, and the two results must be
equal. In the oplog it checks which files went to the tool. The steps in which the test acts
between two points of a script (a crash, a held sibling call, a wait of `tee`) run on the owner
with the tools only.

- `app::builtin_bash::bound_bash_cat_reads_text_files_through_read_file`: 1,000 lines in five
  calls, and files whose pages end inside a line, inside a character and inside a CR LF pair. A
  file with a NUL byte and a missing file give GNU's output, and `cat` ends before a reader that
  exits at once. When `tee` empties the file while `cat` waits, `cat` writes the same bytes as
  on the owner without the tools. Only a real server shows this: it gives a file to a reader in
  blocks.
- `app::builtin_bash::bound_bash_tee_writes_text_files_through_write_file`: a text, bytes that
  need care, two operands and a file that exists. `tee` writes the file itself for an append, for
  bytes that are not text and for no input. When `tee` must wait for more input, the file has the
  bytes so far.
- `app::builtin_bash::bound_bash_sed_changes_text_files_in_place_through_edit_file`: one line, a
  new, a removed and a changed line, and a file with no change. `sed` replaces the file itself for
  a copy under a suffix, a symbolic link, a file with a second name and a file that the script
  has open.

Each test also checks that a replay passes the calls, and that the command does the work itself
while a sibling call is in flight. The `cat` and `tee` tests check that a job that has not started
does not run during a call. The three tests are ignored (`#[ignore]`) while the pinned `bash`
artifact is one in which these commands do not use the tools.

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
