"""Jobs, traps and signals in the cooperative process model.

Cases print facts derived from process numbers (equality, status), never the numbers themselves,
since the two shells number processes differently. Scripts that leave a job running redirect its
output, so the oracle does not wait for it to close the captured streams.
"""

CASES = [
    # the job cap is session-wide, counting nested and disowned jobs. The jobs
    # sleep far longer than the case runs, so none finishes and frees a slot however slowly the
    # case is scheduled, and the counts are exact; `kill 0` ends them with the script.
    (
        "jobs: job cap counts disowned jobs",
        "for i in $(seq 1 300); do sleep 30 >/dev/null 2>&1 & disown; done 2>/tmp/err; "
        "echo \"refused=$(grep -c retry /tmp/err)\"; kill 0",
    ),
    (
        "jobs: job cap counts jobs of every subshell",
        "for s in 1 2; do ( for i in $(seq 1 200); do sleep 30 >/dev/null 2>&1 & done 2>>/tmp/err; "
        ": >/tmp/done$s; wait ) & done; until [ -e /tmp/done1 ] && [ -e /tmp/done2 ]; do sleep 0.05; done; "
        "echo \"refused=$(grep -c retry /tmp/err)\"; kill 0",
    ),
    # Bash never refuses, so the script also stops waiting once every leaf has started.
    (
        "jobs: job cap bounds a fork tree",
        "f() { if [ \"$1\" -gt 0 ]; then f $(($1-1)) & f $(($1-1)) & wait; "
        "else echo >>/tmp/leaves; sleep 30 >/dev/null 2>&1; fi; }; : >/tmp/leaves; f 9 2>/tmp/err & "
        "for i in $(seq 200); do [ -s /tmp/err ] && break; "
        "[ \"$(wc -l </tmp/leaves)\" = 512 ] && break; sleep 0.05; done; "
        "[ -s /tmp/err ] && echo capped || echo uncapped; kill 0",
    ),
    # jobs that have not had their first turn do not count against the cap.
    (
        "jobs: jobs that finish at once never hit the cap",
        "for i in $(seq 300); do : >/tmp/f$i & done 2>/tmp/err; wait; ls /tmp | grep -c '^f'; cat /tmp/err",
    ),
    # a trapped signal interrupts wait.
    (
        "jobs: trapped signal interrupts wait",
        "trap 'kill $c; echo stopped; exit 1' TERM; (while :; do sleep 0.05; done) & c=$!; "
        "(sleep 0.2; kill $$) & wait $c",
    ),
    (
        "jobs: wait returns 128 plus the trapped signal",
        # The trailing bare `wait` is cleanup, not what this case tests (`wait %1`'s exit status
        # is): by the time it runs, job 2 may already be reaped, and bash's "is not a child of
        # this shell" diagnostic for that case names its PID -- silenced the same way as this
        # file's other incidental-cleanup `wait`s.
        "trap 'echo caught' TERM; sleep 0.8 & (sleep 0.2; kill $$) & wait %1; echo st=$?; "
        "wait 2>/dev/null",
    ),
    # jobs outlive the subshell, stage, substitution or child that
    # started them, as orphaned processes do.
    ("jobs: a job started in a subshell outlives it", "( { sleep 0.2; echo late; } & ); sleep 0.6; echo end"),
    (
        "jobs: a job started in a subshell keeps its effect",
        # A wider margin than the inner sleep (was 0.2/0.5, a 2.5x cushion): reproducibly stale
        # under a loaded CI runner running many oracle containers at once, where scheduling delay
        # alone can eat that gap even though the ordering this case tests never changes.
        "( (sleep 0.2; echo written > /tmp/f) & ); sleep 1.5; cat /tmp/f",
    ),
    (
        "jobs: command substitution waits for a job holding its output",
        "x=$( { sleep 0.2; echo hi; } & ); echo \"[$x]\"",
    ),
    (
        "jobs: command substitution does not wait for a job writing elsewhere",
        "x=$( sleep 0.3 >/dev/null & ); echo \"[$x]\"; sleep 0.5",
    ),
    ("jobs: a job started in a pipeline stage outlives the stage", "{ { sleep 0.2; echo late; } & } | cat; echo end"),
    (
        "jobs: a job started by bash -c outlives it",
        "bash -c '{ sleep 0.2; echo late >/tmp/f; } &'; sleep 0.5; cat /tmp/f",
    ),
    ("jobs: wait for a disowned pid", "sleep 0.1 & p=$!; disown; wait $p; echo st=$?"),
    (
        "jobs: a disowned job is stopped and reported at the end of the call",
        "(sleep 0.3; echo late >/tmp/f) & disown; echo started\n#--call--\nsleep 0.8; cat /tmp/f",
    ),
    (
        "jobs: a job orphaned by its subshell is stopped and reported at the end of the call",
        "( sleep 5 >/dev/null 2>&1 & ); echo end",
    ),
    # EXIT traps run when a signal ends a shell, and in every subshell.
    ("jobs: EXIT trap runs when a signal ends the script", "trap 'echo cleanup' EXIT; kill $$; echo after"),
    ("jobs: EXIT trap's exit keeps the signal status", "trap 'echo cleanup; exit 5' EXIT; kill $$; echo after"),
    (
        "jobs: EXIT trap runs when a watchdog kills the script",
        "trap 'echo cleanup' EXIT; ( sleep 0.1; kill $$ ) & while :; do sleep 0.05; done",
    ),
    ("jobs: EXIT trap runs after HUP", "trap 'echo exit-trap' EXIT; kill -HUP $$; echo after"),
    ("jobs: EXIT trap runs after INT", "trap 'echo exit-trap' EXIT; kill -INT $$; echo after"),
    ("jobs: KILL skips the EXIT trap", "trap 'echo exit-trap' EXIT; kill -KILL $$; echo after"),
    ("jobs: EXIT trap in a command substitution", "x=$(trap 'echo inner-exit' EXIT; echo body); echo \"[$x]\""),
    ("jobs: EXIT trap in a pipeline stage", "{ trap 'echo stage-exit' EXIT; echo stage-body; } | cat"),
    (
        "jobs: EXIT trap in a background subshell",
        ": >/tmp/lock; ( trap 'rm -f /tmp/lock' EXIT; sleep 0.1 ) & wait; test -e /tmp/lock && echo leaked || echo removed",
    ),
    ("jobs: EXIT trap in a background group", "{ trap 'echo job-exit' EXIT; echo job; } & wait; echo done"),
    (
        "jobs: EXIT trap in a job killed while it sleeps",
        "( trap 'echo job-cleanup' EXIT; sleep 5 >/dev/null ) & p=$!; sleep 0.1; kill $p; wait $p; echo st=$?",
    ),
    (
        "jobs: EXIT trap in a job killed in a loop",
        "( trap 'echo job-cleanup' EXIT; while :; do sleep 0.02; done ) & p=$!; sleep 0.1; kill $p; wait $p; echo st=$?",
    ),
    ("jobs: EXIT trap writes to the subshell's redirection", "( trap 'echo sub' EXIT; echo in ) >/tmp/f; echo out; cat /tmp/f"),
    ("jobs: EXIT trap of a stage ended by SIGPIPE", "{ trap 'echo bye >&2' EXIT; while :; do echo x; done; } | head -1"),
    ("jobs: EXIT trap of a substitution ended by a signal", "x=$(trap 'echo bye' EXIT; kill $BASHPID); echo \"[$x] $?\""),
    ("jobs: EXIT trap of a child shell ended by a signal", "bash -c 'trap \"echo child-exit\" EXIT; kill $$' 2>/dev/null; echo st=$?"),
    (
        "jobs: EXIT trap of sh -c writes to its redirection",
        "sh -c 'trap \"echo bye\" EXIT; echo in' >/tmp/f; echo out; cat /tmp/f",
    ),
    (
        "jobs: children do not run the caller's EXIT trap",
        "trap 'echo P' EXIT; echo x | cat; ( echo sub ); y=$(echo s); { echo job; } & wait",
    ),
    # the whole signal set, and trap installs every valid signal it is given.
    ("jobs: trap installs every signal of a list", "trap 'echo cleanup' EXIT INT QUIT TERM; echo st=$?"),
    (
        "jobs: trap keeps the valid handlers around an invalid name",
        "trap 'echo cleanup' EXIT BOGUS TERM; echo st=$?; trap -p TERM",
    ),
    ("jobs: trap by signal numbers", "trap 'echo cleanup' 0 1 2 3 15; echo st=$?; trap -p QUIT"),
    (
        "jobs: trap clears and ignores signal lists",
        "trap - INT QUIT TERM; echo st=$?; trap '' HUP INT QUIT TERM; echo st=$?; trap -p QUIT",
    ),
    ("jobs: USR1 terminates a job by default", "sleep 5 & p=$!; kill -USR1 $p; wait $p 2>/dev/null; echo st=$?"),
    (
        "jobs: a job catches USR1",
        "(trap 'echo got-usr1' USR1; while :; do sleep 0.02; done) & p=$!; sleep 0.1; kill -USR1 $p; "
        "sleep 0.1; kill $p; wait $p; echo st=$?",
    ),
    ("jobs: background jobs ignore QUIT", "sleep 0.5 & p=$!; sleep 0.1; kill -QUIT $p; wait $p; echo st=$?"),
    (
        "jobs: signals whose default action is to ignore",
        "sleep 5 & p=$!; kill -CHLD $p; kill -WINCH $p; kill -URG $p; kill -CONT $p; "
        "kill -0 $p && echo alive; kill $p; wait $p; echo st=$?",
    ),
    (
        "jobs: STOP and CONT",
        "( sleep 0.2; echo t ) & p=$!; kill -STOP $p; sleep 0.4; echo before; kill -CONT $p; wait $p; echo st=$?",
    ),
    (
        "jobs: TERM to a stopped job waits for CONT",
        "sleep 0.2 & p=$!; kill -STOP $p; kill $p; echo st=$?; kill -CONT $p; wait $p; echo st=$?",
    ),
    ("jobs: KILL ends a stopped job", "sleep 0.2 & p=$!; kill -STOP $p; kill -KILL $p; wait $p 2>/dev/null; echo st=$?"),
    ("jobs: a job continues a stopped script", "( sleep 0.1; kill -CONT $$ ) & kill -STOP $$; echo resumed $?"),
    (
        "jobs: trap accepts KILL and STOP",
        "trap 'echo x' KILL; echo st=$?; trap 'echo y' STOP; echo st=$?; trap -p KILL; trap -p STOP",
    ),
    (
        "jobs: real-time signal names",
        "trap 'echo x' RTMIN; trap -p RTMIN; trap 'echo y' SIGRTMAX-2; trap -p 62; trap 'echo z' 34; trap -p 34",
    ),
    # kill, trap and jobs listings and diagnostics.
    ("jobs: kill -l table", "kill -l; echo \"st=$?\""),
    ("jobs: trap -l table", "trap -l; echo \"st=$?\""),
    (
        "jobs: kill -l numbers and statuses",
        "kill -l 143 15 TERM SIGTERM 9 64 65 0 127 128 256 271; echo \"st=$?\"",
    ),
    (
        "jobs: kill -l names",
        "kill -l USR1 sigusr2 RTMIN RTMIN+1 RTMAX-1 35 50 63 131 137 138 130 129 192 hup SIGhup bogus; "
        "echo \"st=$?\"; kill -l 34; echo \"[$?]\"; kill -L 9",
    ),
    (
        "jobs: kill signal specification errors",
        "kill -s 99 $$; echo st=$?; kill -n 99 $$; echo st=$?; kill -99 $$; echo st=$?; "
        "kill -n 9x $$; echo st=$?; kill -9x $$; echo st=$?",
    ),
    ("jobs: kill usage", "kill; echo st=$?; kill -s TERM; echo st=$?; kill -s; echo st=$?"),
    (
        "jobs: kill of something that is not a pid",
        "kill abc; echo $?; kill -- -abc; echo $?; kill 1.5; echo $?; kill ''; echo $?",
    ),
    ("jobs: kill by job name", "kill %sleep; echo $?; sleep 0.3 & kill %sleep; wait; echo $?"),
    ("jobs: kill by job substring", "sleep 0.3 & kill %?lee; echo $?; wait %1 2>/dev/null; echo $?"),
    ("jobs: ambiguous job name", "sleep 0.3 & sleep 0.3 & kill %sleep; echo $?; wait"),
    ("jobs: kill 0 ends the script", "kill 0; echo after $?"),
    ("jobs: kill 0 with a TERM trap", "trap 'echo caught' TERM; kill 0; echo after $?"),
    ("jobs: kill 0 from a subshell", "( kill 0; echo inner ); echo after $?"),
    # kill 0 ends every job with the script, disowned or orphaned too: none is left to report.
    (
        "jobs: kill 0 ends background jobs with the script",
        "sleep 30 >/dev/null 2>&1 & sleep 30 >/dev/null 2>&1 & disown; echo started; kill 0",
    ),
    (
        "jobs: kill 0 reaches a job whose subshell has ended",
        "( sleep 30 >/dev/null 2>&1 & ); echo started; kill 0",
    ),
    ("jobs: kill -l of a death status", "sleep 5 & p=$!; kill $p; wait $p; kill -l $?"),
    ("jobs: INT reaches a job that has not started", "sleep 0.5 & p=$!; kill -INT $p; wait $p; echo st=$?"),
    ("jobs: INT is ignored once a job runs", "sleep 0.5 & p=$!; sleep 0.1; kill -INT $p; wait $p; echo st=$?"),
    (
        "jobs: jobs -n",
        "sleep 0.3 & jobs -n; echo st=$?; jobs -n; echo st=$?; wait; true & sleep 0.1; jobs -n; echo st=$?; jobs; echo st=$?",
    ),
    ("jobs: jobs with job specs", "sleep 0.2 & sleep 0.1; jobs %1; jobs %2; echo st=$?; wait"),
    ("jobs: suspend without job control", "suspend; echo st=$?"),
    ("jobs: jobs shows a job killed by a signal", "sleep 5 & kill %1; sleep 0.1; jobs; echo st=$?"),
    # exec onto an output process substitution keeps it draining.
    (
        "jobs: exec stdout into tee",
        "exec > >(tee /tmp/log); echo hello; echo world; sleep 0.2; cat /tmp/log >&2",
    ),
    (
        "jobs: exec stdout and stderr into tee",
        "exec > >(tee /tmp/log) 2>&1; echo hello; echo world >&2\n#--call--\ncat /tmp/log",
    ),
    ("jobs: exec a descriptor into a process substitution", "exec 3> >(cat); echo into3 >&3; exec 3>&-; sleep 0.1; echo end"),
    ("jobs: exec stdout into a filter", "exec > >(tr a-z A-Z); echo hello"),
    # exit, break and return in a pipeline stage end only the stage.
    (
        "jobs: exit in the last pipeline stage",
        "printf 'a\\nb\\n' | while read -r l; do [ \"$l\" = b ] && exit 1; done; echo \"continued $?\"",
    ),
    ("jobs: exit in a single-command stage", "echo x | exit 3; echo \"after $?\""),
    ("jobs: break in a pipeline stage", "for i in 1 2; do echo x | break; echo \"in $i\"; done; echo \"st=$?\""),
    ("jobs: return in a pipeline stage", "f() { echo x | return 5; echo \"after $?\"; }; f; echo \"f=$?\""),
    # children and subshells are processes of their own.
    ("jobs: bash -c gets its own $$", "a=$$; b=$(bash -c 'echo $$'); [ \"$a\" != \"$b\" ] && echo differs"),
    (
        "jobs: concurrent sh -c children do not share $$",
        "{ for i in 1 2 3; do sh -c 'echo \"job $1\" > /tmp/t.$$; sleep 0.2; cat /tmp/t.$$' sh \"$i\" & done; wait; } | sort; "
        "ls /tmp | grep -c '^t\\.'",
    ),
    ("jobs: kill $$ in bash -c ends only the child", "bash -c 'kill $$; echo not'; echo \"after child: $?\""),
    ("jobs: kill $BASHPID ends only the subshell", "(kill $BASHPID; echo x); echo main=$?"),
    (
        "jobs: BASHPID differs in every subshell",
        "a=$BASHPID; b=$(echo $BASHPID); ( c=$BASHPID; [ \"$a\" != \"$c\" ] && echo sub ); "
        "[ \"$a\" != \"$b\" ] && echo subst; echo x | { read; d=$BASHPID; [ \"$a\" != \"$d\" ] && echo stage; }; "
        "[ \"$a\" = \"$$\" ] && echo main",
    ),
    ("jobs: EXIT trap in a subshell that kills itself", "( trap 'echo sub-exit' EXIT; kill $BASHPID; echo not ); echo st=$?"),
    # $! is the shell's last background pid, not a job-table lookup.
    ("jobs: $! survives wait", "sleep 0.01 & p=$!; wait; [ \"$!\" = \"$p\" ] && echo kept"),
    (
        "jobs: $! survives disown and wait -n",
        "sleep 0.01 & p=$!; disown; [ \"$!\" = \"$p\" ] && echo disown; sleep 0.01 & q=$!; wait -n; "
        "[ \"$!\" = \"$q\" ] && echo waitn; sleep 0.1",
    ),
    ("jobs: $! inside a subshell", "sleep 0.1 & p=$!; ( [ \"$!\" = \"$p\" ] && echo inherited ); wait"),
    # a background job reads a piped or redirected compound's input.
    ("jobs: background job reads a piped group's input", "echo hi | { cat & wait; }"),
    ("jobs: background job reads a piped subshell's input", "echo hi | ( cat & wait )"),
    ("jobs: background job reads a redirected group's input", "{ cat & wait; } <<< hi"),
    ("jobs: background job in a nested subshell reads nothing", "echo hi | { ( cat & wait ); }"),
    ("jobs: background job in a piped function", "f() { cat & wait; }; echo hi | f"),
    ("jobs: redirected function call does not keep input", "f() { cat & wait; }; f <<< hi"),
    (
        "jobs: background jobs share piped input",
        "seq 20000 | { { wc -l >/tmp/r1; } & { wc -l >/tmp/r2; } & wait; }; echo $(( $(cat /tmp/r1) + $(cat /tmp/r2) ))",
    ),
    ("jobs: loop input reaches a background job", "while read l; do echo \"got $l\"; cat & wait; done <<< $'a\\nb\\nc'"),
    # stages, substitutions and jobs list their parent's jobs but cannot wait for or
    # signal them.
    ("jobs: wait for a parent's job in a stage", "sleep 0.3 & p=$!; wait $p 2>/dev/null | cat; echo ${PIPESTATUS[*]}; wait"),
    ("jobs: kill %1 from a stage", "sleep 0.5 & kill %1 2>/dev/null | cat; wait %1; echo $?"),
    ("jobs: kill %1 from a stage reports no such job", "sleep 0.3 & kill %1 | cat; wait"),
    ("jobs: wait for a sibling from a job", "sleep 0.3 & s=$!; ( wait $s 2>/dev/null; echo \"in job $?\" ) & wait"),
    ("jobs: wait in a stage has no jobs to wait for", "sleep 0.3 & wait | cat; echo \"${PIPESTATUS[*]}\"; wait"),
    ("jobs: a stage lists the parent's jobs", "sleep 0.3 & jobs | cat; wait"),
    # waited jobs leave the table; their statuses stay known.
    ("jobs: a second wait on a pid", "(exit 3) & p=$!; wait $p; wait $p; echo $?"),
    ("jobs: waited jobs leave the table", "for i in 1 2 3; do true & wait $!; done; jobs; echo end"),
    ("jobs: job numbers restart after reaping", "true & wait $!; sleep 3 & kill %1; wait %1 2>/dev/null; echo st=$?"),
    ("jobs: many waited jobs stay fast", "for i in $(seq 3000); do true & wait $!; done; jobs; echo done"),
    # jobs formatting.
    (
        "jobs: jobs prints subshells and groups on one line",
        "( sleep 0.3; echo x >/dev/null ) & { sleep 0.3; true; } & sleep 0.3 | cat & jobs; wait",
    ),
    # Process numbers differ between the shells, so these print each line's width up to the state
    # or the stage instead: the columns bash lays out.
    (
        "jobs: jobs -l for a pipeline job",
        "sleep 0.3 | cat | cat & jobs -l | while IFS= read -r l; do p=${l%%[R|]*}; echo \"${#p}:${l#\"$p\"}\"; done; wait",
    ),
    ("jobs: jobs -p for a pipeline job", "sleep 0.3 | cat & jobs -p | wc -l; wait"),
    (
        "jobs: jobs -l for a simple job",
        "sleep 0.3 & jobs -l | while IFS= read -r l; do p=${l%%[R|]*}; echo \"${#p}:${l#\"$p\"}\"; done; wait",
    ),
    ("jobs: jobs states after a job ends", "(exit 3) & sleep 0.1; jobs; sleep 5 & kill -9 %1; sleep 0.1; jobs; sleep 5 & kill -USR1 %1; sleep 0.1; jobs"),
    (
        "jobs: jobs text of redirected and nested commands",
        "( sleep 0.3; echo x ) >/dev/null & { sleep 0.3; } 2>/dev/null & { true & sleep 0.3; } & echo x | ( cat ) >/dev/null & "
        "x=1 sleep 0.3 >/dev/null 2>&1 & jobs; wait",
    ),
    ("jobs: declare -f prints pipelines and redirections as bash does", "f() { a | b 2>&1; ( c; d ) > /dev/null; { e; } >&2 2>&1; cat <&3; }; declare -f f"),
    ("jobs: stopped-job note is one line", "( sleep 5; echo x ) >/dev/null & echo started"),
    # The tool's end-of-run grace: a HUP handler that computes before exiting still counts as
    # hangup; the grace is one timer, whatever the handler does.
    (
        "jobs: a HUP handler that computes before exiting",
        "(trap 'for i in $(seq 300); do :; done; echo handled >/tmp/h; exit 0' HUP; while :; do :; done) >/dev/null & echo main"
        "\n#--call--\ncat /tmp/h",
    ),
    # wait -n and wait on reaped jobs.
    (
        "jobs: wait -n honours its ids",
        "(sleep 0.4; exit 3) & a=$!; (sleep 0.1; exit 4) & b=$!; (exit 5) & c=$!; wait -n $a $b; echo $?; wait",
    ),
    ("jobs: wait on a reaped job spec", "sleep 0.1 & wait -n; wait %1; echo $?"),
    ("jobs: wait -n with nothing to wait for", "wait -n; echo $?"),
    # wait -n sleeps until a job ends, wherever it runs.
    ("jobs: wait -n in a command substitution", "x=$(sleep 0.1 & wait -n; echo $?); echo \"[$x]\""),
    ("jobs: wait -n in a pipeline stage", "{ (sleep 0.1; exit 3) & wait -n; echo \"stage $?\"; } | cat"),
    ("jobs: wait -n in a background job", "( (sleep 0.1; exit 5) & wait -n; echo \"job $?\" ) & wait"),
    ("jobs: wait -n for a parent's job in a stage", "sleep 0.2 & wait -n | cat; echo \"${PIPESTATUS[*]}\"; wait"),
    (
        "jobs: wait -n returns for a trapped signal",
        # The trailing cleanup `wait` (any remaining jobs, not job %1 specifically) reports an
        # error naming job %1's own PID once `wait -n %1` already returned early for the trapped
        # signal -- that PID isn't part of what this case tests (the trap firing and `wait -n`'s
        # own status), and it changes every run, so it's silenced rather than recorded.
        "trap 'echo caught' TERM; sleep 0.8 & (sleep 0.2; kill $$) & wait -n %1; echo st=$?; wait 2>/dev/null",
    ),
    ("jobs: wait -n reports jobs in the order they end", "(sleep 0.3; exit 1) & (sleep 0.1; exit 2) & wait -n; echo $?; wait -n; echo $?"),
    # jobs -x runs a command with job specs replaced by the job's process group: `$$` without
    # job control.
    (
        "jobs: jobs -x replaces job specs",
        "sleep 0.2 & jobs -x echo %1 %% %+ a%1 '%1 x' | { read a b c d e f; [ \"$a $b $c\" = \"$$ $$ $$\" ] && echo \"groups $d $e $f\"; }; wait",
    ),
    ("jobs: jobs -x passes other words through", "sleep 0.2 & jobs -x echo %2 %sleepy; echo st=$?; wait; jobs -x echo %1; echo st=$?"),
    (
        "jobs: jobs -x runs functions, statuses and missing commands",
        "sleep 0.2 & f() { echo \"f:$#\"; }; jobs -x f %1 x; echo st=$?; jobs -x false; echo st=$?; "
        "jobs -x; echo st=$?; jobs -x nosuchcmd; echo st=$?; x=1; jobs -x x=2; echo st=$? x=$x; wait",
    ),
    (
        "jobs: jobs -x options",
        "sleep 0.2 & jobs -l -x echo hi; echo st=$?; jobs -xl echo hi; echo st=$?; jobs -r -x echo hi; "
        "r=$(jobs -x -- echo -n %1); [ \"$r\" = \"$$\" ] && echo dashdash; wait",
    ),
    (
        "jobs: a redirected background subshell catches signals itself",
        "( trap 'echo caught; exit 3' TERM; while :; do sleep 0.01; done ) >/tmp/o & p=$!; sleep 0.1; kill $p; wait $p; echo st=$?; cat /tmp/o",
    ),
    (
        "jobs: a redirected background subshell keeps its redirected input",
        "echo hi >/tmp/in; ( cat & wait ) </tmp/in & wait",
    ),
    ("jobs: jobs -x in a stage and a subshell", "sleep 0.2 & jobs -x echo %1 | { read x; [ \"$x\" = \"$$\" ] && echo stage; }; ( jobs -x echo %1 ); wait"),
    ("jobs: wait for a job spec twice", "(exit 3) & wait %1; echo $?; wait %1; echo $?"),
    ("jobs: wait without operands keeps no statuses", "(exit 3) & p=$!; wait; wait $p 2>/dev/null; echo $?"),
    ("jobs: jobs keeps the status of a job it reported", "(exit 3) & p=$!; sleep 0.1; jobs >/dev/null; wait $p; echo $?"),
    ("jobs: wait -n reaps the job it reports", "(sleep 0.1; exit 7) & p=$!; wait -n $p; echo $?; wait $p; echo $?; wait -n; echo $?"),
    ("jobs: wait -n with a non-child", "wait -n 99999; echo $?; sleep 0.1 & wait -n 99999 $!; echo $?"),
    ("jobs: wait on something that is not a job", "wait abc; echo $?; wait %abc; echo $?; wait %; echo $?"),
    ("jobs: wait on an ambiguous job name", "sleep 0.2 & sleep 0.2 & wait %sleep; echo $?; wait"),
    # a utility's output and diagnostics keep their order under 2>&1. (tac and od
    # still write theirs in an order of their own; see the coreutils fork.)
    ("jobs: ls diagnostics before the listing under 2>&1", "echo x >/tmp/f; ls /tmp/nosuch /tmp/f 2>&1"),
    ("jobs: md5sum interleaves under 2>&1", "echo a >/tmp/a; echo b >/tmp/b; md5sum /tmp/a /tmp/missing /tmp/b 2>&1"),
    ("jobs: cksum interleaves under 2>&1", "echo a >/tmp/a; cksum /tmp/a /tmp/missing /tmp/a 2>&1"),
    ("jobs: nl interleaves under 2>&1", "echo a >/tmp/a; nl /tmp/a /tmp/missing /tmp/a 2>&1"),
    ("jobs: rm interleaves under 2>&1", "echo a >/tmp/a; rm -v /tmp/missing /tmp/a 2>&1"),
    (
        "jobs: md5sum -c interleaves under 2>&1",
        "echo a >/tmp/a; md5sum /tmp/a >/tmp/sums; echo '0123456789abcdef0123456789abcdef  /tmp/missing' >>/tmp/sums; "
        "md5sum /tmp/a >>/tmp/sums; md5sum -c /tmp/sums 2>&1; echo st=$?",
    ),
    ("jobs: separate streams keep their contents", "echo a >/tmp/a; md5sum /tmp/a /tmp/missing /tmp/a 2>/tmp/err; cat /tmp/err"),
]

