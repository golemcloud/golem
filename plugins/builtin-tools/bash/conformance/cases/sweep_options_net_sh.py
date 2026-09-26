"""Option sweep, part F: curl and wget (no network on either side: usage and error paths only),
and the `sh` / `bash` commands' own options.

Transfers go to 127.0.0.1 port 1, which refuses on both sides, or to a name under `.invalid`,
which never resolves; their diagnostics carry timings, so those cases compare the status and
anything written to stdout or files. Error paths fold GNU's curly quotes into ASCII ones.
"""

TIER = "sweep"
CASES = []


def err(script):
    return "{ " + script + "; } 2>/tmp/err; echo \"status=$?\"; sed \"s/[\u2018\u2019]/'/g\" /tmp/err"


def add(cmd, label, script, error=False, quiet=False):
    tags = [f"cmd.{cmd}"]
    if error or quiet:
        tags += [f"cmd.{cmd}.error", "error"]
    if error:
        script = err(script)
    if quiet:
        script = "{ " + script + "; } >/tmp/out 2>/tmp/err; echo \"status=$?\"; wc -c </tmp/out"
    CASES.append((f"opt {cmd}: {label}", script, tags))


REFUSED = "http://127.0.0.1:1/"
NXHOST = "http://host.invalid/"

# --- curl ----------------------------------------------------------------------------------------
add("curl", "no url", "curl", error=True)
add("curl", "no url with options", "curl -s -o /tmp/x", error=True)
add("curl", "unknown option", "curl --bogus " + REFUSED, error=True)
add("curl", "unknown short option", "curl -Q " + REFUSED, error=True)
add("curl", "missing argument short", "curl -H", error=True)
add("curl", "missing argument long", "curl --max-time", error=True)
add("curl", "missing argument output", "curl -o", error=True)
add("curl", "unsupported protocol", "curl -sS gopher2://x/", error=True)
add("curl", "unsupported protocol silent", "curl -s foo://x/", quiet=True)
add("curl", "malformed port", "curl -sS http://127.0.0.1:99999/", error=True)
add("curl", "empty url", "curl -sS ''", error=True)
add("curl", "connection refused status", "curl -s " + REFUSED, quiet=True)
add("curl", "connection refused fail", "curl -sf " + REFUSED, quiet=True)
add("curl", "resolve failure status", "curl -s " + NXHOST, quiet=True)
add("curl", "output file not created on failure", "curl -s -o /tmp/o " + REFUSED + "; echo \"status=$?\"; ls /tmp")
add("curl", "remote name no path", "cd /tmp; curl -s -O http://127.0.0.1:1; echo \"status=$?\"")
add("curl", "remote name refused", "cd /tmp; curl -s -O " + REFUSED + "f.txt; echo \"status=$?\"; ls /tmp")
add("curl", "create dirs refused", "curl -s --create-dirs -o /tmp/a/b/c " + REFUSED + "; echo \"status=$?\"; ls /tmp")
add("curl", "invalid max time", "curl -m abc " + REFUSED, error=True)
add("curl", "negative max time", "curl -m -1 " + REFUSED, error=True)
add("curl", "invalid connect timeout", "curl --connect-timeout x " + REFUSED, error=True)
add("curl", "invalid retry", "curl --retry x " + REFUSED, error=True)
add("curl", "invalid retry delay", "curl --retry-delay x " + REFUSED, error=True)
add("curl", "retry refused", "curl -s --retry 1 --retry-delay 0 " + REFUSED, quiet=True)
add("curl", "data file missing", "curl -s -d @/tmp/nosuch " + REFUSED, error=True)
add("curl", "upload file missing", "curl -s -T /tmp/nosuch " + REFUSED, error=True)
add("curl", "form file missing", "curl -s -F 'f=@/tmp/nosuch' " + REFUSED, error=True)
add("curl", "form bad syntax", "curl -s -F novalue " + REFUSED, error=True)
add("curl", "json refused", "curl -s --json '{}' " + REFUSED, quiet=True)
add("curl", "method refused", "curl -s -X DELETE " + REFUSED, quiet=True)
add("curl", "head refused", "curl -s -I " + REFUSED, quiet=True)
add("curl", "include refused", "curl -s -i " + REFUSED, quiet=True)
add("curl", "location refused", "curl -sL " + REFUSED, quiet=True)
add("curl", "header refused", "curl -s -H 'X-A: b' " + REFUSED, quiet=True)
add("curl", "user agent refused", "curl -s -A agent " + REFUSED, quiet=True)
add("curl", "user refused", "curl -s -u a:b " + REFUSED, quiet=True)
add("curl", "referer refused", "curl -s -e http://x/ " + REFUSED, quiet=True)
add("curl", "cookie refused", "curl -s -b 'a=b' " + REFUSED, quiet=True)
add("curl", "cookie jar refused", "curl -s -c /tmp/jar " + REFUSED + "; echo \"status=$?\"; ls /tmp")
add("curl", "compressed refused", "curl -s --compressed " + REFUSED, quiet=True)
add("curl", "get with data refused", "curl -s -G -d a=b " + REFUSED, quiet=True)
add("curl", "write out on failure", "curl -s -w '[%{http_code}]' " + REFUSED + "; echo \" status=$?\"")
add("curl", "write out exitcode", "curl -s -w '%{exitcode}' " + REFUSED + "; echo \" status=$?\"")
add("curl", "write out unknown variable", "curl -s -w '%{bogus}' " + REFUSED, error=True)
add("curl", "verbose refused", "curl -sv " + REFUSED + " 2>/dev/null; echo \"status=$?\"")
add("curl", "help", "curl --help >/dev/null; echo \"status=$?\"")
add("curl", "version", "curl --version >/dev/null; echo \"status=$?\"")
# The resolver's reason, in parentheses after curl's own words, depends on the host's network (no
# DNS server in the oracle's container, NXDOMAIN elsewhere); only curl's own words are compared.
add(
    "curl",
    "silent show error",
    "curl -sS " + NXHOST + " 2>&1 >/dev/null | sed 's/ ([^()]*)$//' >&2; (exit ${PIPESTATUS[0]})",
    error=True,
)
add("curl", "fail with body refused", "curl -s --fail-with-body " + REFUSED, quiet=True)
add("curl", "several urls refused", "curl -s " + REFUSED + " " + REFUSED, quiet=True)
add("curl", "url option", "curl -s --url " + REFUSED, quiet=True)
add("curl", "output dir missing", "curl -s -o /tmp/nosuch/x " + REFUSED, quiet=True)
add("curl", "insecure refused", "curl -sk https://127.0.0.1:1/", quiet=True)
add("curl", "range refused", "curl -s -r 0-1 " + REFUSED, quiet=True)
add("curl", "data urlencode refused", "curl -s --data-urlencode 'a=b c' " + REFUSED, quiet=True)
add("curl", "config file missing", "curl -s -K /tmp/nosuch " + REFUSED, error=True)

