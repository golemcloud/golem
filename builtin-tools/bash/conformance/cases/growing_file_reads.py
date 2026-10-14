"""Reading a file that the same command is writing: `grep`, `sed` and `head -c` must not read
forever, filling the disk and ignoring `timeout`/the call's time limit the way a real OS process
would respect SIGTERM.

`cat f >> f` is already refused this way (see streaming.rs); these are the same hazard in the
other commands whose own read loop can see the just-appended bytes before the call ends.
"""

CASES = [
    # grep: GNU grep itself checks for this (device+inode identity) and refuses up front, so
    # this one matches the oracle exactly rather than needing a fixture.
    (
        "growing file: grep -r refuses when a discovered target is the redirected output",
        'mkdir w; cd w; echo x > a; grep -r x . > out; echo status=$?',
    ),
    (
        "growing file: grep refuses a named file that is the redirected output",
        'mkdir w; cd w; echo x > a; grep x a > a; echo status=$?',
    ),
    # sed and head -c have no such check in GNU, so these rely on `timeout` cutting the call
    # short instead of a diagnostic; see EXPECTED below for why that isn't the oracle's answer.
    (
        "growing file: sed honors timeout instead of reading its own output forever",
        'mkdir w; cd w; echo x > a; timeout 2 sed p a >> a; echo status=$?',
    ),
    (
        "growing file: head -c honors timeout and never reaches its full count",
        'mkdir w; cd w; echo x > a; timeout 2 head -c 100000000 a >> a; echo status=$?',
    ),
]

# GNU sed and head have no same-file check, so on real Bash this finishes almost immediately
# instead of looping: stdio latches end-of-file the first time a read comes up short and never
# re-checks the (by-then-larger) file, so the oracle's sed/head read only the handful of bytes
# that existed when they opened it. bash-tool's own record-at-a-time engines re-poll the
# filesystem every cycle and so never see that incidental end-of-file; they only stop once
# `timeout` cuts them off. Matching the oracle's answer here would mean giving up the fix (and
# its answer is itself a timing accident of its own I/O buffering, not a documented behavior).
# The byte count the file ends up with is wall-clock-dependent on both sides, so neither case
# inspects it; only the exit status (124, GNU timeout's own) is asserted.
YIELDS_TO_TIMEOUT = (
    "fixture: GNU sed/head have no same-file check, so the oracle finishes almost instantly "
    "by accident of its own stdio buffering instead of looping; bash-tool's own loop now yields "
    "so `timeout` can end it instead, which is the fix (see README.md's Limits section)"
)
EXPECTED = {
    'growing file: sed honors timeout instead of reading its own output forever': (
        0, b'status=124\n', b'', YIELDS_TO_TIMEOUT,
    ),
    'growing file: head -c honors timeout and never reaches its full count': (
        0, b'status=124\n', b'', YIELDS_TO_TIMEOUT,
    ),
}
