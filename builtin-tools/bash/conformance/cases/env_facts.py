"""What a script can learn about its environment in a Golem agent.

Both sides run as a user with no passwd entry and no HOME, in `/`, with Golem's GOLEM_* variables
and a fresh `/tmp`; `no_tmp` cases have no `/tmp` at all, which Golem does not guarantee. The oracle
also has a PATH (GNU bash must find its tools) and LC_ALL; our shell has neither, so cases that
print those are fixtures.
"""

CASES = [
    ("env: HOME is unset", 'echo "[${HOME-unset}]"; [ -z "${HOME+x}" ] && echo unset', ["env.home"]),
    ("env: cd with no operand and no HOME", "cd; echo status=$?; pwd", ["env.home", "builtin.cd"]),
    ("env: tilde forms with no HOME", "echo ~ ~/a ~+ ~-; cd /tmp; echo ~+ ~-", ["env.home"]),
    ("env: HOME exported reaches children", "export HOME=/tmp; bash -c 'echo ~ $HOME'", ["env.home", "param.export"]),
    ("env: UID and EUID", 'echo "uid=[${UID-unset}] euid=[${EUID-unset}]"', ["env.identity"]),
    ("env: USER LOGNAME SHELL", 'echo "[${USER-u}] [${LOGNAME-l}] [${SHELL-s}]"', ["env.identity"]),
    ("env: HOSTNAME and OSTYPE are set", '[ -n "${OSTYPE+x}" ] && echo ostype; [ -n "${HOSTNAME+x}" ] && echo hostname; true', ["env.identity"]),
    ("env: PPID is a number", '[[ $PPID =~ ^[0-9]+$ ]] && echo numeric', ["env.identity"]),
    ("env: GOLEM variables", 'echo "$GOLEM_AGENT_ID $GOLEM_COMPONENT_REVISION"; env | grep -c "^GOLEM_"', ["env.golem"]),
    ("env: GOLEM variables are exported", "bash -c 'echo ${GOLEM_AGENT_ID-missing}'", ["env.golem", "param.export"]),
    ("env: working directory", "pwd; echo $PWD; cd /tmp && pwd && cd - >/dev/null && pwd", ["builtin.pwd", "builtin.cd"]),
    ("env: root is writable", "mkdir /work && echo data >/work/f && cat /work/f && ls /work", ["env.filesystem"]),
    ("env: dev null reads empty", "cat /dev/null | wc -c; wc -c </dev/null", ["redirection.dev-paths"]),
    ("env: dev null swallows output", "echo hidden >/dev/null; echo shown 2>/dev/null; echo status=$?", ["redirection.dev-paths"]),
    ("env: test operators on dev null", "[ -e /dev/null ] && echo exists; [ -f /dev/null ] || echo not-regular; [ -c /dev/null ] && echo char", ["redirection.dev-paths", "builtin.["]),
    ("env: tmp is empty and writable", "ls -A /tmp | wc -l; touch /tmp/x && ls /tmp", ["env.tmp"]),
    ("env: mktemp in tmp", 'f=$(mktemp); case "$f" in /tmp/tmp.*) echo in-tmp;; esac; [ -f "$f" ] && echo file', ["env.tmp", "cmd.mktemp"]),
    ("env: mktemp directory", 'd=$(mktemp -d); [ -d "$d" ] && echo dir; case "$d" in /tmp/*) echo in-tmp;; esac', ["env.tmp", "cmd.mktemp"]),
    ("env: no tmp directory", "[ -d /tmp ] && echo present || echo absent", ["env.tmp"]),
    ("env: mktemp without tmp", "mktemp; echo status=$?", ["env.tmp", "cmd.mktemp", "error"]),
    ("env: dev null without tmp", "echo x >/dev/null; echo status=$?; [ -d /tmp ] && echo tmp-created || echo no-tmp", ["env.tmp", "redirection.dev-paths"]),
    ("env: TMPDIR unset", 'echo "[${TMPDIR-unset}]"', ["env.tmp"]),
    ("env: LANG and LC_ALL", 'echo "[${LANG-unset}]"', ["env.identity"]),
    ("env: command lookup of a builtin command", "command -v cat >/dev/null && echo found; type -t cat", ["env.path", "builtin.command", "builtin.type"]),
    ("env: command lookup of a missing command", "command -v nosuchcmd; echo status=$?; nosuchcmd 2>/dev/null; echo status=$?", ["env.path", "builtin.command"]),
    ("env: hash of a missing command", "hash nosuchcmd 2>/dev/null; echo status=$?", ["env.path", "builtin.hash"]),
    ("env: SHLVL and BASH are set", '[ -n "${SHLVL+x}" ] && echo shlvl; [ -n "${BASH+x}" ] && echo bash', ["env.identity"]),
    ("env: umask default is printable", "umask >/dev/null; echo status=$?", ["env.identity"]),
    ("env: id -u", "id -u >/dev/null 2>&1; echo status=$?", ["env.identity"]),
    ("env: whoami", "whoami >/dev/null 2>&1; echo status=$?", ["env.identity"]),
]

OPTIONS = {
    "env: no tmp directory": {"no_tmp": True},
    "env: mktemp without tmp": {"no_tmp": True},
    "env: dev null without tmp": {"no_tmp": True},
}

MISSING = 'ignored (missing-commands): bash-tool has no such command'
EXPECTED = {
    'env: id -u': (
        0, b'status=127\n', b'', MISSING,
    ),
    'env: whoami': (
        0, b'status=127\n', b'', MISSING,
    ),
}

REFUSED_GAP = 'refused (runtime-gaps): bash-tool refuses a feature it does not implement, up front when the script names it'
EXPECTED |= {
    'env: umask default is printable': (
        2, b'', b'bash: umask is unsupported in bash-tool\n', REFUSED_GAP,
    ),
}

# `env: mktemp without tmp` no longer needs a UTF8_QUOTES override: the shell now defaults
# to a UTF-8 locale (see `session::Session::set_identity`) and mktemp's own message was fixed
# to match GNU's curly quotes under it, so the plain byte-exact check above now passes.
