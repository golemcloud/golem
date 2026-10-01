"""`ln -s` and `cp`'s own ways of creating or preserving a symbolic link, when the target text
would resolve above the sandbox root once followed: climbing out of the agent's own filesystem
this way can wedge the whole agent (a separate executor-side defect), so bash-tool refuses to
create the link up front, the same way it already refuses an absolute target.

Real Bash has no sandbox, so every case here is a deliberate divergence -- there is no oracle
answer to record.
"""

CASES = [
    ("symlink escape: ln -s refuses a relative target that climbs above root",
     'mkdir w; cd w; ln -s ../../nonexistent-xyz esc; echo status=$?'),
    ("symlink escape: ln -sf refuses the same way",
     'mkdir w; cd w; : > esc; ln -sf ../../nonexistent-xyz esc; echo status=$?'),
    ("symlink escape: ln -s into a directory refuses per target",
     'mkdir w d; cd w; ln -s ../../a ../../b ../d; echo status=$?'),
    ("symlink escape: ln -s relative target that stays inside root still works",
     'mkdir -p w/a/b; cd w/a/b; ln -s ../../x safe; readlink safe; echo status=$?'),
    ("symlink escape: cp -s is checked the same way ln -s is",
     'mkdir w; cd w; cp -s ../../nonexistent-xyz esc; echo status=$?'),
    ("symlink escape: cp -a preserves a symlink's target text, checked against its new directory",
     'mkdir -p a/b/c; cd a/b/c; ln -s ../x safelink; cd /; cp -a a/b/c/safelink shallow; echo status=$?'),
    ("symlink escape: cp -a of the same symlink into a directory just as deep still works",
     'mkdir -p a/b/c d; ln -s ../x a/b/c/safelink; cp -a a/b/c/safelink d/; readlink d/safelink; echo status=$?'),
]

NO_SANDBOX = (
    "fixture: real Bash has no sandbox root to escape, so it creates the (here, dangling) "
    "symbolic link bash-tool refuses; following one, once created, can wedge the whole agent "
    "(README.md's Limits section)"
)
EXPECTED = {
    'symlink escape: ln -s refuses a relative target that climbs above root': (
        0, b'status=1\n',
        b"ln: failed to create symbolic link to '../../nonexistent-xyz': relative links that "
        b"resolve above the sandbox root are unsupported in bash-tool\n",
        NO_SANDBOX,
    ),
    'symlink escape: ln -sf refuses the same way': (
        0, b'status=1\n',
        b"ln: failed to create symbolic link to '../../nonexistent-xyz': relative links that "
        b"resolve above the sandbox root are unsupported in bash-tool\n",
        NO_SANDBOX,
    ),
    'symlink escape: ln -s into a directory refuses per target': (
        0, b'status=1\n',
        b"ln: failed to create symbolic link to '../../a': relative links that resolve above "
        b"the sandbox root are unsupported in bash-tool\n",
        NO_SANDBOX,
    ),
    'symlink escape: cp -a preserves a symlink\'s target text, checked against its new directory': (
        0, b'status=1\n',
        b"cp: failed to create symbolic link to '../x': relative links that resolve above the "
        b"sandbox root are unsupported in bash-tool\n",
        NO_SANDBOX,
    ),
}

# `cp -s` on real Bash also fails here, but for an unrelated reason (GNU `cp -s`, unlike `ln -s`,
# stats its source first; the oracle's `/etc` exists, bash-tool's sandboxed filesystem does not)
# -- both stderr strings are fixtures, not a shared answer.
CP_S_REASON = (
    "fixture: both sides refuse, but for different reasons -- GNU cp -s stats its source before "
    "linking (the oracle has no such file either way) while bash-tool's refusal is the escape "
    "check itself"
)
EXPECTED_STDERR = {
    'symlink escape: cp -s is checked the same way ln -s is': (
        b"cp: failed to create symbolic link to '../../nonexistent-xyz': relative links that "
        b"resolve above the sandbox root are unsupported in bash-tool\n",
        CP_S_REASON,
    ),
}