# --- wget ----------------------------------------------------------------------------------------
add("wget", "no url", "wget", error=True)
add("wget", "unknown option", "wget --bogus " + REFUSED, error=True)
add("wget", "unknown short option", "wget -Z " + REFUSED, error=True)
add("wget", "missing argument", "wget -O", error=True)
add("wget", "connection refused quiet", "wget -q " + REFUSED, quiet=True)
add("wget", "resolve failure quiet", "wget -q " + NXHOST, quiet=True)
add("wget", "output document not left", "cd /tmp; wget -q -O out " + REFUSED + "; echo \"status=$?\"; ls")
add("wget", "output stdout refused", "wget -q -O - " + REFUSED, quiet=True)
add("wget", "prefix refused", "wget -q -P /tmp/dl " + REFUSED + "; echo \"status=$?\"; ls /tmp")
add("wget", "continue refused", "wget -q -c " + REFUSED, quiet=True)
add("wget", "timestamping refused", "wget -q -N " + REFUSED, quiet=True)
add("wget", "timeout refused", "wget -q -T 1 " + REFUSED, quiet=True)
add("wget", "tries refused", "wget -q -t 1 " + REFUSED, quiet=True)
add("wget", "invalid timeout", "wget -T x " + REFUSED, error=True)
add("wget", "invalid tries", "wget -t x " + REFUSED, quiet=True)
add("wget", "max redirect refused", "wget -q --max-redirect 0 " + REFUSED, quiet=True)
add("wget", "invalid max redirect", "wget --max-redirect=x " + REFUSED, quiet=True)
add("wget", "post data refused", "wget -q --post-data a=b " + REFUSED, quiet=True)
add("wget", "post file missing", "wget -q --post-file /tmp/nosuch " + REFUSED, quiet=True)
add("wget", "post data and file", "echo x >/tmp/f; wget -q --post-data a --post-file /tmp/f " + REFUSED, quiet=True)
add("wget", "header refused", "wget -q --header 'X-A: b' " + REFUSED, quiet=True)
add("wget", "user agent refused", "wget -q -U agent " + REFUSED, quiet=True)
add("wget", "server response refused", "wget -q -S " + REFUSED, quiet=True)
add("wget", "content disposition refused", "wget -q --content-disposition " + REFUSED, quiet=True)
add("wget", "unsupported scheme", "wget -q ftpx://x/", quiet=True)
add("wget", "unsupported scheme message", "wget ftpx://x/", error=True)
add("wget", "help", "wget --help >/dev/null; echo \"status=$?\"")
add("wget", "version", "wget --version >/dev/null; echo \"status=$?\"")
add("wget", "output directory missing", "wget -q -O /tmp/nosuch/x " + REFUSED, quiet=True)
add("wget", "several urls", "wget -q " + REFUSED + " " + NXHOST, quiet=True)
add("wget", "no clobber refused", "wget -q -nc " + REFUSED, quiet=True)
add("wget", "spider refused", "wget -q --spider " + REFUSED, quiet=True)
add("wget", "input file missing", "wget -q -i /tmp/nosuch", quiet=True)
add("wget", "empty url", "wget -q ''", quiet=True)

