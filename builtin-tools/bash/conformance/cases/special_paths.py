"""Every file-reading command with /dev/null, /dev/stdin and `-` operands; writers with /dev/stdout.

Generated: a command added to FILTERS or PAIRS below is checked against all three input forms.
"""

INPUT = "printf 'b\\na\\nb\\n'"

# Commands that read one file operand, with the arguments that make them do something.
FILTERS = {
    "b2sum": "", "base32": "", "base64": "", "basenc": "--base64", "cat": "", "cksum": "",
    "cut": "-c1", "expand": "", "file": "-b", "fmt": "", "fold": "-w1", "grep": "b", "head": "-n1",
    "jq": "-R .", "md5sum": "", "nl": "", "od": "-c", "rev": "", "sed": "p", "sha1sum": "",
    "sha256sum": "", "sha512sum": "", "sort": "", "tac": "", "tail": "-n1", "tsort": "",
    "unexpand": "", "uniq": "", "wc": "", "csplit": "-s -f /tmp/cs", "split": "-l1",
    "shuf": "-n0", "stat": "-c %F", "readlink": "-f", "realpath": "",
}

# Commands that read two file operands.
PAIRS = {"cmp": "", "comm": "", "diff": "", "join": "", "paste": ""}

# readlink/realpath's whole job on /dev/stdin is printing the path it resolves to, and on the
# oracle that's a pipe under /proc/PID -- a fresh PID (and thus a fresh, unrecordable answer)
# every single run, even with nothing else in the script changed. Testing the property a real
# resolution has (matches /proc/*/fd/*) rather than the literal path keeps the case meaningful
# (an agent has no /proc, so it never matches) without embedding that PID in the recorded golden.
PROC_FD_PROPERTY = {"readlink", "realpath"}

CASES = []
for command, args in FILTERS.items():
    prefix = f"{command} {args}".strip()
    if command in PROC_FD_PROPERTY:
        stdin_case = f"{INPUT} | [[ $({prefix} /dev/stdin) == /proc/*/fd/* ]] && echo match; echo status=$?"
    else:
        stdin_case = f"{INPUT} | {prefix} /dev/stdin; echo status=$?"
    CASES += [
        (f"special paths: {command} /dev/null", f"{prefix} /dev/null; echo status=$?", ["redirection.dev-paths"]),
        (f"special paths: {command} /dev/stdin", stdin_case, ["redirection.dev-paths"]),
        (f"special paths: {command} dash", f"{INPUT} | {prefix} -; echo status=$?"),
    ]
for command, args in PAIRS.items():
    prefix = f"{command} {args}".strip()
    CASES += [
        (f"special paths: {command} /dev/null twice", f"{prefix} /dev/null /dev/null; echo status=$?", ["redirection.dev-paths"]),
        (f"special paths: {command} stdin and a file", f"printf 'a\\n' >/tmp/f; {INPUT} | {prefix} - /tmp/f; echo status=$?"),
    ]

CASES += [
    ("special paths: sort -o /dev/stdout", f"{INPUT} | sort -o /dev/stdout; echo status=$?", ["redirection.dev-paths"]),
    ("special paths: tee /dev/stderr", f"{INPUT} | tee /dev/stderr >/dev/null; echo status=$?", ["redirection.dev-paths"]),
    ("special paths: cp to /dev/stdout", "printf 'x\\n' >/tmp/f; cp /tmp/f /dev/stdout; echo status=$?", ["redirection.dev-paths"]),
    ("special paths: sed w /dev/stdout", f"{INPUT} | sed -n 'w /dev/stdout'; echo status=$?", ["redirection.dev-paths"]),
    ("special paths: cat /dev/fd/0", f"{INPUT} | cat /dev/fd/0; echo status=$?", ["redirection.dev-paths"]),
    ("special paths: redirect into /dev/stdout", "echo out >/dev/stdout; echo err 2>/dev/null >/dev/stderr", ["redirection.dev-paths"]),
    ("special paths: find /dev/null", "find /dev/null; echo status=$?", ["redirection.dev-paths"]),
    ("special paths: ls /dev/null", "ls /dev/null; echo status=$?", ["redirection.dev-paths"]),
    ("special paths: test on /dev/stdin", "[ -e /dev/stdin ] && echo stdin; [ -e /dev/fd/1 ] && echo fd1; true", ["redirection.dev-paths"]),
]

EXPECTED = {}

PROC_PATHS = 'the oracle resolves /dev/stdin to a real /proc/PID/fd/N pipe path (the case only checks that property now, not the literal path, since the PID changes every run); an agent has no /proc, so it never matches'
EXPECTED |= {
    'special paths: readlink /dev/stdin': (
        0, b'status=1\n', b'', PROC_PATHS,
    ),
    'special paths: realpath /dev/stdin': (
        0, b'status=1\n', b'realpath: /dev/stdin: No such file or directory\n', PROC_PATHS,
    ),
}
