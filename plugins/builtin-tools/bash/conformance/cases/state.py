"""Separate calls: every `run` is a fresh shell, and only the working directory crosses calls.

Each script is split at `#--call--` lines into separate calls. Our shell's example starts each call
in the directory the previous one ended in, as a caller passing back the returned `cwd` does; the
oracle runs each call as its own `bash -c` in the directory the marker names (`/` when it names
none). Anything else a script wants to keep, it writes to a file.
"""

M = "\n#--call--\n"


def IN(directory):
    """A call boundary after which the next call starts in `directory`."""
    return f"\n#--call-- {directory}\n"


TAGS = ["state.cross-call"]

CASES = [
    ("calls: the working directory carries", "mkdir -p /tmp/w && cd /tmp/w" + IN("/tmp/w") + "pwd; cd ..; pwd" + IN("/tmp") + "pwd"),
    ("calls: relative paths resolve from the carried directory", "mkdir -p /tmp/r/sub; cd /tmp/r; echo data > sub/f" + IN("/tmp/r") + "cat sub/f; cd sub" + IN("/tmp/r/sub") + 'cat f; echo "$PWD"'),
    ("calls: exit keeps the directory it left in", "cd /tmp && exit 3" + IN("/tmp") + "pwd"),
    ("calls: variables and exports end with their call", "x=1; export y=2; declare -i n=5" + M + 'echo "[${x-unset}] [${y-unset}] [${n-unset}]"'),
    ("calls: arrays end with their call", "a=(1 2 3); declare -A m=([k]=v)" + M + 'echo "${#a[@]} ${#m[@]}"'),
    ("calls: functions end with their call", 'greet() { echo "hi $1"; }' + M + "declare -F; echo done"),
    ("calls: aliases end with their call", "shopt -s expand_aliases; alias ll='echo listing'" + M + "alias; echo done"),
    ("calls: shell options end with their call", "set -eu; set -o pipefail; shopt -s nullglob" + M + "set -o | grep -E '^(errexit|nounset|pipefail)'; shopt nullglob"),
    ("calls: errexit does not carry", "set -e" + M + "false; echo reached"),
    ("calls: the last status does not carry", "(exit 7)" + M + "echo $?"),
    ("calls: traps end with their call", "trap 'echo err' ERR; trap '' PIPE; trap 'echo bye' EXIT" + M + "trap -p; false; echo after"),
    ("calls: readonly ends with its call", "readonly r=1" + M + "r=2; echo $r"),
    ("calls: positional parameters and the directory stack end with their call", "set -- a b c; cd /tmp; mkdir -p s; pushd s >/dev/null" + IN("/tmp/s") + "echo $#; dirs"),
    ("calls: each call is a new process", "echo $$ > /tmp/pid" + M + 'test "$$" != "$(cat /tmp/pid)" && echo different'),
    ("calls: settings kept in a file", "printf '%s\\n' 'x=kept' 'greet() { echo \"hi $1\"; }' 'set -o pipefail' > /tmp/env.sh" + M + ". /tmp/env.sh; echo $x; greet you; set -o | grep pipefail"),
]
