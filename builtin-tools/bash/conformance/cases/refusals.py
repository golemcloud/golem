"""Refusals and the builtins at their edge: `exec`, `source`, `bind`, `history`, `enable`, `ulimit`
and `fc`.

Each case name starts with `refusals: `.
"""

CASES = [
    # `exec`'s own -a/-c/-l refusal must not fire on a target command's own
    # flags that happen to contain the letters a, c or l (`exec ls -l`, `exec sort -c`).
    ("refusals: exec ls -l is not exec -l", "exec ls -l /tmp/does-not-exist-xyz 2>/dev/null; echo status=$?"),
    ("refusals: exec wc -l is not exec -l", "printf 'a\\nb\\nc\\n' >/tmp/wcl-xyz; exec wc -l /tmp/wcl-xyz"),
    ("refusals: exec sort -c is not exec -c", "printf 'a\\nb\\nc\\n' >/tmp/sc-xyz; exec sort -c /tmp/sc-xyz; echo after"),
    ("refusals: exec -- passes through to the command", "exec -- true; echo after"),
    # exec's own -a/-c/-l options are still refused, wherever the script names them literally.
    ("refusals: exec -l is still refused", "exec -l true; echo after"),
    ("refusals: exec -a name is still refused", "exec -a myname true; echo after"),
    # a failed `exec` ends the script, as replacing the shell would; it does
    # not fall through to the next command.
    ("refusals: a failed exec ends the script", "exec nosuchcommand123; echo continued"),
    ("refusals: a successful exec still ends the script", "exec true; echo continued"),
    # `exec` inside a subshell ends just that subshell (its own call frame, not
    # a real OS process here either), the same way it ends the top-level script -- whether it
    # succeeds or fails to find its target.
    ("refusals: a failed exec ends the subshell it's in", "(exec nosuchcommand123; echo notreached); echo after"),
    ("refusals: a successful exec ends the subshell it's in", "(exec echo hi; echo notreached); echo after"),
    # Brush's "not yet implemented" placeholders (status 99) now surface as canonical,
    # documented refusals instead. (`logout` is not one: outside a login shell it fails as bash's
    # does.)
    ("refusals: read -e reads plainly from input that is no terminal", "read -e x <<< hi; echo \"$x\" $?"),
    ("refusals: mapfile -C runs its callback", "printf 'a\\nb\\n' | mapfile -C echo -c 1 arr; echo done=$?"),
    # `jobs -n` and job specs are implemented, not refused; jobs.py covers them.
    ("refusals: extended test -N is refused", "[[ -N /tmp ]]; echo s=$?; echo after"),
    ("refusals: cd -@ is refused", "cd -@; echo s=$?"),
    ("refusals: enable -f is refused", "enable -f /tmp/x.so foo; echo s=$?"),
    ("refusals: enable -d of a name that is no builtin", "enable -d foo; echo s=$?"),
    ("refusals: help -m is refused", "help -m cd; echo s=$?"),
    # The builtins above still work normally outside the refused options.
    ("refusals: read still works", "read x <<< hi; echo \"$x\" $?"),
    ("refusals: jobs with no args still works", "jobs; echo s=$?"),
    # `jobs -l` has its own wasm32 implementation (unlike -n and a job spec): it must stay
    # unrefused, the regression an earlier draft of this fix introduced.
    ("refusals: jobs -l still works", "sleep 0.1 & jobs -l >/dev/null; echo s=$?; wait"),
    ("refusals: cd still works", "cd /tmp && pwd"),
    # `/dev/fd` is a symlink to `/proc/self/fd`; the target itself must answer as a
    # directory too, or `cd /proc/self/fd` fails while `cd /dev/fd` (an equivalent path) succeeds.
    ("refusals: cd /proc/self/fd works like cd /dev/fd", "cd /dev/fd && pwd; cd /proc/self/fd && pwd"),
    # `source <(cmd)` (a buffered process substitution) now works instead of being
    # refused; `/dev/stdin` (a live stream) is still refused, since reading it synchronously here
    # would block forever.
    ("refusals: source of process substitution works", "source <(printf 'x=1\\n'); echo $x"),
    ("refusals: source of /dev/stdin is refused", "printf 'y=2\\n' | { source /dev/stdin; echo $y; }"),
    # `source`/`.` follows bash's `sourcepath`: a name with no `/` is searched for in
    # $PATH when not found relative to the working directory.
    ("refusals: source searches PATH", "mkdir -p /tmp/fixes-sourcepath; echo 'echo hi from sourced' >/tmp/fixes-sourcepath/myfile.sh; PATH=\"/tmp/fixes-sourcepath:$PATH\" source myfile.sh"),
    # a sourced file's own syntax error names the file, not `bash`, matching bash exactly
    # (bash-tool's all-or-nothing preflight means nothing before the error ran either way, which
    # is documented and intentional, not part of what this case checks).
    ("refusals: sourced syntax error names the file", "printf 'echo ok\\nif then\\n' >/tmp/fixes-badsrc.sh; source /tmp/fixes-badsrc.sh; echo after=$?"),
    # a runtime error inside a sourced file also names the file, not `bash` ($0 itself is
    # unaffected by `source`, but bash's own diagnostics from inside one are).
    ("refusals: sourced runtime error names the file", "printf 'set -u\\necho $UNSET_VAR\\n' >/tmp/fixes-badsrc2.sh; source /tmp/fixes-badsrc2.sh; echo after=$?"),
    # `bind`'s binding forms (nothing to query) warn like bash's own
    # non-interactive `bind` and succeed; only its query forms, which would need readline's
    # default tables, are refused.
    ("refusals: bind binding form warns and succeeds", "bind '\"\\C-x\\C-x\": \"hi\"'; echo s=$?"),
    ("refusals: bind -x warns and succeeds", "bind -x '\"\\C-t\": true'; echo s=$?"),
    ("refusals: bind -q is refused", "bind -q abort >/dev/null; echo s=$?"),
    ("refusals: bind -l is refused", "bind -l >/dev/null; echo s=$?"),
    # `history` is faithful now: -s/-c/-d/-p/-a/-n/-r/-w and N all work against a real in-memory
    # list, but nothing auto-records into it, matching bash's own non-interactive behavior.
    ("refusals: history -s and display", "history -s foo; history"),
    ("refusals: history -p expands !!", "history -s foo; history -p '!!'"),
    ("refusals: history -c clears", "history -s foo; history -c; history; echo s=$?"),
    ("refusals: history -d deletes an offset", "history -s foo; history -s bar; history -d 1; history"),
    ("refusals: history -w then -r round-trips a file", "history -s foo; history -w /tmp/fixes-histfile-wr; history -c; history -r /tmp/fixes-histfile-wr; history"),
    ("refusals: history -a appends to a file", "history -s foo; history -a /tmp/fixes-histfile-a; cat /tmp/fixes-histfile-a"),
    ("refusals: history -n reads a file", "printf 'foo\\nbar\\n' >/tmp/fixes-histfile-n; history -n /tmp/fixes-histfile-n; history"),
    ("refusals: history rejects a non-numeric N", "history 3x; echo s=$?"),
    ("refusals: history -p reports a bad event", "history -p '!nonexistentxyz'; echo s=$?"),
    # an unreadable -d @file is curl's own exit 26 (CURLE_READ_ERROR), not a usage error
    # (exit 2) -- the command line was fine, reading the file failed. This one is deterministic
    # against the oracle without a live request: the file read fails before curl ever connects.
    ("refusals: unreadable -d @file exits 26", "curl -d @/tmp/fixes-no-such-file-xyz http://127.0.0.1:1/; echo status=$?"),
]

