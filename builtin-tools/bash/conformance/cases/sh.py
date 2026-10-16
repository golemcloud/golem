"""`sh -c` and `bash -c`, which run a script string through this same shell as a child.

In the oracle image `sh` is BusyBox ash, not bash; these cases use only behaviour the two agree
on. Usage errors, which each shell words differently, are covered by the unit tests.
"""

CASES = [
    ("sh: -c with positional arguments", "sh -c 'echo \"$1-$2\" $#' _ a 'b c'"),
    ("bash: -c with positional arguments", "bash -c 'printf \"<%s>\\n\" \"$@\"' _ one 'two words'"),
    (
        "sh: child exit status, cwd and variables stay inside",
        "cd /tmp; sh -c 'cd /; x=1; exit 3'; echo status=$? pwd=$PWD x=${x-unset}",
    ),
    ("bash: stdin reaches the script", "echo piped | bash -c 'cat; echo done'"),
    ("bash: -e stops at the first failure", "bash -ec 'false; echo not-reached'; echo status=$?"),
    ("bash: -o pipefail", "bash -o pipefail -c 'false | true'; echo status=$?"),
    # GNU bash, as a new process, exits 127 on a fatal expansion error under -c; this shell runs
    # the script as a subshell, which exits 1 as `( set -u; ... )` does in bash. Check the option
    # took effect instead.
    ("bash: -u sets the nounset option", "bash -uc 'case $- in *u*) echo nounset;; esac; echo \"${nope-default}\"'"),
    ("sh: script read from stdin with -s", "echo 'echo from-stdin \"$1\"' | sh -s arg"),
    ("sh: nested sh -c", "sh -c 'sh -c \"echo inner \\$1\" _ deep'"),
    (
        "find: exec sh -c per file",
        "mkdir -p /tmp/shx && cd /tmp/shx && touch 'a b' c && "
        "find . -type f -exec sh -c 'echo \"<$1>\"' _ {} \\; | sort",
    ),
    (
        "find: exec bash -c batch",
        "mkdir -p /tmp/shy && cd /tmp/shy && touch x y z && "
        "find . -type f -exec bash -c 'echo $#' _ {} +",
    ),
    ("xargs: bash -c per item", "printf '1\\n2\\n3\\n' | xargs -n 1 bash -c 'echo $(( $1 * 10 ))' _"),
    ("xargs: sh -c exit status", "printf 'a\\n' | xargs sh -c 'exit 7'; echo status=$?"),
]
