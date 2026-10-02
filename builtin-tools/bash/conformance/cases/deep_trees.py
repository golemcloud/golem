"""`du` and `rm -r` walking a directory tree many levels deep.

Both utilities used to walk a tree with native recursion, one call per directory level, which
could exhaust the WASM call stack on a tree deep enough (thousands of levels) and take down the
whole call. They now walk with an explicit stack instead. These cases keep the tree shallow
enough to run in the per-PR budget; the stack-depth regression itself is only reachable at a
depth no conformance case can afford, and is checked separately.

`du`'s size and block columns depend on the host filesystem's block size, which differs between
this shell's virtual filesystem and the oracle container's, so these cases strip that column with
`cut -f2` and compare paths only, the same way the existing `du -a` case in `coreutils_edge.py`
does.
"""

DEEP = "/".join(["d"] * 30)

CASES = [
    (
        "deep trees: du -a lists every level of a deep tree, deepest first",
        f"cd /tmp; mkdir -p {DEEP}; printf x > {DEEP}/f; du -a d | cut -f2 | sort; du -s d | cut -f2",
    ),
    (
        "deep trees: du -a of two deep trees keeps each total separate",
        f"cd /tmp; mkdir -p a/{DEEP} b/{DEEP}; printf x > a/{DEEP}/f; printf yy > b/{DEEP}/f; "
        "du -s a b | cut -f2",
    ),
    (
        "deep trees: rm -rvf removes a deep tree bottom-up and leaves nothing behind",
        f"cd /tmp; mkdir -p {DEEP}; printf x > {DEEP}/f; rm -rvf d; echo status=$?; "
        "test -e d && echo still-there || echo gone",
    ),
    (
        "deep trees: rm -r on a deep tree stops for -i like a shallow one",
        f"cd /tmp; mkdir -p {DEEP}; printf x > {DEEP}/f; echo n | rm -ri d >/dev/null 2>&1; "
        "echo status=$?; test -e d && echo still-there || echo gone",
    ),
]
