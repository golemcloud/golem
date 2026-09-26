"""Conformance for the additional coreutils (uu_* backed) plus our own yes/seq/rev/tac.

Scripts are written so every comparison is deterministic: nondeterministic commands (mktemp,
shuf, date's wall clock) are wrapped so only a fixed-content check is printed, never raw output
that would differ between the wasm shell and the oracle even when both are "correct".
"""

CASES = [
    # -- basename / dirname / realpath / readlink -----------------------------------------
    ("basename: strips directory and suffix", "basename /usr/bin/sort; basename foo.txt .txt"),
    ("dirname: strips the last component", "dirname /usr/bin/sort; dirname a/b/c"),
    (
        "realpath: resolves relative and .. components",
        "mkdir -p /tmp/rp/a/b; cd /tmp/rp/a/b && realpath ../b && realpath .",
    ),
    (
        "readlink: prints a symlink target",
        "printf x > /tmp/rl-target; ln -s rl-target /tmp/rl-link; readlink /tmp/rl-link",
    ),
    # -- ln / link / unlink / rmdir --------------------------------------------------------
    (
        "ln: hard link shares content",
        "printf hard > /tmp/ln-src; ln /tmp/ln-src /tmp/ln-dst; cat /tmp/ln-dst",
    ),
    (
        "ln: symlink resolves via readlink",
        "printf soft > /tmp/lns-target; ln -s lns-target /tmp/lns-link; readlink /tmp/lns-link; cat /tmp/lns-link",
    ),
    (
        "link: creates a second directory entry for the same file",
        "printf x > /tmp/link-src; link /tmp/link-src /tmp/link-dst; cat /tmp/link-dst",
    ),
    (
        "unlink: removes exactly one file",
        "printf x > /tmp/unlink-f; unlink /tmp/unlink-f; if test -e /tmp/unlink-f; then echo exists; else echo gone; fi",
    ),
    (
        "rmdir: removes an empty directory",
        "mkdir /tmp/rmdir-d; rmdir /tmp/rmdir-d; if test -d /tmp/rmdir-d; then echo exists; else echo gone; fi",
    ),
    # -- mktemp / truncate ------------------------------------------------------------------
    (
        "mktemp: creates a fresh file and directory",
        'f=$(mktemp); if test -f "$f"; then echo file-ok; fi; rm -f "$f"; '
        'd=$(mktemp -d); if test -d "$d"; then echo dir-ok; fi; rmdir "$d"',
    ),
    (
        "truncate: shrinks a file to an exact size",
        "printf '123456789' > /tmp/trunc-f; truncate -s 4 /tmp/trunc-f; cat /tmp/trunc-f; echo; wc -c < /tmp/trunc-f",
    ),
    # -- nl / paste / join / comm -----------------------------------------------------------
    ("nl: numbers non-blank lines", "printf 'a\\n\\nb\\nc\\n' | nl"),
    (
        "paste: merges lines of two files column-wise",
        "printf 'a\\nb\\n' > /tmp/paste-1; printf '1\\n2\\n' > /tmp/paste-2; paste /tmp/paste-1 /tmp/paste-2",
    ),
    (
        "join: merges on a common sorted field",
        "printf '1 a\\n2 b\\n3 c\\n' > /tmp/join-1; printf '1 x\\n2 y\\n4 z\\n' > /tmp/join-2; join /tmp/join-1 /tmp/join-2",
    ),
    (
        "comm: three-column set comparison",
        "printf 'a\\nb\\nc\\n' > /tmp/comm-1; printf 'b\\nc\\nd\\n' > /tmp/comm-2; comm /tmp/comm-1 /tmp/comm-2",
    ),
    # -- fold / fmt / expand / unexpand ------------------------------------------------------
    ("fold: wraps at a fixed width", "printf '1234567890\\n' | fold -w4"),
    (
        "fmt: fills short lines into a paragraph",
        "printf 'one two three four five six\\n' | fmt -w 10",
    ),
    ("expand: tabs become spaces", "printf 'a\\tb\\tc\\n' | expand -t 4"),
    ("unexpand: runs of spaces become tabs", "printf 'a       b\\n' | unexpand -a | cat -A"),
    # -- tsort / split / csplit ---------------------------------------------------------------
    ("tsort: topological order of a DAG", "printf 'a b\\nb c\\na c\\n' | tsort"),
    (
        "split: splits a file into fixed-size line chunks",
        "printf '1\\n2\\n3\\n4\\n' > /tmp/split-src; cd /tmp && split -l 2 split-src split-part-; cat split-part-aa; echo ---; cat split-part-ab",
    ),
    (
        "csplit: splits a file at a line number",
        "printf '1\\n2\\n3\\n4\\n' > /tmp/csplit-src; cd /tmp && csplit -z csplit-src 3; cat xx00; echo ---; cat xx01",
    ),
    # -- base64 / base32 / basenc -------------------------------------------------------------
    ("base64: round-trips through encode and decode", "printf hello | base64; printf hello | base64 | base64 -d"),
    ("base32: encodes to the expected alphabet", "printf hello | base32"),
    ("basenc: base64 mode matches base64", "printf hello | basenc --base64"),
    # -- checksums ------------------------------------------------------------------------------
    ("md5sum: hashes fixed content", "printf hello | md5sum"),
    ("sha1sum: hashes fixed content", "printf hello | sha1sum"),
    (
        "sha256sum: hashes and self-checks",
        "printf hello > /tmp/sha-f; sha256sum /tmp/sha-f > /tmp/sha-f.sha256; cd /tmp && sha256sum -c sha-f.sha256",
    ),
    ("sha512sum: hashes fixed content", "printf hello | sha512sum"),
    ("b2sum: hashes fixed content", "printf hello | b2sum"),
    ("cksum: checksums and counts a file", "printf hello | cksum"),
    # -- od / date / expr / factor / numfmt ----------------------------------------------------
    ("od: dumps bytes as hex with no offset column", "printf AB | od -An -tx1"),
    (
        "date: formats a fixed epoch in UTC",
        "date -u -d @0 +'%Y-%m-%d %H:%M:%S'; date -u -d @1700000000 +%Y-%m-%d",
    ),
    (
        "date: +%s round-trips through -d @",
        "s=$(date -u -d @1700000000 +%s); if test \"$s\" = 1700000000; then echo ok; fi",
    ),
    ("expr: arithmetic and string length", "expr 6 + 3; expr 10 / 3; expr length hello"),
    ("factor: prints the prime factorization", "factor 60"),
    ("numfmt: reformats to human-readable IEC units", "numfmt --to=iec 1500000"),
    (
        # No fixed --random-source: the WASI sandbox has no /dev, so only the multiset/count
        # survives sorting — never the actual permutation, which is expected to differ.
        "shuf: permutes without changing the multiset",
        "seq 1 5 | shuf -n 3 | sort -n | wc -l | tr -d ' '",
    ),
    # -- yes / seq / rev / tac (custom streaming commands) --------------------------------------
    ("yes: repeats y forever until the reader stops", "yes | head -n 3"),
    ("yes: repeats the given words", "yes a b c | head -n 2"),
    ("seq: default integer range", "seq 1 5"),
    ("seq: single-argument form counts from 1", "seq 3"),
    ("seq: decimal increment", "seq 1 0.5 2"),
    ("seq: descending range with a negative increment", "seq 5 -1 1"),
    ("seq: equal-width zero pads to the widest endpoint", "seq -w 1 10"),
    ("seq: custom separator only appears between terms", "seq -s, 1 5"),
    ("seq: streams instead of materializing a huge range", "seq 1 1000000000 | head -n 1"),
    ("rev: reverses ascii lines", "printf 'hello\\nworld\\n' | rev"),
    ("rev: reverses UTF-8 characters, not bytes", "printf 'h\\xc3\\xa9llo\\n' | rev"),
    ("rev: terminates against an endless producer", "while :; do echo x; done | rev | head -n 1"),
    (
        "rev: continues past a missing file without the os-error suffix",
        "printf 'ab\\n' > /tmp/rev-multi; rev /tmp/rev-multi /nonexist /tmp/rev-multi; echo status=$?",
    ),
    ("tac: reverses line order", "printf 'a\\nb\\nc\\n' | tac"),
    ("tac: a missing final newline fuses onto the previous line", "printf 'a\\nb\\nc' | tac"),
    ("tac: unsupported regex separator refuses", "printf 'a\\nb\\n' | tac -r"),
    (
        "tac: reverses several files independently in operand order",
        "printf 'a\\nb\\n' > /tmp/tac-t1; printf 'c\\nd\\n' > /tmp/tac-t2; tac /tmp/tac-t1 /tmp/tac-t2",
    ),
    (
        "tac: reports a missing file and keeps going",
        "printf 'a\\nb\\n' > /tmp/tac-t3; tac /nonexist /tmp/tac-t3; echo status=$?",
    ),
    # -- broken-pipe policy: matches GNU with SIGPIPE ignored, and the default 141 pipeline status
    (
        "yes: reports broken pipe when SIGPIPE is ignored",
        "trap '' PIPE; yes | head -n 1; echo \"${PIPESTATUS[*]}\"",
    ),
    (
        "yes: default disposition still yields the synthetic 141 status",
        "set -o pipefail; yes | head -n 2; echo \"$? ${PIPESTATUS[*]}\"",
    ),
    (
        "seq: reports broken pipe when SIGPIPE is ignored",
        "trap '' PIPE; seq 1 100000 | head -n 1; echo \"${PIPESTATUS[*]}\"",
    ),
    (
        "seq: default disposition still yields the synthetic 141 status",
        "set -o pipefail; seq 1 100000 | head -n 1; echo \"$? ${PIPESTATUS[*]}\"",
    ),
    # -- seq -f: real C %e/%g, not Rust Display or lexical-string carry detection
    ("seq: -f exponential format matches C printf", "seq -f %e 1 2"),
    ("seq: -f general format strips trailing zeros", "seq -f %g 1 0.1 1.3"),
    # -- seq diagnostics: exact GNU wording, curly quotes included
    ("seq: invalid float argument diagnostic matches GNU", "seq x; echo status=$?"),
    ("seq: zero increment diagnostic matches GNU", "seq 1 0 3; echo status=$?"),
    ("seq: missing operand diagnostic matches GNU", "seq; echo status=$?"),
    ("seq: extra operand diagnostic matches GNU", "seq 1 2 3 4; echo status=$?"),
    # -- date's clock resolution, and setting the clock as an unprivileged user
    ("date: --resolution", "date --resolution; echo status=$?"),
    ("date: -s cannot set the clock", "date -u -s @0; echo status=$?; date -u -s '2020-01-01'; echo status=$?"),
    # -- sort -c / -C / -m: a single-threaded path on WASI, which has no threads
    (
        "sort: -c reports the first disorder",
        "printf 'a\\nb\\n' | sort -c; echo status=$?; printf 'a\\nc\\nb\\na\\n' | sort -c; echo status=$?; "
        "printf 'a\\na\\n' | sort -cu; echo status=$?; sort -c /dev/null; echo status=$?; "
        "printf 'x 2\\ny 10\\n' | sort -c -k2n; echo status=$?",
    ),
    ("sort: -C is silent", "printf 'b\\na\\n' | sort -C; echo status=$?; printf 'a\\nb\\n' | sort -C; echo status=$?"),
    (
        "sort: -m merges sorted inputs",
        "printf '1\\n3\\n5\\n' >/tmp/a; printf '2\\n3\\n4\\n' >/tmp/b; printf '' >/tmp/e; "
        "sort -m /tmp/a /tmp/b /tmp/e; echo status=$?; sort -mu /tmp/a /tmp/b; echo status=$?; "
        "printf '5\\n3\\n1\\n' >/tmp/c; sort -m -nr /tmp/c - <<<$'4\\n2'; echo status=$?",
    ),
]

# bash-tool refuses tac's regex-separator mode outright (exit 2); the oracle would instead run
# GNU tac's regex-separator mode. -s/-b for a fixed-string separator are implemented for real
# (see text_tools.py's own tac cases), so the refusal wording now says so specifically.
EXPECTED_REASON = "bash-tool refuses tac's regex-separator mode; only a fixed-string separator is implemented"
EXPECTED = {
    "tac: unsupported regex separator refuses": (
        2,
        b"",
        b"tac: -r/--regex is unsupported in bash-tool (only a fixed-string separator is implemented)\n",
    ),
}
