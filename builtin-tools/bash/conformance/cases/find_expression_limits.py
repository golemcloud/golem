"""`find`'s expression compiler and its parser's real-nesting (`(`/`!`/`-not`) depth.

A flat `-a`/`-o`/`,' chain, however long, parses and compiles as a loop and so never counts
against the nesting guard; only genuine parenthesised or negated nesting does, since that is
the shape that used to recurse once per level in both the parser and `emit`.
"""

FLAT_OR_CHAIN = (
    'touch f; args=(); for i in $(seq 1 8000); do args+=(-name "nomatch$i" -o); done; '
    'find . -maxdepth 1 "${args[@]}" -name f -print'
)
NESTED_PARENS = (
    'o=$(printf "( %.0s" $(seq 1 NEST)); c=$(printf ") %.0s" $(seq 1 NEST)); '
    'find . -maxdepth 0 $o -true $c'
)

CASES = [
    (
        "find nesting: a flat 8000-name -o chain is not nesting",
        f'mkdir w; cd w; {FLAT_OR_CHAIN}; echo status=$?',
    ),
    (
        "find nesting: 900 levels of real parentheses still compiles",
        f'mkdir w; cd w; {NESTED_PARENS.replace("NEST", "900")}; echo status=$?',
    ),
    (
        "find nesting: 1001 levels of real parentheses is refused cleanly",
        f'mkdir w; cd w; {NESTED_PARENS.replace("NEST", "1001")}; echo status=$?',
    ),
]

# Generating 8,000 arguments (and GNU find parsing them) is slower than the default budget.
OPTIONS = {
    "find nesting: a flat 8000-name -o chain is not nesting": {"timeout": 60},
}

# GNU findutils' own parser doesn't recurse the same way `emit` and this fork's recursive-descent
# parser used to, so it has no comparable depth limit; past the point bash-tool must refuse to
# protect its native stack, real `find` keeps working. Verified against the oracle.
NESTING_LIMIT = (
    "fixture: find's own recursive-descent parser and bytecode compiler would overflow the "
    "native stack past this depth (see README.md's Limits section); GNU findutils has no "
    "comparable limit"
)
EXPECTED = {
    'find nesting: 1001 levels of real parentheses is refused cleanly': (
        0, b'status=2\n',
        b'find: maximum nesting level exceeded: deeper nesting is unsupported in bash-tool\n',
        NESTING_LIMIT,
    ),
}
