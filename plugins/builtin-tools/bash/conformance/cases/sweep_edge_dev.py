"""Edge inputs: the special /dev paths (/dev/null, /dev/stdin, /dev/stdout, /dev/stderr, /dev/fd/N)
as operands and as redirection targets, in the forms special_paths.py does not already cover:
/dev/fd/N, stdin that is a file, a here-document or a partly read file, closed descriptors,
process substitution paths, writers, and the test operators.

Generated from fixed tables with plain loops, so every Python 3 produces the same scripts.
"""

TIER = "sweep"
TAGS = ["redirection.dev-paths"]

DATA = "printf 'b\\na\\nb\\n'"
FILE = "printf 'b\\na\\nb\\n' > /tmp/in; "

# How a /dev path reaches a reader. {c} is the command, {p} the path it gets.
MODES = [
    ("fd0 from pipe", DATA + " | {c} /dev/fd/0"),
    ("stdin from file", FILE + "{c} /dev/stdin < /tmp/in"),
    ("fd0 from file", FILE + "{c} /dev/fd/0 < /tmp/in"),
    ("stdin from heredoc", "{c} /dev/stdin <<'EOF'\nb\na\nb\nEOF"),
    ("stdin from herestring", "{c} /dev/stdin <<< $'b\\na\\nb'"),
    ("fd3 from exec", FILE + "exec 3< /tmp/in; {c} /dev/fd/3"),
    ("fd3 on the command", FILE + "{c} /dev/fd/3 3< /tmp/in"),
    ("fd that is not open", "{c} /dev/fd/7"),
    ("stdin closed", "{c} /dev/stdin <&-"),
    ("stdin after partial read of a file", FILE + "{ read -r first; {c} /dev/stdin; } < /tmp/in"),
    ("dash after partial read of a file", FILE + "{ read -r first; {c} -; } < /tmp/in"),
    ("stdin after partial read of a pipe", DATA + " | { read -r first; {c} /dev/stdin; }"),
    ("stdin twice", DATA + " | {c} /dev/stdin /dev/stdin"),
    ("process substitution", "{c} <(" + DATA + ")"),
]

READERS = [
    ("cat", "cat"), ("head", "head -n 1"), ("tail", "tail -n 1"), ("wc -l", "wc -l"),
    ("sort", "sort"), ("uniq -c", "uniq -c"), ("grep", "grep b"), ("sed", "sed -n 1p"),
    ("cut", "cut -c1"), ("nl", "nl"), ("od", "od -An -c"), ("rev", "rev"), ("tac", "tac"),
    ("base64", "base64"), ("md5sum", "md5sum"), ("jq -R", "jq -R ."), ("fold", "fold -w 1"),
    ("paste", "paste -s -d ,"),
]

CASES = []
for mode, template in MODES:
    for label, command in READERS:
        if label == "uniq -c" and mode == "stdin twice":
            continue  # uniq's second operand is its output: Bash writes into its own stdin pipe and hangs
        script = template.replace("{c}", command) + "; echo \"status=$?\""
        CASES.append((f"edge dev: {label} {mode}", script))

