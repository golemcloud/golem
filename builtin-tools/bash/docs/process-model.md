# Process model v1 for Golem's built-in bash tool

Status: approved, 2026-09-22.

## Overview

Golem's built-in bash tool (`builtin-tools/bash` in the Golem repository) runs one finite
script per `run` call inside a fresh Brush shell, on a single cooperative WASM thread. It has no
process model: `&`, `wait` and `coproc` are refused before execution, `kill` does not exist on WASM,
`$!` is empty, `BASHPID` is unset, and `$$` aborts the component because Brush expands it with
`std::process::id()`, which panics on `wasm32-wasip2` ("no pids on this platform").

This design adds a synthetic process model scoped to a single `run`: numbered background jobs with
`&`, `wait`, `jobs`, `kill`, `$!`, `$$` and `BASHPID`, catchable TERM/INT/HUP, and deterministic
cleanup of jobs still running when the script ends. It keeps the tool's published `run` signature
and result format unchanged.

Out of scope for v1: `ps`, `/proc`, process substitution, `coproc`, `fg`, `bg`, `disown`,
`wait -p`/`-f`, and any job that outlives the `run` call that started it. (Since then, the
conformance sweep added buffered process substitution and `disown`; see the README. On 2026-09-24
the tool stopped carrying any state between calls, so `$$` is now new in every call rather than
fixed for a session; the sections below say so where it matters.)

## Feasibility findings this design relies on

All references are to the Brush fork at `360af58f` (branch `cooperative-pipe-correctness`, rebased
onto upstream `737dd57e`) and Golem at `builtin-bash-tool`.

- `brush-core/src/execution.rs` provides `ExecutionServices::spawn`, owned `LocalTaskHandle`s with
  `abort`/`join`, and a `TaskScope` tree: aborting a task aborts its descendants and awaits their
  cleanup. This is the process tree.
- `brush-core/src/execution/process.rs` provides `run_process`, a logical process with its own
  SIGPIPE disposition, a `terminated` flag that ends it with `128 + 13`, and a nested task scope. It
  is installed as the current process only while its future is polled. Only SIGPIPE uses it today.
- `interp.rs::spawn_async_ao_list_in_task` already implements `&` on WASM: it clones the `Shell`
  (isolated cwd, environment, functions, open files), redirects stdin to `/dev/null`, spawns the
  list as a task and registers a `Job` with `JobTask::Internal`. It does not wrap the job in
  `run_process`. `jobs.rs` has `Job::abort` (exit 137) and upstream's `kill -0` support.
- `brush-builtins` registers `kill` only under `cfg(unix)`. `wait` supports `%job` and "wait for
  all", but `wait PID` and `wait -n` are `unimp`.
- The component's `yield_now` lets CPU-bound shell code yield: a 50 ms timer stage completes while a
  compute loop in a sibling stage is still running.
- Golem runs a tool body as one export in a transient Store and destroys the Store when the export
  returns (`golem-worker-executor/src/worker/instance.rs`, `invoke_scoped`). No shell task can
  outlive `run`. Admitted sibling tool calls, including fire-and-forget ones, settle before the
  parent operation completes.
- A sibling tool call is cancelled only by `future-invoke-result.cancel()`
  (`golem:tool/host`); dropping the handle detaches observation and does not cancel accepted work.
  `cancel` is effective for live and incomplete-replay calls and a no-op for completed replays.
- Finite uutils run through `run_uu`, which swaps process-global cwd and stdio synchronously and
  never across an await. Concurrent jobs are therefore safe, but such a utility cannot be preempted
  or killed mid-call and buffers its whole input first.

## Design

### Ownership

```mermaid
flowchart TB
  subgraph Brush["Brush fork: generic engine"]
    PT[Session process table and PID allocator]
    PS[Logical process state: pending signals, disposition, termination]
    BI["WASM builtins: kill, wait PID, wait -n, jobs -l/-p"]
    SP["Special parameters: $$, $!, BASHPID"]
  end
  subgraph Tool["bash-shell and golem-bash-tool: Golem policy"]
    PF[Preflight allows &]
    ST[Each call seeds a new $$]
    END[End-of-run stop and report]
    TX[Cancellable sibling tool calls]
  end
  ST --> PT
  END --> PS
  TX --> PS
```

