"""Error paths for the uutils-backed commands that otherwise have only a happy-path case.

Every case compares the exit status and the exact diagnostic with GNU coreutils: the utility's own
name, the translated message (curly quotes in this shell's UTF-8 locale), and no Rust
`(os error N)` suffix or raw clap usage block.
"""

CASES = [
    # -- checksums and encodings
    ("coreutils errors: md5sum missing file", "md5sum /nope; echo status=$?"),
    ("coreutils errors: sha1sum missing file", "sha1sum /nope; echo status=$?"),
    ("coreutils errors: sha256sum missing file", "sha256sum /nope; echo status=$?"),
    ("coreutils errors: sha512sum missing file", "sha512sum /nope; echo status=$?"),
    ("coreutils errors: b2sum missing file", "b2sum /nope; echo status=$?"),
    ("coreutils errors: cksum missing file", "cksum /nope; echo status=$?"),
    (
        "coreutils errors: sha256sum check reports a mismatch",
        "printf 'a\\n' > /tmp/c; printf '%064d  /tmp/c\\n' 0 | sha256sum -c; echo status=$?",
    ),
    ("coreutils errors: base32 missing file", "base32 /nope; echo status=$?"),
    ("coreutils errors: base64 invalid input", "printf '!!!' | base64 -d; echo status=$?"),
    ("coreutils errors: basenc without an encoding", "basenc </dev/null; echo status=$?"),
    # -- text shaping
    ("coreutils errors: nl missing file", "nl /nope; echo status=$?"),
    ("coreutils errors: paste missing file", "paste /nope; echo status=$?"),
    ("coreutils errors: fold missing file", "fold /nope; echo status=$?"),
    ("coreutils errors: fold invalid width", "echo x | fold -w 0x; echo status=$?"),
    ("coreutils errors: fmt missing file", "fmt /nope; echo status=$?"),
    ("coreutils errors: expand missing file", "expand /nope; echo status=$?"),
    ("coreutils errors: unexpand missing file", "unexpand /nope; echo status=$?"),
    ("coreutils errors: comm missing file", "printf 'a\\n' > /tmp/a; comm /tmp/a /nope; echo status=$?"),
    ("coreutils errors: tsort missing file", "tsort /nope; echo status=$?"),
    ("coreutils errors: tsort odd token count", "echo a b c | tsort; echo status=$?"),
    ("coreutils errors: csplit missing file", "csplit /nope 1; echo status=$?"),
    ("coreutils errors: shuf missing file", "shuf /nope; echo status=$?"),
    # -- numbers, dates and expressions
    ("coreutils errors: expr syntax error", "expr 1 +; echo status=$?"),
    ("coreutils errors: expr non-integer", "expr a + 1; echo status=$?"),
    ("coreutils errors: expr false result", "expr 0; echo status=$?"),
    ("coreutils errors: factor invalid number", "factor x; echo status=$?"),
    ("coreutils errors: numfmt invalid number", "numfmt x; echo status=$?"),
    ("coreutils errors: date invalid date", "date -d 'not a date'; echo status=$?"),
    # -- files and paths
    ("coreutils errors: rmdir missing directory", "rmdir /nope; echo status=$?"),
    ("coreutils errors: rmdir non-empty directory", "mkdir -p /tmp/d/e; rmdir /tmp/d; echo status=$?"),
    ("coreutils errors: unlink missing file", "unlink /nope; echo status=$?"),
    ("coreutils errors: link missing operand", "link /tmp; echo status=$?"),
    ("coreutils errors: realpath -e missing path", "realpath -e /nope/x; echo status=$?"),
    ("coreutils errors: readlink on a regular file", "touch /tmp/r; readlink /tmp/r; echo status=$?"),
    ("coreutils errors: truncate without a size", "truncate /tmp/t; echo status=$?"),
    ("coreutils errors: mktemp in a missing directory", "mktemp /nope/XXXXXX; echo status=$?"),
    ("coreutils errors: env unset needs a name", "env -u; echo status=$?"),
    ("coreutils errors: env unknown option", "env --bogus; echo status=$?"),
    ("coreutils errors: touch invalid date", "touch -d 'not a date' /tmp/x; echo status=$?"),
    ("coreutils errors: rm directory without -r", "mkdir -p /tmp/rd; rm /tmp/rd; echo status=$?"),
    # -- mv, which had no case at all
    ("coreutils errors: mv renames a file", "printf x > /tmp/m1; mv /tmp/m1 /tmp/m2; cat /tmp/m2; ls /tmp/m1 2>/dev/null; echo status=$?"),
    ("coreutils errors: mv into a directory", "mkdir -p /tmp/md; printf y > /tmp/m3; mv /tmp/m3 /tmp/md/; cat /tmp/md/m3"),
    ("coreutils errors: mv missing source", "mv /nope /tmp/x; echo status=$?"),
    # These four ended the whole shell: uutils called `process::exit` on a usage error, which in
    # a WASM agent ends the invocation. The script must carry on with the right status.
    ("coreutils errors: mv one operand keeps the shell alive", "mv /tmp; echo status=$?; echo survived"),
    ("coreutils errors: mktemp unknown option keeps the shell alive", "mktemp --bogus; echo status=$?; echo survived"),
    ("coreutils errors: env unknown option with a command keeps the shell alive", "env --bogus true; echo status=$?; echo survived"),
    ("coreutils errors: uniq unknown option keeps the shell alive", "uniq --bogus </dev/null; echo status=$?; echo survived"),
]