# --- sh and bash ---------------------------------------------------------------------------------
for sh in ["sh", "bash"]:
    add(sh, "c with name and args", sh + " -c 'echo \"$0|$1|$#\"' name a b")
    add(sh, "c default name", sh + " -c 'echo \"$0\"'")
    add(sh, "c exit status", sh + " -c 'exit 7'; echo \"status=$?\"")
    add(sh, "c missing argument", sh + " -c", error=True)
    add(sh, "errexit", sh + " -e -c 'false; echo no'; echo \"status=$?\"")
    add(sh, "nounset", sh + " -u -c 'echo $nope'", error=True)
    add(sh, "xtrace", sh + " -x -c 'echo hi'", error=True)
    add(sh, "verbose", sh + " -v -c 'echo hi'", error=True)
    add(sh, "noglob", "cd /tmp; touch a1; " + sh + " -f -c 'echo a*'")
    add(sh, "noexec", sh + " -n -c 'echo hi'; echo \"status=$?\"")
    add(sh, "noexec syntax error", sh + " -n -c 'if'", error=True)
    add(sh, "allexport", sh + " -a -c 'X=1; sh -c \"echo \\$X\"'")
    add(sh, "pipefail option", sh + " -o pipefail -c 'false | true'; echo \"status=$?\"")
    add(sh, "plus o option", sh + " +o errexit -c 'false; echo went on'")
    add(sh, "shopt option", sh + " -O extglob -c 'shopt -q extglob && echo on'")
    add(sh, "plus O option", sh + " +O extglob -c 'shopt -q extglob || echo off'")
    add(sh, "invalid option name", sh + " -o bogus -c true", error=True)
    add(sh, "invalid shopt name", sh + " -O bogus -c 'echo ran'", error=True)
    add(sh, "invalid short option", sh + " -Q -c true", error=True)
    add(sh, "invalid long option", sh + " --bogus -c true", error=True)
    add(sh, "combined flags", sh + " -ec 'false; echo no'; echo \"status=$?\"")
    add(sh, "double dash", sh + " -c -- 'echo ok'", error=True)
    add(sh, "stdin script", "echo 'echo from stdin; echo $#' | " + sh + " -s a b")
    add(sh, "stdin script default", "echo 'echo piped' | " + sh)
    add(sh, "norc", sh + " --norc -c 'echo ok'")
    add(sh, "noprofile", sh + " --noprofile -c 'echo ok'")
    add(sh, "noediting", sh + " --noediting -c 'echo ok'")
    add(sh, "posix mode", sh + " --posix -c 'shopt -qo posix && echo posix || echo not'")
    add(sh, "posix o", sh + " -o posix -c 'shopt -qo posix && echo posix'")
    add(sh, "login shell", sh + " -l -c 'shopt -q login_shell && echo login || echo not'")
    add(sh, "rcfile refused", sh + " --rcfile /tmp/rc -c 'echo ok'", error=True)
    add(sh, "restricted", sh + " -r -c 'cd /tmp'", error=True)
    add(sh, "restricted long", sh + " --restricted -c 'echo ok'", error=True)
    add(sh, "help status", sh + " --help >/dev/null; echo \"status=$?\"")
    add(sh, "version first line", sh + " --version | head -n 1 | cut -c1-9")
    add(sh, "isolated cd", "cd /tmp; " + sh + " -c 'cd /'; pwd")
    add(sh, "isolated variables", "X=1; " + sh + " -c 'X=2'; echo $X")
    add(sh, "no function inheritance", "f() { echo f; }; " + sh + " -c 'f'", error=True)
    add(sh, "exported function", "f() { echo f; }; export -f f; " + sh + " -c 'f'")
    add(sh, "stdin inherited", "echo data | " + sh + " -c 'cat'")
    add(sh, "exit code over 255", sh + " -c 'exit 300'; echo \"status=$?\"")
    add(sh, "syntax error in c", sh + " -c 'if then'", error=True)
    add(sh, "dollar dash", sh + " -e -c 'echo $-' | tr -d 'hBc'")
    add(sh, "positional count", sh + " -c 'echo $#; shift; echo $1' n a b c")
    add(sh, "set options after c", sh + " -c 'echo $1' -x arg; echo \"status=$?\"")
    add(sh, "nested", sh + " -c \"" + sh + " -c 'echo \\$0' inner\"")

