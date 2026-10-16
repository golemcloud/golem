"""The shell's memory budget: what a script builds in memory, and where bash-tool stops it.

Bash allocates until `malloc` fails, then ends with `xmalloc: cannot allocate N bytes` (status 2),
or, for a brace expansion it cannot allocate, fails just that expansion. An agent's memory is
capped, and running out of it traps the agent, so bash-tool keeps the shell's own data within a
budget and ends the same way when a script asks for more (README.md, "Limits"). Bash, with the
memory of its machine, finishes each of these.

Where a case's refusal names a byte count that depends on what the shell held at the time, its
stderr is piped through `sed` so the golden does not change with unrelated allocations.

Each case name starts with `memory: `.
"""

# Prints the stage's output with the byte count of a refusal replaced by `N`.
NORMALIZE = " 2>&1 | sed 's/allocate [0-9]* bytes/allocate N bytes/'"

# 50 MB of `y` lines.
LARGE = "x=$(yes | head -c 50000000)"

CASES = [
    # Within the budget: what a script plausibly builds still works.
    ("memory: a 50 MB variable and its copy", LARGE + "; y=$x; echo ${#x} ${#y}"),
    (
        "memory: an array of 100,000 elements",
        "for ((i = 0; i < 100000; i++)); do a[i]=element-$i; done; echo ${#a[@]} ${a[99999]}",
    ),
    (
        "memory: a brace expansion of a million words",
        "a=({1..1000000}); echo ${#a[@]} ${a[999999]}",
    ),
    # Past it.
    (
        "memory: a value that keeps doubling ends the script",
        "x=y; for i in {1..28}; do x=$x$x; done; echo ${#x}",
    ),
    (
        "memory: appending to an array past the budget ends its subshell",
        LARGE + "; (for i in {1..9}; do a+=(\"$x\"); done; echo ${#a[@]})" + NORMALIZE
        + "; echo \"status=${PIPESTATUS[0]}\"",
    ),
    (
        "memory: many large variables end their subshell",
        LARGE + "; (for i in {1..9}; do declare \"v$i=$x\"; done; echo done)" + NORMALIZE
        + "; echo \"status=${PIPESTATUS[0]}\"",
    ),
    (
        "memory: a brace expansion past the budget ends the script",
        "a=({1..3000000}); echo after",
    ),
    (
        "memory: text too large to parse ends its subshell",
        "s=$(yes 'echo hi;' | head -n 400000); { eval \"$s\"; }" + NORMALIZE
        + " | tail -n 1; echo \"status=${PIPESTATUS[0]}\"",
    ),
]

# Bash builds these tens or hundreds of megabytes in a few seconds; under a loaded run, longer.
OPTIONS = {name: {"timeout": 60} for name, _ in CASES}

BUDGET = (
    "fixture: bash-tool keeps the shell's memory within a budget (README.md, \"Limits\") and "
    "refuses past it as bash does when malloc fails, where bash, with the memory of its machine, "
    "finishes"
)
OVER = b"shell memory over 384 MiB is unsupported in bash-tool"
EXPECTED = {
    "memory: a value that keeps doubling ends the script": (
        2, b"", b"bash: xmalloc: cannot allocate 134217744 bytes: " + OVER + b"\n", BUDGET,
    ),
    "memory: appending to an array past the budget ends its subshell": (
        0, b"bash: xmalloc: cannot allocate N bytes: " + OVER + b"\nstatus=2\n", b"", BUDGET,
    ),
    "memory: many large variables end their subshell": (
        0, b"bash: xmalloc: cannot allocate N bytes: " + OVER + b"\nstatus=2\n", b"", BUDGET,
    ),
    # Bash leaves a sequence it cannot allocate unexpanded and runs the command with it; bash-tool
    # fails the expansion as it fails any other, which ends the script.
    "memory: a brace expansion past the budget ends the script": (
        1,
        b"",
        b"bash: line 1: brace expansion: failed to allocate memory for 3000000 elements: "
        + OVER + b"\n",
        BUDGET,
    ),
    "memory: text too large to parse ends its subshell": (
        0, b"bash: xmalloc: cannot allocate N bytes: " + OVER + b"\nstatus=2\n", b"", BUDGET,
    ),
}
