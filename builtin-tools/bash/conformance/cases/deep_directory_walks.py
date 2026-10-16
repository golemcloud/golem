"""`grep -r`/`-R` and `diff -r` over a directory tree several hundred levels deep: both walks are
iterative now (one explicit stack frame per directory level, not one native call frame), so
ordinary correctness at this depth is what these check.

These depths are a fast, CI-sized regression check, not the stress case that found the bug:
building a real directory tree this deep costs roughly the *square* of its depth (each `mkdir`
along the way re-resolves its whole path from the sandbox root), which makes the thousands-of-
levels trees that actually used to trap the old native-recursive walk impractically slow to
build here. That reproduction instead lives in the fixing round's own x86-64 rig notes.
"""

DEEP_GREP = (
    'p=$(printf "d/%.0s" $(seq 1 200)); mkdir -p "$p"; echo needle > "$p/f"; '
)
DEEP_DIFF = (
    'p=$(printf "d/%.0s" $(seq 1 220)); mkdir -p a/"$p" b/"$p"; '
    'echo one > a/"$p/f"; echo two > b/"$p/f"; '
)

CASES = [
    (
        "deep walk: grep -r finds a match several hundred levels down",
        f'{DEEP_GREP}grep -rl needle d; echo status=$?',
    ),
    (
        "deep walk: grep -r reports no match the same way at depth",
        f'{DEEP_GREP}grep -rl nomatch d; echo status=$?',
    ),
    (
        "deep walk: diff -rq reports a differing file several hundred levels down",
        f'{DEEP_DIFF}diff -rq a b; echo status=$?',
    ),
]

# Building and walking a couple hundred levels of real directories is inherently slower than the
# matrix's default budget, on both sides.
OPTIONS = {
    "deep walk: grep -r finds a match several hundred levels down": {"timeout": 45},
    "deep walk: grep -r reports no match the same way at depth": {"timeout": 45},
    "deep walk: diff -rq reports a differing file several hundred levels down": {"timeout": 45},
}