# Writers into /dev paths, and redirections to them from each kind of stdout.
WRITES = [
    ("echo to stdout", "echo x > /dev/stdout"),
    ("echo to fd1", "echo x > /dev/fd/1"),
    ("echo to stderr", "echo x > /dev/stderr"),
    ("echo to fd2", "echo x > /dev/fd/2"),
    ("append to stdout", "echo x >> /dev/stdout"),
    ("both to null", "echo x &> /dev/null; echo y >& /dev/null; echo status=$?"),
    ("append both to null", "echo x &>> /dev/null; echo status=$?"),
    ("stderr to stdout path", "ls /tmp/nosuch 2> /dev/stdout | sed 's/^/piped: /'"),
    ("stdout to stderr path", "echo moved > /dev/stderr 2> /dev/null; echo after"),
    ("fd3 dup to stdout path", "exec 3> /dev/stdout; echo three >&3; exec 3>&-"),
    ("fd3 via fd path", "echo three > /dev/fd/3 3>&1"),
    ("fd path not open", "echo x > /dev/fd/5; echo status=$?"),
    ("bad fd duplicate", "echo x >&5; echo status=$?"),
    ("read fd path not open", "cat < /dev/fd/6; echo status=$?"),
    ("stdout reopened on a file", "{ echo a; echo b > /dev/stdout; } > /tmp/o; cat /tmp/o"),
    ("stdout appended on a file", "{ echo a; echo b >> /dev/stdout; } > /tmp/o; cat /tmp/o"),
    ("fd1 reopened on a file", "{ echo a; echo b > /dev/fd/1; } > /tmp/o; cat /tmp/o"),
    ("stderr reopened on a file", "{ echo a >&2; echo b > /dev/stderr; } 2> /tmp/o; cat /tmp/o"),
    ("stdout reopened on append file", "echo 0 > /tmp/o; { echo a; echo b > /dev/stdout; } >> /tmp/o; cat /tmp/o"),
    ("exec stdout to null and back", "exec 3>&1 > /dev/null; echo hidden; exec >&3 3>&-; echo shown"),
    ("exec stderr to null", "exec 2> /dev/null; ls /tmp/nosuch; echo status=$?"),
    ("exec fd to null then write", "exec 4> /dev/null; echo lost >&4; echo status=$?"),
    ("exec fd from null then read", "exec 4< /dev/null; read -r -u 4 x; echo status=$? [$x]"),
    ("exec stdin from null", "exec < /dev/null; read -r x; echo status=$?; cat; echo end"),
    ("exec fd from stdin path", "exec 5< /dev/stdin; read -r -u 5 x; echo \"[$x]\"", ),
    ("tee stdout twice", "echo x | tee /dev/stdout /dev/stdout"),
    ("tee fd2", "echo x | tee /dev/fd/2"),
    ("tee fd3", "echo x | tee /dev/fd/3 3> /tmp/o > /dev/null; cat /tmp/o"),
    ("tee -a stderr", "echo x | tee -a /dev/stderr"),
    ("tee null", "echo x | tee /dev/null"),
    ("sed w stderr", "echo x | sed 'w /dev/stderr'"),
    ("sed w null", "echo x | sed -n 'w /dev/null'; echo status=$?"),
    ("sed r stdin", "echo inserted > /tmp/i; echo x > /tmp/x; sed 'r /dev/stdin' /tmp/x < /tmp/i"),
    ("sed -i null", "sed -i 's/a/b/' /dev/null; echo status=$?"),
    ("sort -o null", "printf 'b\\na\\n' | sort -o /dev/null; echo status=$?"),
    ("sort -o stderr", "printf 'b\\na\\n' | sort -o /dev/stderr"),
    ("cp file to null", "echo x > /tmp/f; cp /tmp/f /dev/null; echo status=$?"),
    ("cp stdin to file", "cp /dev/stdin /tmp/c <<< data; cat /tmp/c"),
    ("cp fd0 to file", "echo piped | cp /dev/fd/0 /tmp/c; cat /tmp/c"),
    ("cp null to file", "cp /dev/null /tmp/c; wc -c < /tmp/c"),
    ("cat file to stderr path", "echo x > /tmp/f; cat /tmp/f > /dev/stderr"),
    ("jq to stderr", "jq -n 1 > /dev/stderr"),
    ("printf to fd path", "printf 'p\\n' > /dev/fd/1"),
    ("split to stdout path", "printf 'a\\nb\\n' | split -l 1 --filter='cat' - /tmp/s; echo status=$?"),
    ("diff output to null", "echo a > /tmp/a; echo b > /tmp/b; diff /tmp/a /tmp/b > /dev/null; echo status=$?"),
    ("function output to null", "f() { echo in-f; echo err-f >&2; }; f > /dev/null 2>&1; echo status=$?"),
    ("subshell output to stderr path", "( echo sub ) > /dev/stderr"),
    ("command substitution with stdout path", "x=$(echo inner > /dev/stdout); echo \"[$x]\""),
    ("command substitution with stderr path", "x=$(echo inner > /dev/stderr); echo \"[$x]\""),
    ("pipeline stage to stderr path", "echo a | tee /dev/stderr | tr a b"),
    ("loop output to null", "for i in 1 2; do echo $i; done > /dev/null; echo status=$?"),
    ("heredoc to stdout path", "cat > /dev/stdout <<EOF\nhd\nEOF"),
    ("closed stdout write", "echo x >&-; echo status=$? >&2"),
    ("closed stderr write", "ls /tmp/nosuch 2>&-; echo status=$?"),
    ("closed stdout command", "cat /dev/null >&-; echo status=$?"),
    ("closed stdout printf", "printf 'x\\n' >&-; echo status=$?"),
]
for label, script, *rest in WRITES:
    CASES.append((f"edge dev write: {label}", script))

