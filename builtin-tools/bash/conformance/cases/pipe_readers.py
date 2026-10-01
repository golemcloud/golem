"""Pipelines whose reader exits early. A short writer finishes before the reader exits, as bash's
does into a pipe with room for its output, so `set -o pipefail` sees no SIGPIPE; a writer that
fills the pipe gets SIGPIPE, as in bash.

Each case name starts with `pipe readers: `.
"""

CASES = [
    (
        "pipe readers: pipefail and errexit with head after a short loop",
        "set -euo pipefail; for i in 1 2 3; do echo $i; done | head -n 1; echo reached",
    ),
    (
        "pipe readers: short writers are not cut off",
        "set -o pipefail\n"
        "for i in 1 2 3; do echo $i; done | head -n 1; echo \"loop $? ${PIPESTATUS[*]}\"\n"
        "printf '%s\\n' a b c | head -1; echo \"printf $?\"\n"
        "seq 3 | head -1; echo \"seq $?\"\n"
        "{ echo a; echo b; echo c; } | head -n 1; echo \"group $?\"\n"
        "f() { echo a; echo b; }; f | head -1; echo \"function $?\"\n"
        "x=$(for i in 1 2 3; do echo $i; done | head -1); echo \"substitution $? $x\"",
    ),
    (
        "pipe readers: writers that fill the pipe get SIGPIPE",
        "set -o pipefail\n"
        "seq 100000 | head -1; echo \"seq $?\"\n"
        "yes | head -1; echo \"yes $?\"\n"
        "for i in $(seq 20000); do echo $i; done | head -1; echo \"loop $? ${PIPESTATUS[*]}\"\n"
        "while :; do echo y; done | head -2; echo \"while $?\"\n"
        "set -e; seq 100000 | head -1; echo not-reached",
    ),
    # However many commands the script ran before, a short writer still finishes first.
    (
        "pipe readers: a short writer is not cut off after other commands",
        "set -o pipefail\n"
        "r=; for k in $(seq 70); do for i in 1 2 3; do echo $i; done | head -n 1 >/dev/null; r=$r$?; :; done; echo \"$r\"",
    ),
]
