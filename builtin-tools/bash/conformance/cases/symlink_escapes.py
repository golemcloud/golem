"""`ln -s` and `cp` creating or preserving a symbolic link whose relative target climbs above the
agent's root. Only the link's text is stored, so the link is created, as in bash.
"""

CASES = [
    ("symlink escape: ln -s creates a relative target that climbs above root",
     'mkdir w; cd w; ln -s ../../nonexistent-xyz esc; echo status=$?; readlink esc'),
    ("symlink escape: ln -sf replaces a file with such a link",
     'mkdir w; cd w; : > esc; ln -sf ../../nonexistent-xyz esc; echo status=$?; readlink esc'),
    ("symlink escape: ln -s into a directory creates one link per target",
     'mkdir w d; cd w; ln -s ../../a ../../b ../d; echo status=$?; readlink ../d/a ../d/b'),
    ("symlink escape: ln -s relative target that stays inside root still works",
     'mkdir -p w/a/b; cd w/a/b; ln -s ../../x safe; readlink safe; echo status=$?'),
    ("symlink escape: cp -s of a source above root fails",
     'mkdir w; cd w; cp -s ../../nonexistent-xyz esc; echo status=$?'),
    ("symlink escape: cp -a recreates a symlink's target text in a shallower directory",
     'mkdir -p a/b/c; cd a/b/c; ln -s ../x safelink; cd /; cp -a a/b/c/safelink shallow; echo status=$?; readlink shallow'),
    ("symlink escape: cp -a of the same symlink into a directory just as deep still works",
     'mkdir -p a/b/c d; ln -s ../x a/b/c/safelink; cp -a a/b/c/safelink d/; readlink d/safelink; echo status=$?'),
]

# GNU `cp -s`, unlike `ln -s`, stats its source first, and both sides fail there. The oracle's
# root is its own parent, so it finds no such file; WASI refuses a path above the agent's root.
CP_S_REASON = (
    "fixture: both sides fail to stat the source -- the oracle reports a missing file, WASI "
    "refuses a path above the agent's root"
)
EXPECTED_STDERR = {
    'symlink escape: cp -s of a source above root fails': (
        b"cp: Operation not permitted (os error 63)\n",
        CP_S_REASON,
    ),
}