# Reading the shell's own inputs through /dev paths.
SHELL_READS = [
    ("source null", "source /dev/null; echo status=$?"),
    ("source stdin", ". /dev/stdin <<< 'echo sourced'"),
    ("source fd3", "source /dev/fd/3 3<<< 'echo three'"),
    ("source process substitution", ". <(echo 'echo ps; x=1'); echo \"x=$x\""),
    ("read from null", "read -r x < /dev/null; echo status=$? [$x]"),
    ("read from stdin path", "read -r x < /dev/stdin <<< value; echo \"[$x]\""),
    ("read from fd path", "read -r x < /dev/fd/0 <<< value; echo \"[$x]\""),
    ("mapfile from null", "mapfile -t a < /dev/null; echo ${#a[@]}"),
    ("while read from stdin path", "while read -r l; do echo \"[$l]\"; done < /dev/stdin <<< $'a\\nb'"),
    ("command substitution from null", "x=$(</dev/null); echo \"[$x] $?\""),
    ("command substitution from stdin path", "x=$(</dev/stdin); echo \"[$x]\"", ),
    ("eval from stdin", "eval \"$(cat /dev/stdin)\" <<< 'echo evald'"),
    ("xargs -a null", "xargs -a /dev/null echo x; echo status=$?"),
    ("xargs -a stdin", "xargs -a /dev/stdin echo <<< 'p q'"),
    ("grep -f null", "echo x | grep -f /dev/null; echo status=$?"),
    ("grep -f stdin", "echo b > /tmp/p; printf 'a\\nb\\n' > /tmp/f; grep -f /dev/stdin /tmp/f < /tmp/p"),
    ("sed -f null", "echo x | sed -f /dev/null"),
    ("sed -f stdin", "echo x > /tmp/f; sed -f /dev/stdin /tmp/f <<< 's/x/y/'"),
    ("jq -f stdin", "echo '{\"a\":1}' > /tmp/j; jq -f /dev/stdin /tmp/j <<< '.a'"),
    ("jq --slurpfile null", "jq -n --slurpfile v /dev/null '$v'"),
    ("jq --rawfile stdin", "jq -n --rawfile v /dev/stdin '$v' <<< raw"),
    ("diff stdin against file", "echo a > /tmp/f; diff /dev/stdin /tmp/f <<< b; echo status=$?"),
    ("diff null against file", "printf 'a\\nb\\n' > /tmp/f; diff /dev/null /tmp/f; echo status=$?"),
    ("cmp null against file", "echo a > /tmp/f; cmp /dev/null /tmp/f; echo status=$?"),
    ("comm null against file", "printf 'a\\n' > /tmp/f; comm /dev/null /tmp/f"),
    ("join stdin with file", "echo 'k v' > /tmp/f; join /dev/stdin /tmp/f <<< 'k w'"),
    ("paste null and file", "printf 'a\\nb\\n' > /tmp/f; paste /dev/null /tmp/f"),
    ("sort -m null and file", "printf 'a\\nb\\n' > /tmp/f; sort -m /dev/null /tmp/f"),
    ("sort -c null", "sort -c /dev/null; echo status=$?"),
    ("head two nulls", "head /dev/null /dev/null"),
    ("tail null and file", "echo a > /tmp/f; tail -n 1 /dev/null /tmp/f"),
    ("wc two nulls", "wc /dev/null /dev/null"),
    ("grep -c null and file", "echo x > /tmp/f; grep -c x /dev/null /tmp/f"),
    ("grep -L null", "grep -L x /dev/null; echo status=$?"),
    ("grep -H stdin path", "grep -H x /dev/stdin <<< x"),
    ("cat -n null and file", "echo a > /tmp/f; cat -n /dev/null /tmp/f /dev/null"),
    ("sed line count null and file", "echo a > /tmp/f; sed -n '$=' /dev/null /tmp/f"),
    ("md5sum null and stdin", "md5sum /dev/null /dev/stdin <<< x"),
    ("sha256sum -c against stdin", "echo x > /tmp/f; sha256sum /tmp/f > /tmp/s; sha256sum -c /dev/stdin < /tmp/s"),
    ("head -c stdin path", "head -c 2 /dev/stdin <<< abc"),
    ("tail -c stdin path", "tail -c 3 /dev/stdin <<< abc"),
    ("find -exec cat null", "find /tmp -maxdepth 0 -exec cat /dev/null \\; ; echo status=$?"),
    ("xargs cat null", "echo /dev/null | xargs cat; echo status=$?"),
]
for label, script, *rest in SHELL_READS:
    CASES.append((f"edge dev read: {label}", script))

