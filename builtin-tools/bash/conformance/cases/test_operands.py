"""`test` and `[` over long operand lists, runs of `!` and nested parentheses, with bash's syntax
errors; runs of `!` in `[[ ]]`. Lists and runs are read in loops, however long; only parentheses
nest, up to a limit.

Each case name starts with `test operands: `.
"""

CASES = [
    (
        "test operands: 5000 operands joined by -a",
        "test x $(printf -- '-a x %.0s' {1..5000}); echo rc=$?; [ x $(printf -- '-a x %.0s' {1..5000}) ]; echo rc=$?\n"
        "test x $(printf -- '-a x %.0s' {1..5000}) -a ''; echo rc=$?",
    ),
    (
        "test operands: 5000 empty operands joined by -o",
        "a=(); for i in $(seq 5000); do a+=('' -o); done; test \"${a[@]}\" ''; echo rc=$?; a+=(x); test \"${a[@]}\"; echo rc=$?",
    ),
    (
        "test operands: a run of 2000 ! before an operand",
        "test $(printf '%.0s! ' {1..2000}) x; echo rc=$?; test $(printf '%.0s! ' {1..2001}) x; echo rc=$?\n"
        "[ $(printf '%.0s! ' {1..2001}) -n '' -a x ]; echo rc=$?",
    ),
    (
        "test operands: -f over 8000 files joined by -a and -o",
        "mkdir /tmp/many && cd /tmp/many && touch $(seq -f f%g 8000)\n"
        "a=($(printf -- '-f f%s -a ' $(seq 8000)) -f f1); test \"${a[@]}\"; echo rc=$?\n"
        "a=($(printf -- '-f g%s -o ' $(seq 8000)) -f f1); [ \"${a[@]}\" ]; echo rc=$?\n"
        "a=($(printf -- '-f f%s -a ' $(seq 8000)) -f nope); test \"${a[@]}\"; echo rc=$?",
    ),
    # Bash evaluates every operand of -a and -o, so an invalid one fails the test even after
    # the others have decided it.
    (
        "test operands: every operand of -a and -o is evaluated",
        "test 1 -eq 1 -o x -eq 1; echo rc=$?; test 1 -eq 2 -a x -eq 1; echo rc=$?; test x -eq 1 -o 1 -eq 1; echo rc=$?",
    ),
    (
        "test operands: syntax errors are worded as bash words them",
        "t() { test \"$@\"; echo \"[$*] $?\"; }\n"
        "t a -a b -a !; t a -a b -o; t '(' a -a b; t a -a '(' b; t '(' = '(' -a x; t -z -o -n x; t a b c d\n"
        "t a b; t '(' a b; t ! a b; t '(' a b ')'; t ! a b c; t ! '(' a b; t ! ! a b; t '(' ! a ')'",
    ),
    (
        "test operands: parentheses nest 64 deep",
        "test $(printf '( %.0s' {1..64}) -n x -a y $(printf ') %.0s' {1..64}); echo rc=$?",
    ),
    (
        "test operands: parentheses nested deeper are refused",
        "test $(printf '( %.0s' {1..65}) -n x -a y $(printf ') %.0s' {1..65}); echo rc=$?\n"
        "[ $(printf '( %.0s' {1..2000}) -n x -a y $(printf ') %.0s' {1..2000}) ]; echo rc=$?",
    ),
    # Bash reads a run of `!` in `[[ ]]` as one negation or none, and prints it that way.
    (
        "test operands: a run of 3000 ! in [[ ]]",
        "eval \"[[ $(printf '! %.0s' {1..3000}) a ]]\"; echo rc=$?; eval \"[[ $(printf '! %.0s' {1..3001}) a ]]\"; echo rc=$?\n"
        "f() { [[ ! ! a ]]; [[ ! ! ! -n '' ]]; }; declare -f f; f; echo rc=$?",
    ),
]

# Creating and testing 8,000 files is slow under emulation.
OPTIONS = {"test operands: -f over 8000 files joined by -a and -o": {"timeout": 60}}

NESTING = b"maximum nesting level exceeded: deeper nesting is unsupported in bash-tool\n"
EXPECTED = {
    "test operands: parentheses nested deeper are refused": (
        0,
        b"rc=2\nrc=2\n",
        b"bash: line 1: test: " + NESTING + b"bash: line 2: [: " + NESTING,
        "bash-tool's limit: test reads and evaluates each level of parentheses recursively, so it "
        "refuses more than 64 (README, Limits); bash reads 2,000",
    ),
}