# Divergences that are decisions, not bugs.
EXPECTED = {
    'opt sh: restricted': (
        0, b'status=2\nsh: a restricted shell (-r) is unsupported in bash-tool\n', b'',
        "fixture: bash-tool does not implement restricted shells (rbash); a bash or sh child given -r or --restricted is refused with bash-tool's canonical refusal (README, Commands), not run unrestricted",
    ),
    'opt sh: restricted long': (
        0, b'status=2\nsh: a restricted shell (--restricted) is unsupported in bash-tool\n', b'',
        "fixture: bash-tool does not implement restricted shells (rbash); a bash or sh child given -r or --restricted is refused with bash-tool's canonical refusal (README, Commands), not run unrestricted",
    ),
    'opt bash: restricted': (
        0, b'status=2\nbash: a restricted shell (-r) is unsupported in bash-tool\n', b'',
        "fixture: bash-tool does not implement restricted shells (rbash); a bash or sh child given -r or --restricted is refused with bash-tool's canonical refusal (README, Commands), not run unrestricted",
    ),
    'opt bash: restricted long': (
        0, b'status=2\nbash: a restricted shell (--restricted) is unsupported in bash-tool\n', b'',
        "fixture: bash-tool does not implement restricted shells (rbash); a bash or sh child given -r or --restricted is refused with bash-tool's canonical refusal (README, Commands), not run unrestricted",
    ),
}
