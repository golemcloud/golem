"""EXIT and ERR traps within one run, and in the child commands the shell starts.

Each run is one script, so its EXIT trap runs when the script ends or exits, as with `bash -c`.
Child commands (`find -exec`, `xargs`, `sh -c`) stand for new processes: they start without the
caller's EXIT and ERR traps and run their own. In the oracle image `sh` is BusyBox ash.
"""

CASES = [
    ("traps: EXIT runs when the script ends", "trap 'echo bye' EXIT; echo hi; echo there"),
    ("traps: EXIT runs on exit and sees its status", "trap 'echo bye $?' EXIT; echo hi; exit 3"),
    ("traps: EXIT sees the final variable values", "x=5; trap 'echo x=$x' EXIT; x=6"),
    ("traps: exit inside the EXIT trap sets the status", "trap 'exit 7' EXIT; true"),
    ("traps: EXIT in a subshell", "( trap 'echo sub' EXIT; echo in ); echo out"),
    ("traps: EXIT replaced and cleared", "trap 'echo one' EXIT; trap 'echo two' EXIT; trap - EXIT; echo end"),
    (
        "traps: find -exec children skip the caller's EXIT trap",
        "trap 'echo bye' EXIT; mkdir -p /tmp/tx && touch /tmp/tx/a /tmp/tx/b; "
        "find /tmp/tx -type f -exec true \\; ; echo done",
    ),
    (
        "traps: xargs children skip the caller's EXIT trap",
        "trap 'echo bye' EXIT; printf 'a\\nb\\n' | xargs -n 1 true; echo done",
    ),
    ("traps: sh -c runs its own EXIT trap", "sh -c 'trap \"echo inner\" EXIT; echo body'; echo outer"),
    (
        "traps: ERR is not inherited by -exec children",
        "trap 'echo err' ERR; find /tmp -maxdepth 0 -exec false \\; ; echo done",
    ),
    ("traps: ERR runs for a failing command", "trap 'echo err $?' ERR; false; echo after"),
    ("traps: ERR is not inherited by functions without errtrace", "trap 'echo err' ERR; f() { false; echo in-f; }; f; set -E; f"),
    ("traps: DEBUG runs before each command", "trap 'echo \"debug: $BASH_COMMAND\"' DEBUG; x=1; echo $x; trap - DEBUG; echo quiet"),
    ("traps: RETURN runs when a function returns", "f() { trap 'echo returning' RETURN; echo body; }; f; echo after"),
    ("traps: trap -p and -l", "trap 'echo e' EXIT; trap -p EXIT; trap - EXIT; trap -p EXIT; echo status=$?"),
    ("traps: trap on an invalid signal", "trap 'echo x' NOSUCH; echo status=$?; trap -- 'echo y' 99; echo status=$?"),
    ("traps: EXIT trap in a function runs at script end", "f() { trap 'echo exiting' EXIT; }; f; echo body"),
    ("traps: exit status inside EXIT trap", "trap 'echo \"exit with $?\"' EXIT; (exit 5); exit"),
]