# `-a`/`-l` genuinely have no wasm32 equivalent (no argv[0] override, no login-shell emulation),
# so bash-tool refuses them, as README.md documents. The oracle's own `true`/`ls` binaries are a
# busybox-style multiplexer that inspects argv[0]; asked to run under a fake one they print their
# own "unknown program" error instead of bash ever seeing an -a/-l refusal, so the goldens for
# real Bash are not useful fixtures for what bash-tool does here.
STILL_REFUSED = "fixture (exec -a/-l): -a and -l are documented refusals; the oracle's coreutils multiplexer, not bash, produces the comparison output for a real run"
EXPECTED = {
    'refusals: exec -l is still refused': (
        2, b'', b'bash: exec -a, -c and -l are unsupported in bash-tool\n', STILL_REFUSED,
    ),
    'refusals: exec -a name is still refused': (
        2, b'', b'bash: exec -a, -c and -l are unsupported in bash-tool\n', STILL_REFUSED,
    ),
}

# real Bash implements all of these; bash-tool doesn't (no readline, no dynamic loading,
# no job control beyond a single foreground script, no xattr view, no man pages). Before this
# fix each one leaked Brush's own "not yet implemented" text and status 99; now each is a
# canonical, documented `bash-tool` refusal, exit status 2.
NOT_YET_IMPLEMENTED = "fixture: a canonical refusal for a Brush placeholder Bash itself implements; not a WASI limit, but implementing readline/dlopen/job-control/man-pages is out of scope"
EXPECTED |= {
    'refusals: extended test -N is refused': (
        2, b'', b'bash: [[ -N ]] is unsupported in bash-tool\n', NOT_YET_IMPLEMENTED,
    ),
    'refusals: cd -@ is refused': (
        2, b'', b'bash: cd -@ is unsupported in bash-tool\n', NOT_YET_IMPLEMENTED,
    ),
    'refusals: enable -f is refused': (
        2, b'', b'bash: enable -f is unsupported in bash-tool\n', NOT_YET_IMPLEMENTED,
    ),
    'refusals: help -m is refused': (
        2, b'', b'bash: help -m is unsupported in bash-tool\n', NOT_YET_IMPLEMENTED,
    ),
}

