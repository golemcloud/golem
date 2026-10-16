"""Diagnostics from uutils-backed and streaming commands, compared byte for byte with GNU.

Each uutils utility runs in-process, so it must be told its own name (argv[0] is the shell) and
load its own translations. These cases also pin that switching between utilities, including
across pipeline stages that interleave on one thread, never shows another utility's name or a
raw translation key, and that I/O errors carry no Rust `(os error N)` suffix.
"""

CASES = [
    # -- usage errors: translated message, the utility's name, and the --help hint
    ("diagnostics: dirname missing operand", "dirname; echo status=$?"),
    ("diagnostics: basename missing operand", "basename; echo status=$?"),
    ("diagnostics: cut without a list", "cut; echo status=$?"),
    ("diagnostics: tr missing operand", "tr; echo status=$?"),
    # -- missing files through uu_* entry points
    ("diagnostics: ls missing file", "ls /nope; echo status=$?"),
    ("diagnostics: cp missing source", "cp /nope /tmp/x; echo status=$?"),
    ("diagnostics: rm missing file", "rm /nope; echo status=$?"),
    ("diagnostics: touch in a missing directory", "touch /nope/x; echo status=$?"),
    ("diagnostics: sort missing file", "sort /nope; echo status=$?"),
    ("diagnostics: od missing file", "od /nope; echo status=$?"),
    # -- missing files through the streaming drivers
    ("diagnostics: cat missing file", "cat /nope; echo status=$?"),
    ("diagnostics: head missing file", "head /nope; echo status=$?"),
    ("diagnostics: tail missing file", "tail /nope; echo status=$?"),
    ("diagnostics: wc missing file", "wc /nope; echo status=$?"),
    ("diagnostics: uniq missing file", "uniq /nope; echo status=$?"),
    ("diagnostics: cut missing file", "cut -f1 /nope; echo status=$?"),
    ("diagnostics: tee into a missing directory", "echo x | tee /nope/x; echo status=$?"),
    # -- switching utilities keeps each one's name and strings
    (
        "diagnostics: alternating utilities keep their own names",
        "ls /nope; dirname; ls /nope2; basename; cut; echo status=$?",
    ),
    (
        "diagnostics: pipeline stages keep their own names",
        "ls /nope 2>&1 | cut -c1-5; cut 2>&1 | tr a-z A-Z; echo status=$?",
    ),
]