The Brush fork gains only mechanism that any embedder could use. Everything Golem-specific (the
per-call `$$`, the end-of-run policy, sibling tool cancellation) stays in the bash tool's crates.

### Brush: process table and PIDs

A session-wide `ProcessTable` is shared by the shell and every subshell or job clone of it
(reference-counted, not copied, so clones cannot hand out duplicate numbers). Each entry records:

| Field | Meaning |
|---|---|
| `pid` | Synthetic process number |
| `ppid` | Number of the process that started it |
| `command` | Command text, as `jobs` displays it |
| `state` | `Running`, `Exited(code)` or `Signaled(signal)` |

The table owns a monotonic allocator seeded by the embedder with the shell's own number and the
next number to hand out. Numbers wrap at 4,194,303 (Linux's `pid_max` ceiling) and skip the shell's
own number.

PIDs are assigned where they are observable:

- the main script, which runs as a logical process numbered `$$`, so `kill $$` reaches it;
- each background job's process, and each stage of a background pipeline (`$!` is the last stage);
- each subshell, pipeline stage and command substitution, with its own `BASHPID`, so
  `(kill $BASHPID)` ends only that subshell, as in bash;
- each child shell (`sh -c`, `bash -c`, and those `find -exec` and `xargs` start), which also has
  its own `$$`, so `bash -c 'kill $$'` ends only the child and concurrent children do not share
  `/tmp/x.$$` names;
- an output process substitution that `exec` makes a shell descriptor (`exec > >(tee log)`).

Commands that run in-process to completion (the coreutils, `sed`, `jq`) do not consume numbers:
nothing can name them while they run.

Special parameters:

- `$$` returns the seeded shell number instead of calling `std::process::id()`.
- `$!` returns the number of the most recent background job's last stage, or is empty. It is the
  shell's own value, kept after `wait`, `disown` and `wait -n` and inherited by subshells.
- `BASHPID` is defined on WASM as described above.

### Brush: signals

`ProcessState` generalizes from SIGPIPE-only to a pending-signal set with a disposition per signal:
default, ignored or caught. All 64 Linux signals exist, named and numbered as bash on Linux with
musl names them (`kill -l`); 32 to 34 have numbers but no names. Their default actions are
Linux's: most terminate the process; CHLD, URG and WINCH are ignored; STOP, TSTP, TTIN and TTOU
stop it and CONT continues it. A stopped process is not polled, and a signal other than KILL
waits until it continues. `trap` installs every valid signal it is given and reports the invalid
ones, with status 1, as bash does.

- Delivery marks the signal pending on the target process and wakes it. Delivery to a job targets
  every process in the job.
- At the existing trap safe point (`commands.rs`, where `take_pending_pipe_trap` is consulted), a
  pending caught signal runs its handler in that process's shell. Execution then continues unless
  the handler exits.
- A pending signal with the default action terminates the process with `128 + signal` at its next
  poll, the same path SIGPIPE uses today.
- KILL cannot be caught or ignored and terminates at the next poll. STOP cannot be caught either.
- A shell a signal other than KILL ends still runs its EXIT trap, whose `exit` cannot change the
  status `128 + signal`. Every subshell, stage, substitution and job runs only an EXIT trap it sets
  itself, when it ends.
- A trapped signal interrupts `wait`, which returns `128 + signal` at once; the trap then runs.
- A foreground command a signal ended is reported as bash reports it (`Terminated` and the command
  for TERM; the line, number and description for other signals but INT and PIPE); so is a job
  `wait` finds a signal other than INT, TERM and PIPE ended.
- A process inside a synchronous `run_uu` call observes signals only when that call returns.

Job inheritance follows bash for asynchronous commands without job control:

- Caught handlers reset to default in the job, as `reset_pipe_for_subshell` already does for PIPE.
  A cleanup trap must therefore be set inside the job, as in bash: `( trap 'rm -f "$t"' TERM; … ) &`.
- INT and QUIT are ignored in background jobs once they run; one sent before a job's first turn
  still ends it, as bash's does before its child ignores them.
- A background job reads `/dev/null`, unless the subshell or compound command it runs in reads a
  pipe or redirects standard input (bash's `stdin_redir`): `echo hi | { cat & wait; }` prints `hi`.

A caller's stop request (the cancel of `run`'s `stdin` stream) sends INT to the main script and
every process it started, as Ctrl+C at a terminal sends it to the foreground process group. A
command in front, such as a `sleep`, ends at once. A script that catches INT then runs its
handler, and one that ignores it runs on until the KILL that follows a second later. Background
jobs ignore INT as before, so the stop leaves them to the end of the run.

Where bash's behavior is subtle, the Bash 5 oracle in the cooperative matrix is the reference.

### Brush: builtins on WASM

| Builtin | v1 behavior |
|---|---|
| `kill` | `kill [-s SIG \| -SIG \| -n N] pid\|%job ...`, `kill -l [status]`, `kill -0 pid`, parsed and worded as bash's. Default TERM. `kill 0` signals every process of the call. Targets are this session's processes only. |
| `wait` | No operands: wait for all jobs, forget them and return 0. `wait PID`/`wait %N`: return that job's status; the job leaves the table at the next cleanup (a new job, or `jobs`), and `wait PID` returns the status again later. `wait -n [ids]`: wait for the next job (among `ids`) to finish and return its status, or 127 if none remain; it sleeps until a job ends rather than polling. |
| `jobs` | Bash's formatting, one line per job (subshells and groups inline, loops as bash prints them), `Done`, `Exit N` or how a signal ended it. `-l` adds numbers, one line per pipeline stage; `-p` prints the first number; `-n` only jobs not yet listed in their state; `%N`, `%name` and `%?text` select jobs. `-x cmd args` runs `cmd` with each job spec among `args` replaced by the job's process group, which without job control is `$$`. |
| `suspend` | Without job control: `cannot suspend: no job control`, status 1. |

`disown` was added later (see the note above and the README); it is not one of the v1 exclusions
any more.

A subshell, stage or substitution lists its parent's jobs, as bash does, but cannot wait for or
signal them.

`bg`, `fg`, `wait -p`, `wait -f` and `suspend -f` remain unsupported.

Background jobs are wrapped in `run_process` so each has process state. They run in the call's job
scope, apart from the process that starts them, so a job outlives the subshell, stage,
substitution or child shell that started it, as an orphaned process does. At most 256 jobs may be
running at once in the whole call, however deeply nested and whether disowned or not. At the cap,
jobs just started get up to 64 turns to finish; if none does, starting another fails that command
with bash's `fork: retry: Resource temporarily unavailable` and status 1.

### Bash tool: preflight

`stateless.rs` stops refusing `&`. It continues to refuse `coproc` with the existing message and
status 2; process substitution was later added, buffered (see the note above and the README), so
it is no longer refused. `bg`, `fg` and `fc` stay refused. The `wait` refusal is removed, because
`wait` now has meaning.

### Bash tool: `$$` per call

Every call is a new shell process with its own numbers; the tool carries nothing between calls:

- `$$` is drawn uniformly from 1,000–4,194,303 using the guest's random source.
- The first number the table hands out is `$$ + 1`.

A random rather than fixed `$$` means two calls on the same agent, concurrent or not, do not
collide on names like `/tmp/out.$$`. Golem records guest randomness, so replay draws the same
number. (Until 2026-09-24 the state carried `$$` and the next number, so `$$` stayed fixed for a
session.)

### Bash tool: end of run

When the script finishes (normally, through `exit`, or through a signal to `$$`, after its EXIT
trap), the shell's descriptors close, an output process substitution `exec` made gets up to 64
turns to finish, and then:

1. Every job still running is sent HUP: jobs of the script (listed or disowned), and jobs that
   subshells, stages, substitutions and child shells started.
2. The tool waits until those jobs exit, or until 1 second passes on the durable monotonic clock.
   The second is one timer raced against the jobs ending.
3. Remaining jobs are sent KILL, and their sibling tool calls are cancelled (below).
4. One line per stopped job is appended to the result's stderr:
   `bash: stopped job [1] (pid 1043, hangup): sleep 1000` or `(pid 1043, killed)`. A job the
   script did not start itself has no job number: `bash: stopped job (pid 1044, hangup): …`.

The script's own exit status is returned unchanged. Two stops replace it, whatever status the
script ended with: the call's time limit returns 124, and the caller's stop request returns 130.
The tool records the place of the script at each step of such a stop. A later run of the call
repeats the step at that place or fails the call, for the scripts that Replay safety names; for
the shapes it lists as limits neither is guaranteed.
Both stops reach a loop of commands that never wait, also one that runs a pipeline in each
round, such as `n=0; while :; do n=$((n+1)); echo $n | while read -r x; do :; done; done`: each
shell and each pipeline stage gives way after 64 of its own commands, and the watchdog acts when
the call gets the completion of the limit's timer or of the stop request.
A job that finished before the script ended is not reported. This deliberately differs from bash,
whose non-interactive scripts leave background jobs running after exit, because a job cannot
survive the transient Store.

### Bash tool: cancellable sibling tool calls

`component/src/transport.rs` switches from `tool-rpc.invoke-and-await` to
`tool-rpc.async-invoke-and-await`, keeping the `future-invoke-result` handle.

- A guard owned by the bound-tool command calls `cancel()` if the owning process is terminated by a
  signal (a `kill`, or the end-of-run HUP/KILL) before `get()` returns.
- Reader closure keeps its existing contract and never cancels accepted work. A bound tool writes
  its output only after the call completes, so a broken pipe cannot terminate a process while its
  call is pending.
- Effects the tool already performed remain. Golem records the cancellation durably.

```mermaid
sequenceDiagram
  participant K as kill 1043
  participant P as job process 1043
  participant G as cancel guard
  participant T as sibling tool
  K->>P: mark TERM pending, wake
  P->>P: next poll: default action, terminate 143
  P->>G: future dropped
  G->>T: future-invoke-result.cancel()
  T-->>G: operation settles as cancelled
```

### Replay safety

Recovery re-executes the bash body from the start. Every input that determines a number, an order
or a result is either in the call's input or recorded by Golem:

- the random `shell_pid`;
- timers, including the 1-second grace;
- sibling calls and their cancellation;
- the completion order the guest observed;
- the place of the script at which each step of a stop acts (below).

A recovered run therefore allocates the same numbers and produces byte-identical output. This is
required, because Golem compares the reconstructed result with the recorded one and treats any
difference as permanent divergence. No design element reads wall-clock time or unrecorded
randomness.

One external effect is not exactly-once. An HTTP request that had not finished when the agent
crashed is sent again on recovery: an idempotent method (GET, HEAD, PUT, DELETE, OPTIONS, TRACE)
with the same `Idempotency-Key` header, a POST or PATCH with a new one, because Golem assumes by
default that a non-idempotent write may be retried. The server may therefore see an interrupted
POST or PATCH twice; a script that needs exactly-once sends its own `Idempotency-Key`, which Golem
keeps on the resent request. At this Golem version that recovery is unreliable (see the README).
`curl -m` and `wget -T` race a durable timer against the request, and the call's own time limit
(`run`'s `timeout`) is one durable wait, started before the script and pending alongside it.
When the caller passes `stdin`, the read that waits for its stop request is a second pending host
call, started after the limit's timer and before the script.

Golem records when a completion reaches the call only as a place among the call's recorded host
calls. What a script computes between two host calls is not recorded. A run that Golem makes
again from the record therefore gets the time limit's timer, or the cancel of `stdin`, as early
as the call can take it: earlier in a computing script than the first run got it. A stop that
acted there would cut the output short, and the result would differ.

So the tool records where each step of a stop acts. The steps are the first signal (TERM or
INT), the KILL a second later, and the end of the wait for a script that outlives KILL. The place
is the number of turns the tasks of the script have taken, and whether the script waited there.
The script runs as tasks: the call's own, and one for each pipeline stage and background job. A
task takes a turn each time it is woken: it runs until it waits or gives way. The interpreter
gives way every 64 commands, and the commands after a fixed number of lines or reads; nothing
gives way by the clock, so the number depends only on what the script has run. The script waits
when no task of it was woken: none takes a turn until a completion wakes one. When a step is
due, the tool makes one durable call (`bash::stop-turn`, a local read whose response is the
place): the first run keeps the place where the script is and acts there, and a later run gets
the place the first run kept. A call that is not stopped records nothing.

Golem gives a later run a completion only after every completion the first run got before it.
It does not say where the script is then. So a later run can learn of a step when its script has
taken fewer turns, or has not started, and a completion that the first run got after the step
can come as soon as the later run has asked for the record. The tool therefore asks three things
of a later run before a step acts, and fails the call when one of them does not hold:

1. **The script takes the recorded number of turns by itself.** From the question for the record
   to the step, no task of the script is woken from outside. A wake from outside is one that
   comes while no task of the script takes a turn: the component's runtime gives a completion
   between two turns. A task that another task of the script wakes is woken in that task's turn,
   and that is not a wake from outside.
2. **At the recorded number of turns the script is as the record says:** it waits if the record
   says that it waited, and it has a woken task if the record says that it had one.
3. **A script that waits earlier is met only in the same wait.** A script that waits takes no
   turn, so a later run cannot bring it to a number of turns that is ahead. If the record says
   that the script waited at the step, and the script is one task, the step acts where it
   waits, at once. The script has had every completion the first run's script had had, and none
   since the question for the record, so it is in the same wait unless what the script does
   depends on when a completion of its own came (see the limits below). It took fewer turns to
   get there because one of its own completions ended a stretch of computing, as in `timeout 5
   COMMAND` around a loop: in a later run that completion comes at once and the stretch is
   shorter. The count then goes on from the recorded number, so a KILL that follows is at its
   recorded place too. In every other case the step does not act in that wait.

A stop that comes while a script computes under `timeout` is repeated at its place: the timer of
`timeout` is a host call that is pending in both runs, and a pending host call wakes no task.

When a later run cannot repeat a step, the call ends with status 1 and one line on stderr, and
no signal is sent for that step. The result then differs from the recorded one and Golem refuses
the run. There are five cases:

- `bash: the stop was recorded at turn N, and the script is at turn M`: the script has taken
  more turns than recorded when the later run learns of the step. A later run can learn of a
  step later, counted in turns, than the first run did: the call sees a completion one turn
  after the host has it, at the least. So this happens when the first run's stop came before the
  script started, or within a few turns after one of the script's host calls returned.
- `… and the script was woken at turn M`: a completion woke a task of the script between the
  question for the record and the step (the first thing above). An example is `trap 'echo caught'
  INT; timeout 7 bash -c 'trap "" INT; LOOP; sleep 300'; sleep 300`, stopped in the inner
  `sleep`: the timer of `timeout` ends after the INT. A later run that learns of the stop while
  LOOP still runs gets the timer's end there, and its script would leave LOOP and be stopped in
  the outer `sleep`. A job whose `sleep` ends after the stop does the same.
- `… and the script waits at turn M`: the script waits, and the step cannot act in that wait.
  There are two causes. The record says that the first run's script had a woken task at the
  step, and the script waits before it has taken the recorded number of turns, or when it has
  just taken them. An example is `timeout 7 bash -c 'while :; do :; done'; for i in $(seq 3000);
  do :; done; sleep 300`, stopped in the `for` loop: in a later run `timeout` ends the first loop
  at once, and the script is in its `sleep` before it has taken the recorded number of turns. Or
  more tasks than the script's own are alive and every one of them waits before the recorded
  number of turns: where each task was in the first run cannot be told from where they wait. An
  example is `sleep 200 & timeout 7 bash -c 'while :; do :; done'; sleep 300`, stopped in the
  last `sleep`. Which tasks are alive is the script's own state: every task the script starts is
  counted from its start to its end.
- `… and the script runs at turn M`: the script has a woken task at the recorded number of
  turns, and the record says that the first run's script waited there (the second thing above).
- `… and the script ended at turn M`: the script ended before the recorded number of turns.

In the first four cases the later run fails where it passed before places were recorded, if the
script's output did not depend on where it was stopped.

So a step is repeated at the same place, or the call fails, for a script that is one task and
gets no completion of its own while it computes: the three things above decide between the two.
For the shapes below neither is guaranteed. The recorded place has these limits:

- **The order of the tasks.** The number of turns is one count for all tasks of the script, so
  it names the same place only while the tasks take their turns in the same order in every run.
  The script's own task and the watchdog are one task for the component's runtime, and a
  completion for the watchdog puts that task in the runtime's queue: in the first run where the
  stop comes, in a later run where it learns of the stop. If another task of the script then
  wakes the script's own task before the runtime polls it, the script's own task takes its turn
  ahead of the tasks that were woken in between; in the other run it took it after them. A
  script whose own task computes the whole time keeps its order, also with a job that computes
  beside it: its own task is in the queue all the time. A script whose own task waits for other
  tasks keeps its order only if none of them wakes it in the turns between a completion for the
  watchdog and the watchdog's next poll, in either run. One pipeline whose stages compute while
  the script waits for it meets that unless a stage ends in just those turns.
  A loop with a pipeline in each round does not meet it: a stage wakes the script's own task in
  every round. An example is `for n in 1 2 3 4 5 6; do f() { for i in $(seq 100); do :; done;
  echo x$n; }; f | cat; echo round $n; done; sleep 300`, stopped while a stage computes: some
  later runs print one round less, with the same status and the same number of turns. Golem
  sees that only in what the script printed. When such a script writes files and prints
  nothing, nothing sees the difference. The tool neither detects nor repairs this.
- **A completion of the script's own while it computes.** A `sleep` in the background next to a
  loop, a download in one stage of a pipeline, a loop under `timeout` whose timer ends: a later
  run gets that completion earlier, and the tool records nothing for it. If the output depends
  on when the completion came, a later run differs whether the script is stopped or not. If it
  does not, a later run still takes another number of turns from there on. A stop that came
  later while the script computed then acts after the recorded number of turns in another part
  of the script, or fails the call when the script gets to a wait first (`waits at`). A stop
  that came later while the script waited, as one task, acts in the same wait (the third thing
  above) or fails the call (`was woken at`, `waits at`). A stop that came before the completion
  fails the call if the later run gets the completion on its way (`was woken at`).
- **A record that says "woken" at the start of a wait.** A stop that comes at the end of the
  turn in which the script begins its wait can find the script's task woken once more, with
  nothing to do. That happens after a command substitution, a subshell, `bash -c`, a pipeline, a
  signal the script sent to itself, and `timeout`. The record then says that the script had a
  woken task. A later run that takes the same turns repeats the step; one that gets to that
  wait in fewer turns fails with `waits at`, where a record that said "waited" would have let it
  stop the script in the wait.
- **The place among the host calls.** The turn places a step within what the script computes
  between two of its host calls. The place of the step among the host calls themselves is in
  Golem's record, not in the tool's. One case moves it: a task of the script that starts a
  recorded host call (the clock of `date`, a `sleep`) in a turn between the moment Golem records
  the completion and the moment the watchdog is polled and the tool records the turn. In the
  record that call stands before `bash::stop-turn`; a later run, which records the turn as soon
  as it has the completion, asks for the two in the other order.
- **A question for the record that is never answered.** `bash::stop-turn` is a synchronous
  durable call: the call's own task makes it and goes on only with the answer. In a later run
  Golem answers such a call only when the call has taken every completion that it was given
  before that call in the first run. A script with more tasks (a pipeline stage, a command
  substitution, a job) can have a task that took a completion, or a read of a stream, before the
  watchdog asked in the first run. A later run in which that task is behind asks first, and the
  question then waits: the call hangs, with no error and no line in a log. The same holds where
  the call's `stdin` reaches the tool through a middleware. A script that is one task, in a call
  that a client makes directly, is not exposed. This follows from how Golem replays a
  synchronous durable call; no run of the tool has shown it.
- **The same checks fail some later runs that would have given the same result.** The first
  thing above fails every later run that gets a completion between the question and the step,
  also when the completion changes nothing, as the end of a job that the stop ends anyway.

## Error Handling

| Situation | Outcome |
|---|---|
| `kill` target is not a process of this session | `kill: (N) - No such process`, status 1 |
| Invalid signal name or number | `kill: SIG: invalid signal specification`, status 1 |
| `wait` on a number that is not this shell's job | bash's message, status 127 |
| 257th concurrent job | `fork: retry: Resource temporarily unavailable`, status 1 for that command |
| `cancel()` fails or the call already completed | Ignored. The job is still reported as stopped. |
| A later run of a call cannot repeat a stop at its recorded place | `bash: the stop was recorded at turn N, and the script is at turn M` (or `was woken at`, `waits at`, `runs at`, `ended at turn M`), status 1 |
| Unsupported job control (`fg`, `bg`, `wait -p`, `wait -f`) | Existing unsupported-in-bash-tool message, status 2 |
| `coproc` | Still refused before execution, status 2 |

## Testing

1. **Brush fork unit tests** (native, current-thread `LocalSet`):
   - allocation, wrap and skip;
   - signal delivery at safe points;
   - `128 + n` statuses;
   - a caught handler that continues;
   - an uncatchable KILL;
   - INT ignored in jobs;
   - `wait PID`, `wait -n`, `jobs -l`/`-p`;
   - `kill -0`, `kill -l` and invalid input.
2. **Bash tool crate tests:**
   - a new `$$` in every call;
   - preflight allows `&` and process substitution, and still refuses `coproc`;
   - the end-of-run sequence and its stderr lines, using test execution services.
3. **Cooperative WASM matrix** against the Bash 5 oracle: `&`, `wait`, `wait -n`, `$!`, `kill`,
   trap handlers, inheritance and statuses, comparing exact stdout, stderr and status. Cases print
   facts derived from numbers (equality, ordering, status), never raw numbers, since the two shells
   allocate differently.
4. **Golem CLI integration**, with the checkpoint fixture:
   - `$$` no longer aborts, and each call gets a new one;
   - `kill` of a job waiting on `fixture checkpoint` cancels it, and `after` is never written;
   - end-of-run stopping cancels a pending sibling call;
   - a simulated crash during a run with live jobs recovers the identical result, with every
     effect exactly once.

## Alternatives Considered

- **Everything in the Brush fork.** Rejected. It puts Golem policy (the per-call `$$`, end-of-run
  reporting, tool cancellation) in a general shell library and enlarges a fork that must be rebased
  regularly.
- **Everything in the bash tool crate, with thin Brush hooks.** Rejected. Job and process internals
  live in `brush-core`, so the tool would duplicate them or reach into them through awkward hooks.
- **Wait for leftover jobs at the end of the script.** Rejected. `sleep 1000 &` would hold the call
  for 1,000 seconds.
- **Treat leftover jobs as an error.** Rejected as surprising for bash users.
- **Report stopped jobs in a new structured result field.** Rejected for v1. It changes the
  published result schema and needs a new tool version. A stderr line carries the same information.
- **Stop only the shell side of a killed job and let its tool call finish.** Rejected. `kill` would
  not stop the work, and the operation would still wait for the call to settle.
- **A `$$` fixed for a session.** Chosen at first, and carried in the state. Dropped on
  2026-09-24 with the state itself: a call keeps nothing from the previous one, so a script that
  needs a name in a later call writes it to a file.
- **A fixed `$$`.** Rejected. Concurrent calls on one agent would collide on `$$`-named files.
- **Jobs that survive across calls.** Deferred. Golem destroys the tool's Store when `run` returns,
  and sibling work settles before the operation completes. This needs a new executor feature for
  durably starting detached work.
- **Kill-only signals without traps.** Rejected. Cleanup traps are a common idiom, and the fork's
  SIGPIPE mechanism generalizes directly.

## Open Questions

- The job cap (256) and the grace period (1 second) are initial values. Should either be
  configurable per binding, or remain constants until real workloads suggest otherwise?
- Upstreaming: the generic engine (session process table, signal generalization, WASM `kill` and
  `wait`) is a candidate for upstream Brush. It should be proposed after v1 has settled in the fork.
