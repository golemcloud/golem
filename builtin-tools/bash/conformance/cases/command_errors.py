"""Error paths for commands that had none: exit status and diagnostic for bad usage.

Network failures are not here: Wasmtime cannot make HTTP requests locally, so they are covered by
the Golem integration tests.
"""

ERROR_PATHS = True

CASES = [
    ("join: unsorted input and missing file", "printf 'b 1\\na 2\\n' >/tmp/l; printf 'a x\\n' >/tmp/r; join /tmp/l /tmp/r; echo status=$?; join /tmp/nosuch /tmp/r; echo status=$?"),
    ("ln: missing target and existing link", "ln /tmp/nosuch /tmp/l; echo status=$?; : >/tmp/t; ln -s /tmp/t /tmp/s; ln -s /tmp/t /tmp/s; echo status=$?"),
    ("mkdir: existing and missing parent", "mkdir /tmp/d; mkdir /tmp/d; echo status=$?; mkdir /tmp/a/b/c; echo status=$?; mkdir -p /tmp/a/b/c; echo status=$?"),
    ("printf: missing and extra arguments", "printf '%s %s\\n' only; printf '%d\\n' x; echo status=$?; printf; echo status=$?"),
    ("sh: missing script operand", "sh -c; echo status=$?"),
    ("sh: unreadable script file", "sh /tmp/nosuch.sh; echo status=$?"),
    ("sleep: invalid intervals", "sleep x; echo status=$?; sleep -1; echo status=$?; sleep; echo status=$?"),
    ("split: missing input", "split /tmp/nosuch /tmp/out; echo status=$?"),
    ("yes: write error to a closed pipe", "yes | head -n 2; echo status=${PIPESTATUS[0]}"),
    ("curl: unsupported protocol", "curl -sS gopher2://example/ ; echo status=$?"),
    ("curl: malformed URL", "curl -sS 'http://[bad' ; echo status=$?"),
    ("curl: missing option argument", "curl -o; echo status=$?"),
    ("wget: missing URL", "wget; echo status=$?"),
    ("curl: invalid time values", "for v in -1 -0 +3 ' 3' .5 5. nan inf abc ''; do curl -s -m \"$v\" http://127.0.0.1:1/; echo \"m=[$v] status=$?\"; done; curl -s --max-time -1 http://127.0.0.1:1/; echo status=$?; curl -s --connect-timeout nan http://127.0.0.1:1/; echo status=$?; curl -s --retry-delay -1 http://127.0.0.1:1/; echo status=$?"),
    ("curl: time values too large", "curl -s -m 9223372036854775 http://127.0.0.1:1/; echo status=$?; curl -s -m 99999999999999999999 http://127.0.0.1:1/; echo status=$?; curl -s -m 1.9999999999999999999 http://127.0.0.1:1/; echo status=$?"),
    ("wget: invalid time periods", "for v in -1 -1m 1e3 nan inf 1e300 abc 3x ''; do wget -q -T \"$v\" http://127.0.0.1:1/; echo \"T=[$v] status=$?\"; done; wget -q --timeout=-2 http://127.0.0.1:1/; echo status=$?"),
]

SCRIPT_FILES = 'fixture (script-files): running a script file by path is refused; source it or use sh -c'
EXPECTED = {
    'sh: unreadable script file': (
        0, b'status=2\n', b'sh: /tmp/nosuch.sh: running a script file is unsupported in bash-tool; use `source /tmp/nosuch.sh` or `sh -c`\n', SCRIPT_FILES,
    ),
}

SYMLINKS = 'fixture (symlinks): links to absolute paths are refused, since WASI cannot create them'
EXPECTED |= {
    'ln: missing target and existing link': (
        0, b'status=1\nstatus=1\n', b"ln: failed to access '/tmp/nosuch': No such file or directory\nln: failed to create symbolic link to '/tmp/t': links to absolute paths are unsupported in bash-tool; use a relative target\nln: failed to create symbolic link to '/tmp/t': links to absolute paths are unsupported in bash-tool; use a relative target\n", SYMLINKS,
    ),
}

CURL_GLOB = 'curl reads [ in a URL as a glob range and points at it; bash-tool reports the malformed URL, with the same status'
EXPECTED |= {
    'curl: malformed URL': (
        0, b'status=3\n', b'curl: (3) URL rejected: Malformed input to a URL function\n', CURL_GLOB,
    ),
}
