"""`file`, `stat`, `which` and `man`: the commands that inspect files and the command set.

`file` is compared with GNU file 5.47 for the cases it describes without a magic database: text
and its line terminators, scripts, empty files, directories, links, missing files and the MIME
forms. `man` has no GNU counterpart in the oracle image, so its cases are fixtures.
"""

FILES = (
    "cd /tmp && printf 'x\\n' > t && printf 'hello' > nonl && printf 'a\\r\\nb\\r\\n' > crlf && "
    "printf 'a\\r\\nb\\n' > mixed && printf 'h\\303\\251\\n' > utf && : > e && mkdir -p d && "
    "ln -sf t link && printf '#!/bin/sh\\necho\\n' > s.sh && "
    "printf '#!/usr/bin/env bash\\necho\\n' > b.sh"
)

CASES = [
    ("inspect: file describes text", f"{FILES}; file t; file nonl; file crlf; file mixed; file utf"),
    ("inspect: file describes empty files, directories and links", f"{FILES}; file e; file d; file link"),
    ("inspect: file describes scripts", f"{FILES}; file s.sh; file b.sh"),
    ("inspect: file aligns several names", f"{FILES}; file t e d link"),
    ("inspect: file brief", f"{FILES}; file -b t d"),
    ("inspect: file mime type", f"{FILES}; file --mime-type t e d s.sh"),
    ("inspect: file mime with charset", f"{FILES}; file -i t utf"),
    ("inspect: file describes a missing file and succeeds", "file /nope; echo status=$?"),
    (
        "inspect: file recognises gzip data",
        "printf '\\037\\213\\010\\000\\000\\000\\000\\000' > /tmp/g; file /tmp/g",
    ),
    ("inspect: file resolves names against the shell's cwd", "mkdir -p /tmp/w && cd /tmp/w && printf 'x\\n' > here && file here"),
    ("inspect: stat format", "printf hello > /tmp/f; mkdir -p /tmp/dd; stat -c '%s %F %n' /tmp/f; stat -c '%F' /tmp/dd"),
    ("inspect: stat missing file", "stat /nope; echo status=$?"),
    ("inspect: which of a missing command", "which nosuchcmd; echo status=$?"),
    ("man: page for a command", "man ls | head -n 1"),
    ("man: missing page", "man nosuchpage; echo status=$?"),
]

EXPECTED_REASON = "the oracle image has no man; these pin this shell's own pages"
EXPECTED = {
    "man: page for a command": (0, b"ls(1)\n", b""),
    "man: missing page": (0, b"status=1\n", b"No manual entry for nosuchpage\n"),
}