CAP = (
    "bash-tool runs at most 256 background jobs at once in a call, counting jobs started in "
    "subshells and disowned ones; bash has no such cap"
)

STOPPED = (
    "a job cannot outlive its call: bash-tool stops every job still running when the call ends, "
    "wherever it was started, and reports it; bash leaves it running"
)

EXPECTED = {
    "jobs: a disowned job is stopped and reported at the end of the call": (
        1, b"started\n",
        b"bash: stopped job [1] (pid 2, hangup): ( sleep 0.3; echo late > /tmp/f )\n"
        b"cat: /tmp/f: No such file or directory\n",
        STOPPED,
    ),
    # Started by a subshell (pid 2), the job has no number in the script's job table.
    "jobs: a job orphaned by its subshell is stopped and reported at the end of the call": (
        0, b"end\n", b"bash: stopped job (pid 3, hangup): sleep 5 > /dev/null 2>&1\n", STOPPED,
    ),
    "jobs: a HUP handler that computes before exiting": (
        0, b"main\nhandled\n",
        b"bash: stopped job [1] (pid 2, hangup): ( trap 'for i in $(seq 300); do :; done; echo handled >/tmp/h; exit 0' HUP; "
        b"while :; do :; done ) > /dev/null\n",
        STOPPED,
    ),
    "jobs: stopped-job note is one line": (
        0, b"started\n", b"bash: stopped job [1] (pid 2, hangup): ( sleep 5; echo x ) > /dev/null\n",
        STOPPED,
    ),
    "jobs: job cap counts disowned jobs": (143, b"refused=44\n", b"", CAP),
    # The two outer subshells are jobs too: 2 + 254 inner jobs run, 146 are refused.
    "jobs: job cap counts jobs of every subshell": (143, b"refused=146\n", b"", CAP),
    "jobs: job cap bounds a fork tree": (143, b"capped\n", b"", CAP),
}