# /dev paths as names: test operators, lookups and errors.
NAMES = [
    ("test -e null", "[ -e /dev/null ]; echo $?"),
    ("test -f null", "[ -f /dev/null ]; echo $?"),
    ("test -d null", "[ -d /dev/null ]; echo $?"),
    ("test -c null", "[ -c /dev/null ]; echo $?"),
    ("test -b null", "[ -b /dev/null ]; echo $?"),
    ("test -s null", "[ -s /dev/null ]; echo $?"),
    ("test -p stdin pipe", "echo x | { [ -p /dev/stdin ]; echo $?; }"),
    ("test -f stdin file", "echo x > /tmp/f; [ -f /dev/stdin ] < /tmp/f; echo $?"),
    ("test -c stdin null", "[ -c /dev/stdin ] < /dev/null; echo $?"),
    ("test -e fd0", "[ -e /dev/fd/0 ]; echo $?"),
    ("test -e fd2", "[ -e /dev/fd/2 ]; echo $?"),
    ("test -e fd9", "[ -e /dev/fd/9 ]; echo $?"),
    ("test -e fd9 open", "[ -e /dev/fd/9 ] 9< /dev/null; echo $?"),
    ("test -e stderr", "[ -e /dev/stderr ]; echo $?"),
    ("cond -e stdout", "[[ -e /dev/stdout ]]; echo $?"),
    ("test -t", "[ -t 0 ]; echo $?; [ -t 1 ]; echo $?"),
    ("test -ef null", "[ /dev/null -ef /dev/null ]; echo $?"),
    ("test -ef stdin fd0", "[ /dev/stdin -ef /dev/fd/0 ]; echo $?"),
    ("test -ef null file", "echo x > /tmp/f; [ /dev/null -ef /tmp/f ]; echo $?"),
    ("test -nt null", "echo x > /tmp/f; [ /dev/null -nt /dev/null ]; echo $?"),
    ("stat -c %F null", "stat -c %F /dev/null"),
    ("stat -c %s null", "stat -c %s /dev/null"),
    ("stat -L -c %F stdin pipe", "echo x | stat -L -c %F /dev/stdin"),
    ("stat -L -c %F stdin file", "echo x > /tmp/f; stat -L -c %F /dev/stdin < /tmp/f"),
    ("file null", "file /dev/null"),
    ("file -L stdin file", "echo text > /tmp/f; file -L /dev/stdin < /tmp/f"),
    ("find -type c null", "find /dev/null -type c"),
    ("find -name null", "find /dev/null -name null"),
    ("find stdout", "find /dev/stdout -maxdepth 0; echo status=$?"),
    ("realpath null", "realpath /dev/null"),
    ("readlink -f null", "readlink -f /dev/null"),
    ("readlink -e fd9", "readlink -e /dev/fd/9; echo status=$?"),
    ("ls -d null", "ls -d /dev/null"),
    ("ls fd9", "ls /dev/fd/9; echo status=$?"),
    ("wc -c null", "wc -c /dev/null"),
    ("mkdir null", "mkdir /dev/null; echo status=$?"),
    ("mkdir -p null", "mkdir -p /dev/null; echo status=$?"),
    ("mkdir under null", "mkdir /dev/null/d; echo status=$?"),
    ("cd null", "cd /dev/null; echo status=$?"),
    ("cd stdin", "cd /dev/stdin; echo status=$?"),
    ("pushd null", "pushd /dev/null; echo status=$?"),
    ("cat under null", "cat /dev/null/x; echo status=$?"),
    ("ls under null", "ls /dev/null/x; echo status=$?"),
    ("redirect under null", "echo x > /dev/null/x; echo status=$?"),
    ("read under null", "read -r x < /dev/null/x; echo status=$?"),
    ("find under null", "find /dev/null/x; echo status=$?"),
    ("touch under null", "touch /dev/null/x; echo status=$?"),
    ("cp into null dir", "echo x > /tmp/f; cp /tmp/f /dev/null/; echo status=$?"),
    ("ls null trailing slash", "ls /dev/null/; echo status=$?"),
    ("test -e null trailing slash", "[ -e /dev/null/ ]; echo $?"),
    ("source under null", ". /dev/null/x; echo status=$?"),
    ("grep -r null", "grep -r x /dev/null; echo status=$?"),
    ("process substitution path", "echo <(true) >(true)"),
    ("process substitution test -e", "[ -e <(true) ]; echo $?"),
    ("process substitution test -p", "[ -p <(true) ]; echo $?"),
    ("process substitution test -f", "[ -f <(true) ]; echo $?"),
    ("process substitution wc name", "wc -c <(printf abc)"),
    ("process substitution head names", "head -n 1 <(seq 3) <(seq 4 6)"),
    ("process substitution grep -H", "grep -H x <(echo x)"),
    ("process substitution md5sum name", "md5sum <(echo a)"),
    ("process substitution cmp", "cmp <(echo a) <(echo b); echo status=$?"),
    ("process substitution diff", "diff <(printf 'a\\nb\\n') <(printf 'a\\nc\\n'); echo status=$?"),
    ("process substitution comm", "comm <(printf 'a\\nb\\n') <(printf 'b\\nc\\n')"),
    ("process substitution join", "join <(printf '1 a\\n') <(printf '1 b\\n')"),
    ("process substitution paste", "paste <(seq 2) <(seq 3 4)"),
    ("process substitution sort -m", "sort -m <(printf 'a\\nc\\n') <(printf 'b\\nd\\n')"),
    ("process substitution exec fd", "exec 3< <(echo from-fd3); cat <&3"),
    ("process substitution read fd path", "exec 3< <(echo via-path); cat /dev/fd/3"),
    ("process substitution output tee", "echo x | tee >(tr x y) >/dev/null"),
    ("process substitution output redirect", "echo x > >(tr x z)"),
    ("process substitution twice same", "cat <(echo 1) <(echo 2) <(echo 3)"),
    ("process substitution in array", "a=(<(true) <(true)); echo ${#a[@]}"),
    ("capture-like file name survives /dev/null", "echo keep > /.bash-tool-capture-0; head -c 1 /dev/null; cat /.bash-tool-capture-0; echo status=$?"),
    ("capture-like file names survive a utility", "for i in 0 1 2 3 4 5; do echo keep$i > /.bash-tool-capture-$i; done; printf 'x\\n' | od -c > /dev/null; cat /.bash-tool-capture-*; echo status=$?"),
]
for label, script, *rest in NAMES:
    CASES.append((f"edge dev names: {label}", script))


# Divergences that are decisions, not bugs.
EXPECTED = {
    'edge dev read: source stdin': (
        2, b'',
        b'bash: source of /dev/stdin, /dev/stdout or /dev/stderr is unsupported in bash-tool\n',
        "refusal: source reads and checks the whole script before any "
        "of it runs, and in a call's single task that read cannot wait for a stream still being "
        "written, so sourcing a standard stream is refused (a regular file, a process "
        "substitution's /dev/fd/N and /dev/null are sourced)",
    ),
}
