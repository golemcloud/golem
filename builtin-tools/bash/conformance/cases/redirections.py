"""`exec` redirections, which change the shell's own descriptors for every later command.

Each run hands the shell its capture streams as descriptors 0-2. They must live in the shell's
descriptor table rather than in per-command parameters, or an `exec` redirection is shadowed and
later commands keep writing to the original streams. These cases cover Brush's builtins and the
embedded commands, which resolve their streams differently.
"""

CASES = [
    (
        "redirections: exec 2>&1 merges stderr into stdout",
        "exec 2>&1; echo builtin >&2; ls /nope; cat /nope; echo after",
    ),
    ("redirections: exec 1>&2 moves stdout to stderr", "exec 1>&2; echo moved; ls -d /"),
    (
        "redirections: exec 2>file captures later diagnostics",
        "exec 2>/tmp/e; ls /nope; cat /nope; exec 2>&1; cat /tmp/e",
    ),
    (
        "redirections: exec saves and restores stdout through fd 3",
        "exec 3>&1; exec 1>/tmp/o; echo to-file; exec 1>&3 3>&-; echo back; cat /tmp/o",
    ),
    (
        "redirections: exec 2>&1 reaches pipeline stages",
        "exec 2>&1; ls /nope | cat; echo status=${PIPESTATUS[*]}",
    ),
    (
        "redirections: exec 2>&1 reaches functions and subshells",
        "exec 2>&1; f() { ls /nope; }; f; ( cat /nope ); echo done",
    ),
    (
        "redirections: command-level redirect still overrides exec",
        "exec 2>&1; ls /nope 2>/dev/null; echo status=$?",
    ),
]
