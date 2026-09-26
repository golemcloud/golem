"""Shell and pipeline cases: cooperative pipes, SIGPIPE, traps, jobs and sibling tool probes.

These were the harness's original built-in cases. `probe-tool` and `legacy-stdin` are fixtures
of the cooperative example; the oracle has neither, so their cases carry whole-result fixtures.
"""

CASES = [
    ("input process substitution", "printf side-effect; cat <(echo input)"),
    ("output process substitution", "printf side-effect; echo output > >(cat)"),
    ("endless copy", "while :; do echo x; done | cat | head -n 1"),
    ("multiple incremental filters", "while :; do echo x; done | tr x y | grep y | sed s/y/z/ | cut -c1- | head -n 1"),
    ("early grep", "while :; do echo x; done | grep -q x; echo status=$?"),
    ("early sed", "while :; do echo x; done | sed 1q | cat"),
    ("early jq", "while :; do echo '{\"x\":\"ø\"}'; done | jq .x | head -n 1"),
    ("early xargs", "while :; do echo x; done | xargs -n 1 echo | head -n 1"),
    ("tail from start", "while :; do echo x; done | tail -n +2 | head -n 1"),
    ("subshell isolation", "v=outer; echo inner | { read v; echo \"$v\"; }; echo \"$v\""),
    ("lastpipe", "shopt -s lastpipe; printf 'héllø\\n' | read -N 2 v; echo \"$v\""),
    ("success status array", "true | false | true; echo \"$? ${PIPESTATUS[*]}\""),
    ("pipefail", "set -o pipefail; true | false | true; echo \"$? ${PIPESTATUS[*]}\""),
    ("finite pipefail", 'set -o pipefail; printf fresh | cat | cat; echo "status=$? ${PIPESTATUS[*]}"'),
    ("early pipefail", "set -o pipefail; while :; do echo x; done | cat | head -n 1; echo \"$? ${PIPESTATUS[*]}\""),
    ("functions and loops", "f() { while :; do echo x; done | cat; }; f | cat | head -n 1; echo after"),
    ("nested substitution", "echo \"$(echo \"$(echo x | cat)\" | cat)\""),
    ("large substitution", "v=$(printf '%100000s\\n' x | cat); echo ${#v}"),
    ("large record", "printf '%100000s\\n' x | cat | head -c 10 | wc -c"),
    ("large here document", "cat <<'EOF' | cat | head -c 10\n" + "x" * 100_000 + "\nEOF\n"),
    ("here string", "cat <<<hello | head -c 2"),
    ("sequential reads", "printf 'héllø\\nsecond\\n' | { read a; read b; echo \"$a:$b\"; }"),
    ("mapfile", "printf 'héllø\\nsecond' | { mapfile -t a; printf '%s\\n' \"${a[@]}\"; }"),
    ("redirected producer", "echo hi > /tmp/redirected | head -n 0; cat /tmp/redirected"),
    ("local stderr", "{ echo out; echo err >&2; } 2>/tmp/error | cat; cat /tmp/error"),
    ("merged local stderr", "{ echo out; echo err >&2; } 2>&1 | cat"),
    ("pipe ampersand precedence", "{ echo out; echo err >&2; } 2>/tmp/error |& cat; cat /tmp/error"),
    ("command directories", "mkdir /tmp/a /tmp/b; (cd /tmp/a; echo data > local; cat local) | (cd /tmp/b; read v; echo \"$v\"; pwd); pwd"),
    ("unconsumed stdin", "printf 'b\\na\\n' >/tmp/input; while :; do echo x; done | sort /tmp/input | cat"),
    ("negative head count", "printf 'a\\nb\\nc\\n' | head -n -1"),
    ("unterminated tail", "printf 'a\\nb\\nc' | tail -n 2"),
    ("sed last address", "printf 'a\\nb\\nc' | sed -n '$p'"),
    ("tee handles broken pipe", "printf '%100000s\\n' x | tee -p /tmp/tee | head -c 1; wc -c </tmp/tee"),
    ("yielding timeout", "sleep 0.1 | { read -t 0.01 v; echo status=$?; }"),
    ("tail zero exits early", "while :; do echo x; done | tail -n 0; echo done"),
    ("foreground file follow", "echo initial >/tmp/follow; { sleep 0.03; echo appended >>/tmp/follow; sleep 0.05; echo end >>/tmp/follow; } | tail -f -s 0.01 /tmp/follow | head -n 2; echo done"),
    ("numeric printf status", "printf '%d %d\\n' bad 2; echo status=$?; echo after"),
    ("formatted printf repeated", "printf '[%s]:%04d\\n' a 1 b 2 c 3 | head -n 2"),
    ("cat byte formatting", "printf 'a\\n\\n\\n\\t\\r\\n' | cat -nsvET"),
    ("cut null records", "printf 'a:b\\0c:d\\0' | cut -z -d : -f 2"),
    ("stateful translation", "printf 'aaabbbbccccc\\n' | tr -s abc xyz | cat"),
    ("stateful unique groups", "printf 'a\\na\\nb\\nc\\nc\\n' | uniq -c | cat"),
    ("grep whole records", "printf 'a\\nAA\\nab\\n' | grep -ix a | cat"),
    ("jq successive values", "printf '1 2 true null \\\"ø\\\" {\\\"x\\\":1}\\n' | jq ."),
    ('pending tool and progressing consumer', 'probe-tool delay | { read -t 0.01 v; echo before=$?; echo ready >/tmp/probe-peer; read v; echo "$v"; }'),
    ('yielding tool input', 'printf "%100000s\\n" x | probe-tool input | head -c 10 | wc -c'),
    ('tool context in nested substitution', 'echo "$(probe-tool input <<<hello | cat)"'),
    ('completion after reader exit then fresh input', 'set -o pipefail; probe-tool delay | head -n 0; printf "status=%s pipes=%s\\n" "$?" "${PIPESTATUS[*]}"; cat /tmp/probe-completed; printf fresh | probe-tool input; echo done'),
    ('ignored PIPE tool completion after reader exit', 'trap \'\' PIPE; set -o pipefail; probe-tool delay | head -n 0; printf "status=%s pipes=%s\\n" "$?" "${PIPESTATUS[*]}"; cat /tmp/probe-completed; printf fresh | probe-tool input; echo done'),
    ('execution-only tool after yield', 'f() { sleep 0.01; probe-tool destroy; }; f; echo status=$?'),
    ('tool named exit code', 'probe-tool fail 2>/tmp/tool-error; echo status=$?; cat /tmp/tool-error'),
    ('delayed completion stays command scoped', '{ echo begin; probe-tool delay; echo survived >/tmp/tool-parent; } | { read v; sleep 0.01; echo "$v"; }; cat /tmp/probe-completed; cat /tmp/tool-parent; printf fresh | probe-tool input; echo done'),
    ('tool failure after output preserves status', 'set -o pipefail; probe-tool fail-output | cat; echo "status=$? ${PIPESTATUS[*]}"'),
    ("unsupported workflows refuse immediately", "prompt-user question >/dev/null 2>&1; a=$?; confirm action >/dev/null 2>&1; b=$?; sudo true >/dev/null 2>&1; c=$?; if test \"$a\" -ne 0 && test \"$b\" -ne 0 && test \"$c\" -ne 0; then echo refused; else echo unexpected; fi", ["tool.refused"]),
    ("following commands", "while :; do echo x; done | cat | head -n 1; printf 'fresh\\n' | cat; echo after"),
    ("reader close preserves later effects", "{ printf x; sleep 0.03; echo survived >/tmp/lifetime; echo diagnostic >&2; } | head -c 1; cat /tmp/lifetime"),
    ("failed builtin write kills only its stage", "set -o pipefail; { while :; do printf x; done; echo bad >/tmp/builtin-after; } | head -c 1; printf 'status=%s pipes=%s\\n' \"$?\" \"${PIPESTATUS[*]}\"; if test -e /tmp/builtin-after; then echo leaked; else echo absent; fi"),
    ("failed utility write leaves group alive", "printf '%100000s\\n' x >/tmp/utility-input; { cat /tmp/utility-input; echo survived >/tmp/utility-after; } | head -c 1; cat /tmp/utility-after"),
    ("failed nested pipeline leaves group alive", "{ while :; do echo x; done | cat; echo survived >/tmp/nested-after; } | head -n 1; cat /tmp/nested-after"),
    ("ignored PIPE is inherited", "trap '' PIPE; { while :; do echo x 2>/dev/null || break; done; echo continued >/tmp/ignored-after; } | head -n 1; cat /tmp/ignored-after"),
    ("caught PIPE preserves failed status", "{ trap 'echo handler:$? >/tmp/caught-status' PIPE; while :; do echo x 2>/dev/null || break; done; echo continued >/tmp/caught-after; } | head -n 1; cat /tmp/caught-status; cat /tmp/caught-after"),
    ("caught SIGPIPE handler exits 23", "set -o pipefail; { trap 'echo handler:$? >/tmp/caught-exit; exit 23' SIGPIPE; while :; do echo x 2>/dev/null; done; } | head -n 1; printf 'status=%s pipes=%s\\n' \"$?\" \"${PIPESTATUS[*]}\"; cat /tmp/caught-exit"),
    ("numeric signal 13 handler exits 23", "set -o pipefail; { trap 'echo handler:$? >/tmp/caught-13; exit 23' 13; while :; do echo x 2>/dev/null; done; } | head -n 1; printf 'status=%s pipes=%s\\n' \"$?\" \"${PIPESTATUS[*]}\"; cat /tmp/caught-13"),
    ("reset PIPE restores default", "set -o pipefail; { trap 'echo bad >/tmp/reset-handler' PIPE; trap - PIPE; while :; do echo x; done; } | head -n 1; printf 'status=%s pipes=%s\\n' \"$?\" \"${PIPESTATUS[*]}\"; if test -e /tmp/reset-handler; then echo leaked; else echo reset; fi"),
    ("inherited caught PIPE is displayed but inert", "trap 'echo bad >/tmp/inherited-handler' PIPE; set -o pipefail; { trap -p PIPE >/tmp/inherited-display; while :; do echo x; done; } | head -n 1; printf 'status=%s pipes=%s\\n' \"$?\" \"${PIPESTATUS[*]}\"; cat /tmp/inherited-display; if test -e /tmp/inherited-handler; then echo leaked; else echo inert; fi"),
    ("function shares caught PIPE process", "{ trap 'echo handler:$? >/tmp/function-handler' PIPE; f() { while :; do echo x 2>/dev/null || break; done; echo continued >/tmp/function-after; }; f; } | head -n 1; cat /tmp/function-handler; cat /tmp/function-after"),
    ("subshell resets caught PIPE", "{ trap 'echo bad >/tmp/subshell-handler' PIPE; (while :; do echo x; done); printf 'sub=%s\\n' \"$?\" >/tmp/subshell-status; echo continued >/tmp/subshell-after; } | head -n 1; cat /tmp/subshell-status; cat /tmp/subshell-after; if test -e /tmp/subshell-handler; then echo leaked; else echo inert; fi"),
    ("substitution has its own PIPE process", "trap 'echo bad >/tmp/substitution-handler' PIPE; value=$(while :; do echo x; done | head -n 1); printf '%s\\n' \"$value\"; if test -e /tmp/substitution-handler; then echo leaked; else echo inert; fi"),
    ("lastpipe trap mutation persists", "shopt -s lastpipe; printf x | trap 'echo lastpipe' PIPE; trap -p PIPE"),
    ("oversized trap display drains asynchronously", "body=$(printf '%100000s' x); trap \"$body\" PIPE; trap -p PIPE | wc -c"),
    ("trap signal listing handles backpressure", "set -o pipefail; { for i in {1..10000}; do trap -l; echo; done; echo bad >/tmp/trap-list-after; } | cat | head -n 1; printf 'status=%s pipes=%s\\n' \"$?\" \"${PIPESTATUS[*]}\"; if test -e /tmp/trap-list-after; then echo leaked; else echo absent; fi"),
    # The recursing pipeline and its own status/PIPESTATUS report both run inside one subshell
    # with stderr silenced: bash segfaults on this construct (a real bash bug, which is the
    # point -- our tool doesn't), and its own job-control report of that crash names a PID and,
    # depending on the host's core-dump settings, "(core dumped)", so it can never be a stable
    # golden. `$?`/`PIPESTATUS` are read inside the subshell (not after it) since a subshell
    # doesn't propagate variable changes back out; only stdout/status need to survive it.
    ("PIPE handler recursion is suppressed", "set -o pipefail; ( { trap 'echo recurse 2>/dev/null; echo once >/tmp/pipe-once; exit 23' PIPE; while :; do echo x 2>/dev/null; done; } | head -n 1; printf 'status=%s pipes=%s\\n' \"$?\" \"${PIPESTATUS[*]}\" ) 2>/dev/null; cat /tmp/pipe-once"),
    ("closed diagnostic stream keeps ignored status", "trap '' PIPE; { while :; do echo x 2>&-; rc=$?; if test \"$rc\" -ne 0; then echo \"$rc\" >/tmp/closed-status; break; fi; done; } | head -n 1; cat /tmp/closed-status"),
    ("closed diagnostic stream still delivers caught PIPE", "set -o pipefail; { trap 'echo handler:$? >/tmp/closed-handler; exit 23' PIPE; while :; do echo x 2>&1; done; } 2>&1 | head -c 1; printf 'status=%s pipes=%s\\n' \"$?\" \"${PIPESTATUS[*]}\"; cat /tmp/closed-handler"),
    ("PIPE handler precedes ERR and errexit", "set -o pipefail; { set -eE; trap 'touch /tmp/pipe-seen' PIPE; trap 'if test -e /tmp/pipe-seen; then touch /tmp/ordered; else touch /tmp/reversed; fi; if test -e /tmp/err-once; then touch /tmp/err-duplicate; else touch /tmp/err-once; fi' ERR; sleep 0.03; echo x 2>/dev/null; echo bad >/tmp/errexit-after; } | true; printf 'status=%s pipes=%s\\n' \"$?\" \"${PIPESTATUS[*]}\"; if test -e /tmp/pipe-seen; then echo pipe; else echo missing-pipe; fi; if test -e /tmp/ordered && test ! -e /tmp/reversed; then echo ordered; else echo reversed; fi; if test -e /tmp/err-once && test ! -e /tmp/err-duplicate; then echo once; else echo duplicate; fi; if test -e /tmp/errexit-after; then echo leaked; else echo absent; fi"),
    ("legacy stdin rejects finite pipe", "printf input | legacy-stdin"),
    ("legacy stdin rejects pipe at EOF", ": | legacy-stdin"),
    ("legacy stdin rejects oversized pipe", "printf '%100000s' x | legacy-stdin"),
    ("legacy stdin rejects endless pipe", "while :; do echo x; done | legacy-stdin"),
    ("xargs reports child SIGPIPE provenance", "set -o pipefail; while :; do echo x; done | xargs -n 1 echo | head -n 1; printf 'status=%s pipes=%s\\n' \"$?\" \"${PIPESTATUS[*]}\""),
    ("xargs treats explicit 141 as ordinary failure", "set -o pipefail; printf 'x\\n' | xargs -n 1 bash -c 'exit 141'; printf 'status=%s pipes=%s\\n' \"$?\" \"${PIPESTATUS[*]}\""),
    ("ignored PIPE cat status", "printf '%1000000s\\n' x >/tmp/ignored-input; trap '' PIPE; set -o pipefail; cat /tmp/ignored-input | head -c 1; printf '\\nstatus=%s pipes=%s\\n' \"$?\" \"${PIPESTATUS[*]}\""),
    ("ignored PIPE head status", "printf '%1000000s\\n' x >/tmp/ignored-input; trap '' PIPE; set -o pipefail; head -c 1000001 /tmp/ignored-input | head -c 1; printf '\\nstatus=%s pipes=%s\\n' \"$?\" \"${PIPESTATUS[*]}\""),
    ("ignored PIPE tail status", "printf '%1000000s\\n' x >/tmp/ignored-input; trap '' PIPE; set -o pipefail; tail -c 1000001 /tmp/ignored-input | head -c 1; printf '\\nstatus=%s pipes=%s\\n' \"$?\" \"${PIPESTATUS[*]}\""),
    ("ignored PIPE tr status", "printf '%1000000s\\n' x >/tmp/ignored-input; trap '' PIPE; set -o pipefail; tr x y </tmp/ignored-input | head -c 1; printf '\\nstatus=%s pipes=%s\\n' \"$?\" \"${PIPESTATUS[*]}\""),
    ("ignored PIPE cut status", "printf '%1000000s\\n' x >/tmp/ignored-input; trap '' PIPE; set -o pipefail; cut -c 1- </tmp/ignored-input | head -c 1; printf '\\nstatus=%s pipes=%s\\n' \"$?\" \"${PIPESTATUS[*]}\""),
    ("ignored PIPE uniq status", "printf '%1000000s\\n' x >/tmp/ignored-input; trap '' PIPE; set -o pipefail; uniq </tmp/ignored-input | head -c 1; printf '\\nstatus=%s pipes=%s\\n' \"$?\" \"${PIPESTATUS[*]}\""),
    ("ignored PIPE grep status", "printf '%1000000s\\n' x >/tmp/ignored-input; trap '' PIPE; set -o pipefail; grep x </tmp/ignored-input | head -c 1; printf '\\nstatus=%s pipes=%s\\n' \"$?\" \"${PIPESTATUS[*]}\""),
    ("ignored PIPE jq status", "{ printf '\"'; printf '%1000000s' x; printf '\"\\n'; } >/tmp/ignored-json; trap '' PIPE; set -o pipefail; jq . </tmp/ignored-json | head -c 1; printf '\\nstatus=%s pipes=%s\\n' \"$?\" \"${PIPESTATUS[*]}\""),
    ("ignored PIPE sort status", "printf '%1000000s\\n' x >/tmp/ignored-input; trap '' PIPE; set -o pipefail; sort </tmp/ignored-input | head -c 1; printf '\\nstatus=%s pipes=%s\\n' \"$?\" \"${PIPESTATUS[*]}\""),
    ("ignored PIPE sed status", "printf '%1000000s\\n' x >/tmp/ignored-input; trap '' PIPE; set -o pipefail; sed s/x/y/g </tmp/ignored-input | head -c 1; printf '\\nstatus=%s pipes=%s\\n' \"$?\" \"${PIPESTATUS[*]}\""),
    ("ignored PIPE tee default", "printf '%1000000s\\n' x >/tmp/tee-input; trap '' PIPE; set -o pipefail; tee /tmp/tee-output </tmp/tee-input | head -c 1; printf '\\nstatus=%s pipes=%s\\n' \"$?\" \"${PIPESTATUS[*]}\"; wc -c </tmp/tee-output"),
    ("ignored PIPE tee p", "printf '%1000000s\\n' x >/tmp/tee-input; trap '' PIPE; set -o pipefail; tee -p /tmp/tee-output </tmp/tee-input | head -c 1; printf '\\nstatus=%s pipes=%s\\n' \"$?\" \"${PIPESTATUS[*]}\"; wc -c </tmp/tee-output"),
    ("ignored PIPE tee warn-nopipe", "printf '%1000000s\\n' x >/tmp/tee-input; trap '' PIPE; set -o pipefail; tee --output-error=warn-nopipe /tmp/tee-output </tmp/tee-input | head -c 1; printf '\\nstatus=%s pipes=%s\\n' \"$?\" \"${PIPESTATUS[*]}\"; wc -c </tmp/tee-output"),
    ("ignored PIPE tee exit-nopipe", "printf '%1000000s\\n' x >/tmp/tee-input; trap '' PIPE; set -o pipefail; tee --output-error=exit-nopipe /tmp/tee-output </tmp/tee-input | head -c 1; printf '\\nstatus=%s pipes=%s\\n' \"$?\" \"${PIPESTATUS[*]}\"; wc -c </tmp/tee-output"),
    ("ignored PIPE tee warn", "printf '%1000000s\\n' x >/tmp/tee-input; trap '' PIPE; set -o pipefail; tee --output-error=warn /tmp/tee-output </tmp/tee-input | head -c 1; printf '\\nstatus=%s pipes=%s\\n' \"$?\" \"${PIPESTATUS[*]}\"; wc -c </tmp/tee-output"),
    ("ignored PIPE tee exit", "printf '%1000000s\\n' x >/tmp/tee-input; trap '' PIPE; set -o pipefail; tee --output-error=exit /tmp/tee-output </tmp/tee-input | head -c 1; printf '\\nstatus=%s pipes=%s\\n' \"$?\" \"${PIPESTATUS[*]}\"; if test \"$(wc -c </tmp/tee-output)\" -lt 1000001; then echo early; else echo full; fi"),
    ("tee policy does not leak to later writer", "printf '%1000000s\\n' x >/tmp/tee-input; set -o pipefail; trap '' PIPE; tee -p /tmp/tee-output </tmp/tee-input | head -c 1; printf '\\ntee=%s pipes=%s\\n' \"$?\" \"${PIPESTATUS[*]}\"; trap - PIPE; while :; do echo x; done | head -n 1; printf 'later=%s pipes=%s\\n' \"$?\" \"${PIPESTATUS[*]}\"; wc -c </tmp/tee-output"),
    ("tee warn survives closed diagnostic stream", "printf '%1000000s\\n' x >/tmp/tee-input; set -o pipefail; tee --output-error=warn /tmp/tee-output </tmp/tee-input 2>&1 | head -c 1; printf '\\nstatus=%s pipes=%s\\n' \"$?\" \"${PIPESTATUS[*]}\"; wc -c </tmp/tee-output"),
    ("tee exit survives closed diagnostic stream", "printf '%1000000s\\n' x >/tmp/tee-input; set -o pipefail; tee --output-error=exit /tmp/tee-output </tmp/tee-input 2>&1 | head -c 1; printf '\\nstatus=%s pipes=%s\\n' \"$?\" \"${PIPESTATUS[*]}\"; if test \"$(wc -c </tmp/tee-output)\" -lt 1000001; then echo early; else echo full; fi"),
    # Synthetic process model: numbered background jobs, signals, and end-of-run stopping.
    ("background output and wait", "echo start; { sleep 0.05; echo job; } & wait; echo end"),
    ("wait pid returns job status", "(exit 7) & p=$!; wait $p; echo status=$?"),
    ("last background pid is a stable number", 'sleep 0.01 & a=$!; b=$!; [ "$a" = "$b" ] && [ "$a" -gt 0 ] && echo ok; wait'),
    ("wait unknown pid", "wait 3999999 2>/dev/null; echo status=$?"),
    ("wait next", "(sleep 0.05; exit 4) & (exit 5) & wait -n; echo first=$?; wait -n; echo second=$?"),
    ("kill default term", "sleep 5 & p=$!; kill $p; wait $p; echo status=$?"),
    # Bash prints "Hangup"/"Killed" notices from wait; they are not modelled, so they are dropped.
    ("kill hangup and kill", "sleep 5 & a=$!; sleep 5 & b=$!; kill -HUP $a; kill -9 $b; wait $a 2>/dev/null; echo a=$?; wait $b 2>/dev/null; echo b=$?"),
    # Bash races an immediate INT against the job ignoring it; wait until the job is running.
    ("background ignores interrupt", "( sleep 0.1; echo survived ) & p=$!; sleep 0.02; kill -INT $p; wait $p; echo status=$?"),
    ("caught term runs inside job", "( trap 'echo cleanup; exit 3' TERM; while :; do sleep 0.01; done ) & p=$!; sleep 0.05; kill $p; wait $p; echo status=$?"),
    ("parent traps are not inherited by jobs", "trap 'echo parent' TERM; sleep 5 & p=$!; kill $p; wait $p; echo status=$?"),
    ("kill unknown pid", "kill 3999999 2>/dev/null; echo status=$?"),
    ("kill zero probes liveness", "sleep 0.1 & p=$!; kill -0 $p && echo alive; wait; kill -0 $p 2>/dev/null || echo gone"),
    ("shell pid is stable in subshells", 'a=$$; b=$(echo $$); ( [ "$a" = "$b" ] && [ "$a" = "$$" ] && echo same )'),
    ("bashpid differs inside a job", '( [ "$BASHPID" != "$$" ] && echo differs ) & wait'),
    # A pipeline stage is a subshell; Bash shows it the parent's jobs, which is not modelled.
    ("jobs lists running jobs", "sleep 0.1 & jobs -p >/tmp/jobs; wc -l </tmp/jobs | tr -d ' '; wait"),
    ("background pipeline last stage", 'true | sleep 0.01 & p=$!; wait $p; echo status=$?'),
    ("leftover job is stopped at the end", "sleep 5 & echo started", ["jobs.end-of-run"]),
    ("leftover job ignoring hangup is killed", "trap '' HUP; sleep 5 & echo started", ["jobs.end-of-run"]),
]
PROBE = "probe-tool is a fixture of the cooperative example; the oracle has no such command"
LEGACY = "legacy-stdin is a fixture of the cooperative example; the oracle has no such command"

