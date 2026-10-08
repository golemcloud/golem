"""Several sibling tool calls in one script, through the in-process `probe-tool` fixture.

`probe-tool delay` answers after 100 ms, `fail` at once with status 7. The oracle has no such
tool, so every case here is a fixture: what bash would do given a command with that behaviour.
"""

CASES = [
    (
        "siblings: concurrent calls keep their own statuses",
        "probe-tool delay >/tmp/d & a=$!; probe-tool fail 2>/tmp/f & b=$!; "
        "wait $a; echo a=$?; wait $b; echo b=$?; cat /tmp/d /tmp/f",
    ),
    (
        "siblings: wait -n reports the first call to finish",
        "probe-tool delay >/dev/null & probe-tool fail 2>/dev/null & wait -n; echo first=$?; wait; echo all=$?",
    ),
    (
        "siblings: kill cancels one pending call and the other completes",
        "probe-tool delay >/tmp/k & k=$!; probe-tool delay >/tmp/o & o=$!; "
        "kill $k; wait $k; echo killed=$?; wait $o; echo other=$?; cat /tmp/o; cat /tmp/k 2>/dev/null; echo end",
    ),
]

EXPECTED_REASON = "probe-tool is a fixture of the cooperative example; the oracle has no such command"
EXPECTED = {
    "siblings: concurrent calls keep their own statuses": (
        0,
        b"a=0\nb=7\ncompleted:peer=false\nnamed fixture error\n",
        b"",
    ),
    "siblings: wait -n reports the first call to finish": (0, b"first=7\nall=0\n", b""),
    # The killed call wrote nothing: its output would only have arrived on completion. (Its
    # redirect may not even have been opened, as in bash, so the file is read quietly.)
    "siblings: kill cancels one pending call and the other completes": (
        0,
        b"killed=143\nother=0\ncompleted:peer=false\nend\n",
        b"",
    ),
}