LIVE_STREAM = "fixture: sourcing a live stream would block forever in this single-threaded cooperative model before the pipeline feeding it could run; a documented refusal, not a WASI limit"
EXPECTED |= {
    'refusals: source of /dev/stdin is refused': (
        0, b'\n', b'bash: source of /dev/stdin, /dev/stdout or /dev/stderr is unsupported in bash-tool\n', LIVE_STREAM,
    ),
}

ALL_OR_NOTHING = "fixture: bash-tool's documented all-or-nothing preflight (README.md) means nothing before a later syntax error runs either, unlike bash itself, which executes each command as it is read"
EXPECTED |= {
    'refusals: sourced syntax error names the file': (
        0, b'after=2\n',
        b"/tmp/fixes-badsrc.sh: line 2: syntax error near unexpected token `then'\n/tmp/fixes-badsrc.sh: line 2: `if then'\n",
        ALL_OR_NOTHING,
    ),
}

INTERACTIVE_ONLY = "fixture: bash's own readline tables are compiled in and answer these queries even under `bash -c` (confirmed against the oracle: `bind -q abort` succeeds, printing bash's own warning and then its real answer) -- Brush has no equivalent table built, not a WASI/terminal limit; reproducing bash's actual emacs/vi defaults just for these query forms is disproportionate"
EXPECTED |= {
    'refusals: bind -q is refused': (
        2, b'',
        b"bash: bind's query options (-l, -p, -P, -s, -S, -v, -V, -q, -u, -X) are unsupported in bash-tool\n",
        INTERACTIVE_ONLY,
    ),
    'refusals: bind -l is refused': (
        2, b'',
        b"bash: bind's query options (-l, -p, -P, -s, -S, -v, -V, -q, -u, -X) are unsupported in bash-tool\n",
        INTERACTIVE_ONLY,
    ),
}


# Each refusal by name, both as a literal command (the preflight path) and reached through a
# variable (the run-time path, which is a DIFFERENT code path -- see stateless.rs's `execute()`),
# so that removing a refusal, or making one reached at run time silent or report status 0, fails a
# case. `ulimit` matters most: removing its refusal doesn't just misbehave, it panics the whole component (`ORIGINALS["ulimit"]`: no entry found), since `ulimit` has no
# real Brush builtin to fall back to.
CASES += [
    # `ulimit -n` alone reads the ambient soft limit, which is a host/container default (varies
    # between environments -- 1048576 locally, 20480 on CI observed) and so isn't a stable oracle
    # answer for `stale` to check against; setting it first makes the read-back deterministic.
    # Refused either way for our own tool: the value read back is bash's own, not ours.
    ("refusals: ulimit is refused", "ulimit -S -n 64; ulimit -n; echo s=$?"),
    ("refusals: ulimit is refused at run time too", "cmd=ulimit; $cmd -S -n 64; $cmd -n; echo s=$?"),
    ("refusals: fc is refused", "fc -l; echo s=$?"),
    ("refusals: exec -a is refused at run time too", "cmd=exec; $cmd -a foo true; echo s=$?"),
]

DELIBERATE_LIMIT = "fixture: a documented refusal (README.md's \"Commands\" section) this platform genuinely cannot support -- WASI has no resource limits (ulimit), no history list to edit (fc), and no argv[0] override (exec -a)"
EXPECTED |= {
    'refusals: ulimit is refused': (
        2, b'', b'bash: ulimit is unsupported in bash-tool\n', DELIBERATE_LIMIT,
    ),
    'refusals: ulimit is refused at run time too': (
        0, b's=2\n', b'bash: ulimit is unsupported in bash-tool\nbash: ulimit is unsupported in bash-tool\n', DELIBERATE_LIMIT,
    ),
    'refusals: fc is refused': (
        2, b'', b'bash: background/history execution (fc) is unsupported in bash-tool\n', DELIBERATE_LIMIT,
    ),
    'refusals: exec -a is refused at run time too': (
        0, b's=2\n', b'bash: exec -a, -c and -l are unsupported in bash-tool\n', DELIBERATE_LIMIT,
    ),
}