EXPECTED = {
    'pending tool and progressing consumer': (0, b'before=142\ncompleted:peer=true\n', b'', PROBE),
    'yielding tool input': (0, b'10\n', b'', PROBE),
    'tool context in nested substitution': (0, b'hello\n', b'', PROBE),
    'completion after reader exit then fresh input': (
        0, b'status=141 pipes=141 0\ncompleted\nfreshdone\n', b'', PROBE,
    ),
    'ignored PIPE tool completion after reader exit': (
        0, b'status=1 pipes=1 0\ncompleted\nfreshdone\n', b'probe-tool: write error: Broken pipe\n', PROBE,
    ),
    'execution-only tool after yield': (0, b'destroyed\nstatus=0\n', b'', PROBE),
    'tool named exit code': (0, b'status=7\nnamed fixture error\n', b'', PROBE),
    'delayed completion stays command scoped': (
        0, b'begin\ncompleted\nsurvived\nfreshdone\n', b'', PROBE,
    ),
    'tool failure after output preserves status': (0, b'partial\nstatus=7 7 0\n', b'', PROBE),
    'PIPE handler recursion is suppressed': (
        0, b'x\nstatus=23 pipes=23 0\nonce\n', b'',
        "the synthetic SIGPIPE is not re-raised while its handler runs",
    ),
    'legacy stdin rejects finite pipe': (
        1, b'', b'synchronous stdin commands cannot consume cooperative pipes; use an async command with OpenFile::async_io()\n', LEGACY,
    ),
    'legacy stdin rejects pipe at EOF': (
        1, b'', b'synchronous stdin commands cannot consume cooperative pipes; use an async command with OpenFile::async_io()\n', LEGACY,
    ),
    'legacy stdin rejects oversized pipe': (
        1, b'', b'synchronous stdin commands cannot consume cooperative pipes; use an async command with OpenFile::async_io()\n', LEGACY,
    ),
    'legacy stdin rejects endless pipe': (
        1, b'', b'synchronous stdin commands cannot consume cooperative pipes; use an async command with OpenFile::async_io()\n', LEGACY,
    ),
    'leftover job is stopped at the end': (
        0, b'started\n', b'bash: stopped job [1] (pid 2, hangup): sleep 5\n',
        "bash-tool stops jobs still running at the end of a call; bash leaves them running",
    ),
    'leftover job ignoring hangup is killed': (
        0, b'started\n', b'bash: stopped job [1] (pid 2, killed): sleep 5\n',
        "bash-tool stops jobs still running at the end of a call; bash leaves them running",
    ),
}

# Status and stdout continue to come from Bash. These per-case replacements document only the
# diagnostics where the Rust command intentionally uses its own exact wording.
EXPECTED_STDERR_REASON = "the command reports a broken-pipe write error in its own words"
EXPECTED_STDERR = {
    'ignored PIPE head status': b'head: write error: Broken pipe\n',
    'ignored PIPE tail status': b'tail: write error: Broken pipe\n',
    'ignored PIPE jq status': b'jq: write error: Broken pipe\n',
    'ignored PIPE sort status': b'sort: write error: Broken pipe\n',
    'ignored PIPE sed status': b'sed: write error: Broken pipe\n',
}
