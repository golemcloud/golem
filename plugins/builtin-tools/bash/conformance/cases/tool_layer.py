"""The tool layer: resource bounds, the virtual /dev and standard streams of the embedded utilities,
and the environment and working directory they see.
"""

# recursion and nesting within the WASM stack.
RECURSE = 'f(){ local n=$1; if [ "$n" -gt 0 ]; then f $((n-1)); else echo bottom; fi; }'
FIB = 'fib(){ if [ $1 -lt 2 ]; then echo $1; else echo $(( $(fib $(($1-1))) + $(fib $(($1-2))) )); fi; }'
NESTED_SUBST = "echo " + "$(echo " * 30 + "deep" + ")" * 30
NESTED_BRACES = "{ " * 400 + "echo hi; " + "}; " * 399 + "}"
CASES = [
    ("tool-layer: recursion 30 deep", f"{RECURSE}; f 30; echo status=$?"),
    ("tool-layer: recursion 60 deep", f"{RECURSE}; f 60; echo status=$?"),
    ("tool-layer: arithmetic recursion 31 deep",
     "f() { if (( $1 > 0 )); then f $(( $1 - 1 )); fi; }; f 31; echo status=$?"),
    ("tool-layer: fib through command substitution", f"{FIB}; fib 14"),
    ("tool-layer: FUNCNEST", "FUNCNEST=50; f() { f; }; f; echo after $?"),
    ("tool-layer: FUNCNEST=5",
     "FUNCNEST=5; f() { echo $1; f $(($1+1)); echo back$1; }; f 1; echo after $?"),
    ("tool-layer: FUNCNEST in a substitution",
     "f() { f; }; x=$(FUNCNEST=3 f); echo st=$? x=$x; echo end"),
    ("tool-layer: arithmetic variable recursion", 'a="a+1"; echo $((a)); echo after'),
    ("tool-layer: thirty nested substitutions", NESTED_SUBST),
    ("tool-layer: tree walk 21 deep",
     "mkdir -p /tmp/t/" + "/".join(["d"] * 21) + "; walk() { local d; for d in \"$1\"/*/; do "
     "[ -d \"$d\" ] && walk \"${d%/}\"; done; echo \"${1#/tmp/t}\"; }; walk /tmp/t | wc -l"),
    ("tool-layer: 400 nested braces", NESTED_BRACES),
    ("tool-layer: endless recursion ends loudly",
     "f() { f; }; f; echo after"),
]

# what the shell holds in memory is bounded; a cut shows only if it is read.
CASES += [
    ("tool-layer: command substitution over the limit",
     "x=$(yes | head -c 68000000); echo st=$? ${#x}; echo after"),
    ("tool-layer: command substitution under the limit",
     "x=$(yes | head -c 67000000); echo st=$? ${#x}"),
    ("tool-layer: large output through a pipe", "yes | head -c 30000000 | wc -c"),
    ("tool-layer: process substitution read by head",
     'cat <(yes) | head -1; echo "st=${PIPESTATUS[*]}"'),
    ("tool-layer: output process substitution",
     "yes > >(head -1); echo st=$?"),
    ("tool-layer: process substitution read to the end",
     'cat <(yes | head -c 68000000) | wc -c; echo "st=${PIPESTATUS[*]}"'),
    ("tool-layer: process substitution under the limit",
     'cat <(yes | head -c 67000000) | wc -c; echo "st=${PIPESTATUS[*]}"'),
    ("tool-layer: null device keeps nothing",
     "yes | head -c 50000000 > /dev/null; echo st=$?; exec 3>/dev/null; seq 3 >&3; echo st=$?"),
]

# the embedded utilities read a bounded prefix of piped input and keep a bounded
# output; a reader that stops early sees bash's answer, one that reads the cut gets an error.
CASES += [
    ("tool-layer: yes into nl into head",
     'yes | nl | head -1; echo "st=${PIPESTATUS[*]}"'),
    ("tool-layer: yes into base64 into head",
     'yes | base64 | head -c 20; echo; echo "st=${PIPESTATUS[*]}"'),
    ("tool-layer: endless shuf into head",
     'shuf -r -i 1-5 | head -3 | wc -l; echo "st=${PIPESTATUS[*]}"'),
    ("tool-layer: piped input over the limit",
     'yes | head -c 68000000 | md5sum; echo "st=${PIPESTATUS[*]}"'),
    ("tool-layer: piped input under the limit",
     'yes | head -c 67000000 | md5sum; echo "st=${PIPESTATUS[*]}"'),
    ("tool-layer: output into a file is not limited",
     "yes | head -c 51000000 > /tmp/f; base64 /tmp/f > /tmp/g; wc -c < /tmp/g; base64 -d /tmp/g > /tmp/h; "
     "cmp /tmp/f /tmp/h && echo same"),
    ("tool-layer: a file operand leaves standard input alone",
     "echo a > /tmp/f; { nl /tmp/f; cat; } <<< data"),
    ("tool-layer: tac of a large file",
     'yes | head -c 50000000 > /tmp/f; echo last >> /tmp/f; tac /tmp/f | head -2; '
     'tac < /tmp/f | head -1; echo "st=${PIPESTATUS[*]}"'),
    ("tool-layer: tac keeps records across blocks",
     "for n in 1 65535 65536 65537 200000; do printf '%*s\\n' $n '' | tr ' ' x; done > /tmp/f; "
     "printf tail >> /tmp/f; tac /tmp/f | cksum; tac < /tmp/f | cksum"),
    ("tool-layer: sort of a file over the limit",
     'yes | head -c 70000000 > /tmp/f; sort /tmp/f | head -1; echo "st=${PIPESTATUS[*]}"'),
]

# synchronous builtins write whole into a pipe.
CASES += [
    ("tool-layer: declare -p into a pipe",
     "x=$(printf '%*s' 100000 ''); declare -p x | wc -c; echo \"${PIPESTATUS[*]}\""),
    ("tool-layer: declare -p round trip",
     'arr=($(seq 20000)); s=$(declare -p arr); unset arr; eval "$s"; echo ${#arr[@]}'),
    ("tool-layer: type of a large function into a pipe",
     "eval \"big() { $(for i in $(seq 3000); do printf 'echo %s; ' $i; done) }\"; "
     "type big | wc -l; declare -f big | tail -2"),
]

# /dev/stdin, /dev/stdout and /dev/fd/N are the command's own descriptors.
CASES += [
    ("tool-layer: /dev/stdin for diff, cmp, sed and grep",
     'cd /tmp; printf "a\\nb\\n" > f; printf "a\\nc\\n" | diff /dev/stdin f; echo st=$?; '
     'printf "a\\nb\\n" | cmp /dev/stdin f; echo st=$?; echo x | sed "r /dev/stdin" f; echo st=$?; '
     'echo a | grep -f /dev/stdin f; echo st=$?'),
    ("tool-layer: paste reads /dev/stdin twice as one pipe",
     "printf '1\\n2\\n3\\n4\\n' | paste /dev/stdin /dev/stdin"),
    ("tool-layer: naming /dev/stdin does not drain it",
     "{ ls /dev/stdin >/dev/null; cat; } <<< data"),
    ("tool-layer: sort reads /dev/stdin twice as one pipe",
     "printf 'b\\na\\n' | sort /dev/stdin /dev/stdin"),
    ("tool-layer: closed standard input",
     "echo hi | cat <&-; echo st=$?; echo hi | nl <&-; echo st=$?"),
    ("tool-layer: writing to /dev/fd/3",
     'cd /tmp; printf src > src; : > out; exec 3<>/tmp/out; cp src /dev/fd/3; echo st=$?; exec 3>&-; '
     'printf "[%s]\\n" "$(cat out)"'),
    ("tool-layer: sort -o /dev/fd/4",
     "cd /tmp; printf 'b\\na\\n' > f; exec 4>/tmp/out2; sort -o /dev/fd/4 f; echo st=$?; exec 4>&-; cat /tmp/out2"),
    ("tool-layer: /dev/fd/3 opens the file again",
     'cd /tmp; printf "file\\n" > f; exec 3</tmp/f; sort /dev/fd/3 >/dev/null; cat <&3; echo end'),
    ("tool-layer: closed descriptors do not exist",
     "[ -e /dev/fd/7 ] && echo e-true || echo e-false; [ -w /dev/fd/9 ] && echo w-true || echo w-false; "
     "exec 7</dev/null; [ -e /dev/fd/7 ] && echo e7-true; ls /dev/fd/7 >/dev/null 2>&1; echo st=$?"),
    ("tool-layer: planted capture names",
     "mkdir /.bash-tool-capture-0 /.bash-tool-capture-1 /.bash-tool-capture-2; echo planted\n"
     "#--call-- /\nprintf hi; echo; basename /a/b; echo st=$?; base64 <<< x; echo st=$?"),
    ("tool-layer: a user file named like a capture survives",
     "echo keep > /.bash-tool-capture-0; basename /a/b; nl <<< x; cat /.bash-tool-capture-0"),
    ("tool-layer: utilities writing to /dev/null",
     "cd /tmp; yes | head -c 1000000 > big; cp big /dev/null; echo st=$?; sort -o /dev/null big; echo st=$?; "
     "nl big > /dev/null; echo st=$?"),
]

# split, csplit, factor, date -f - and -i prompts read the pipeline's input.
CASES += [
    ("tool-layer: factor, split and date read standard input",
     "echo 12 | factor; echo st=$?; cd /tmp; seq 10 | split -l 4; echo st=$?; ls x*; cat xac; "
     "echo 2024-01-02 | date -u -f - +%Y; echo st=$?"),
    ("tool-layer: csplit reads standard input",
     "cd /tmp; seq 30 | csplit -s - 15; echo st=$?; wc -l xx00 xx01"),
    ("tool-layer: answers to rm -i",
     "cd /tmp; touch f g; echo y | rm -i f 2>/dev/null; echo n | rm -i g 2>/dev/null; ls f g 2>&1; true"),
]

NESTING = (
    "bash-tool's limit: nesting deeper than its WASM stacks can hold is refused (static nesting, "
    "before the script runs) or stopped with an error (recursion); bash runs until its own stack "
    "overflows and crashes"
)
BUFFERED = (
    "bash-tool's limit: a substitution holds at most 64 MiB in memory; past it the writer gets "
    "SIGPIPE and whoever reads the cut output gets an error"
)
# cat onto its own input, and yes's options.
CASES += [
    ("tool-layer: cat f >> f",
     "echo x > /tmp/o; cat /tmp/o >> /tmp/o; echo st=$?; cat /tmp/o; cat /tmp/o > /tmp/o; echo st=$?; wc -c < /tmp/o"),
    ("tool-layer: yes options",
     'yes -- -a | head -n 2; yes -a | head -1; echo "st=${PIPESTATUS[*]}"; yes --bogus | head -1; '
     'echo "st=${PIPESTATUS[*]}"; yes a -- b | head -1; yes - | head -1; yes -- | head -1'),
    ("tool-layer: yes --version ends", "yes --version > /dev/null; echo st=$?; yes --help | head -1"),
]

# a working directory the process cannot enter is still where relative paths go.
CASES += [
    ("tool-layer: cd /dev then rm", "mkdir -p /tmp/project; cd /dev && rm -rf tmp; echo rm=$?; ls /tmp"),
    ("tool-layer: deleted working directory",
     "mkdir -p /tmp/job/out; cd /tmp/job/out; rm -rf /tmp/job; touch newfile; echo st=$?; ls /newfile 2>&1"),
]

# device paths at every libc entry point.
CASES += [
    ("tool-layer: changing devices",
     "touch /dev/null; echo st=$?; rm /dev/null; echo st=$?; ln /dev/null /tmp/link; echo st=$?; "
     "touch /dev/stdout; echo st=$?; rmdir /dev/fd; echo st=$?; cd /dev; touch x; echo st=$?"),
    ("tool-layer: device statuses",
     "cd /tmp; echo a > f; mkdir /dev/foo 2>/dev/null; echo st=$?; mv /dev/null /tmp/x 2>/dev/null; echo st=$?; "
     "cp -p f /dev/null 2>/dev/null; echo st=$?; truncate -s 0 /dev/null 2>/dev/null; echo st=$?; ls /tmp"),
    ("tool-layer: a device is no directory", "cat /dev/null/; echo st=$?; cat /dev/null/.; echo st=$?"),
    ("tool-layer: listing /dev", "ls /dev; echo st=$?"),
]

# the script's environment reaches the embedded utilities.
CASES += [
    ("tool-layer: TZ reaches date",
     "TZ=JST-9 date -d @0 +%H:%M; export TZ=EST5; date -d @0 +%H; TZ=UTC-3 date -d @0 +%H; unset TZ; date -d @0 +%H"),
    ("tool-layer: TMPDIR reaches mktemp",
     "mkdir /tmp/mine; TMPDIR=/tmp/mine mktemp | cut -c1-13; export TMPDIR=/tmp/mine; mktemp -d | cut -c1-13"),
    ("tool-layer: backup suffix reaches cp",
     "cd /tmp; echo a > a; echo b > b; SIMPLE_BACKUP_SUFFIX=.bak cp -b a b; ls b*"),
    ("tool-layer: touch -d offsets and local time",
     'touch -d "2024-01-01 00:00 +0100" /tmp/t; TZ=UTC date -r /tmp/t +%F_%H:%M; '
     'TZ=JST-9 touch -d "2024-01-01 00:00:00" /tmp/u; TZ=UTC date -r /tmp/u +%F_%H:%M'),
]

# env runs its command in the shell.
CASES += [
    ("tool-layer: env with a command",
     'env FOO=bar sh -c "echo \\$FOO"; echo st=$?; env -i sh -c "echo [\\$HOME][\\$FOO]"; X=1; export X; '
     'env -u X sh -c "echo [\\$X]"; env -C /tmp pwd; env -- A=2 sh -c "echo \\$A"; env - B=3 sh -c "echo \\$B"; '
     'env echo hi | tr a-z A-Z; env false; echo st=$?'),
    ("tool-layer: env -S splits its string", "env -S 'echo hi'; echo st=$?"),
    ("tool-layer: env command errors",
     "env nosuch 2>/dev/null; echo st=$?; env /tmp 2>/dev/null; echo st=$?; env -0 true; echo st=$?"),
]

# split --filter, -u and empty input.
CASES += [
    ("tool-layer: split --filter",
     'cd /tmp; seq 5 > in; split -l 2 --filter="cat > \\$FILE.f" in; ls; seq 4 | split -l 2 --filter="echo \\$FILE; cat" - part'),
    ("tool-layer: split --filter failing",
     'cd /tmp; seq 5 > in; split -l 2 --filter="exit 3" in; echo st=$?; split -n 2 --filter="wc -c" in; echo st=$?'),
    ("tool-layer: split -u and empty input",
     "cd /tmp; seq 5 > in; split -u -l 2 in; echo st=$?; ls x*; rm -f x*; : > empty; split -l 2 empty; echo st=$?; "
     "ls x* 2>/dev/null; echo n=$?"),
]

# commands named by a path.
CASES += [
    ("tool-layer: missing and directory path commands",
     "./missing.sh; echo st=$?; /abs/missing 2>/dev/null; echo st=$?; /non/existent | cat; echo ${PIPESTATUS[*]}; "
     "/tmp; echo st=$?; nosuchcmd; echo st=$?"),
    ("tool-layer: a file as a command", 'echo "echo hi" > /tmp/s.sh; /tmp/s.sh; echo st=$?'),
]

# the text drivers' I/O errors, in each utility's own words.
CASES += [
    ("tool-layer: directory operands",
     'mkdir -p /tmp/d; echo f > /tmp/f; for c in "uniq" "cut -c1" "tail -n1" "rev" "tac" "jq ." "head -c1" '
     '"sort" "od -c" "sed p" "nl" "grep x"; do $c /tmp/d /tmp/f 2>&1; echo "$c st=$?"; done'),
    ("tool-layer: directory on standard input",
     "mkdir -p /tmp/d; tr a b < /tmp/d; echo st=$?; uniq < /tmp/d; echo st=$?; cut -c1 < /tmp/d; echo st=$?; "
     "sort -n < /tmp/d; echo st=$?"),
    ("tool-layer: uniq output that cannot be opened",
     'printf "a\\n" | uniq - /no/such/dir/out; echo st=$?'),
    ("tool-layer: an empty operand names nothing",
     "ls ''; echo st=$?; cat ''; echo st=$?; wc -l ''; echo st=$?"),
]

# printf formats each item without a capture per item.
CASES += [
    ("tool-layer: printf of many items", 'printf "%s\\n" $(seq 10001) | tail -n 1; printf "%d-%s\\n" $(seq 20000) | wc -l'),
]

# Real bash's own stack unwinding on an unbounded recursion (both here and in `f 60 deep`-style
# cases above) takes a variable amount of wall-clock time -- reproducibly over the default 15s
# under a loaded CI runner running many oracle containers at once, even though it finishes in a
# few seconds in isolation. This doesn't change what the case tests, just how long it's given.
OPTIONS = {
    "tool-layer: endless recursion ends loudly": {"timeout": 60},
    # 51 MB through base64 and back takes about 12 s alone, past the default under a loaded run.
    "tool-layer: output into a file is not limited": {"timeout": 60},
}

# bytes that are not UTF-8 reach the tool's own commands and come back from them intact.
CASES += [
    ("tool-layer: rev of a line that is not UTF-8",
     r"""printf 'ab\xffcd\nxy\n' | rev | od -c; printf 'x\xff' | rev | od -c"""),
    ("tool-layer: xargs keeps an item's bytes",
     r"""x=$'\xff'; printf %s "$x" | xargs printf '%s\n' | od -c; printf 'a\xffb\n' | xargs echo | od -c"""),
    ("tool-layer: xargs -0 and -I keep bytes",
     r"""printf 'a\xffb\0' | xargs -0 printf '%s\n' | od -c; printf 'a\xffb\n' | xargs -I{} printf '[%s]\n' {} | od -c"""),
    ("tool-layer: xargs -d and -E take bytes",
     r"""printf 'a\xffb\n' | xargs -d $'\xff' printf '[%s]' | od -c; """
     r"""x=$'\xff'; printf 'a\nSTOP\xff\nb\n' | xargs -E "STOP$x" echo | od -c"""),
    ("tool-layer: xargs -t quotes a byte",
     r"""x=$'\xff'; printf 'a\n' | xargs -t echo "$x" 2>&1 | od -c"""),
    ("tool-layer: yes and seq write argument bytes",
     r"""x=$'\xff'; yes "a${x}b" | head -1 | od -c; seq -s "$x" 3 | od -c; seq -f "%g$x" 2 | od -c"""),
    ("tool-layer: env prints value bytes",
     r"""x=$'\xff'; env -i V="$x" env | od -c; export W="$x"; env | grep -a '^W=' | od -c"""),
    ("tool-layer: find -printf and stat -c write format bytes",
     r"""x=$'\xff'; find /tmp -maxdepth 0 -printf "%p$x\n" | od -c; stat -c "%n$x" /tmp | od -c"""),
    ("tool-layer: sh reads script bytes from standard input",
     r"""printf 'echo a\xffb\n' | bash | od -c; printf 'printf %%s "\xff"\n' | sh | od -c"""),
    ("tool-layer: jq reads argument bytes as text",
     r"""x=$'\xff'; jq -n --arg v "a${x}b" '$v' | od -c; V="$x" jq -rn 'env.V' | od -c"""),
]

# A command's program path (`/bin/cat`, `/usr/bin/cat`, what `which` and `type -P` print) runs
# the command, and `-x` holds for it; a path that names no command is still no program.
CASES += [
    ("tool-layer: a program path runs the command",
     "printf 'a\\nb\\n' > f; /bin/cat f; /bin/echo hi; printf '1\\n2\\n' | /usr/bin/sort -r; "
     "/usr/bin/head -1 f; echo st=$?"),
    ("tool-layer: program paths in a pipeline and a substitution",
     "yes | /usr/bin/head -2; x=$(/bin/echo sub); echo $x; printf 'b\\na\\n' > f; "
     "/bin/cat f | /usr/bin/sort | /bin/cat -n; f=/bin/cat; $f f"),
    ("tool-layer: the path which and type -P print runs the command",
     """echo '{"a":1}' | "$(which jq)" .a; "$(type -P cat)" <<< hi; "$(command -v jq)" -n 1"""),
    ("tool-layer: env, xargs and find -exec run a program path",
     "/usr/bin/env x=1 printenv x; echo a b | /usr/bin/xargs /bin/echo X; seq 2 | xargs -n1 /bin/echo n; "
     "mkdir d; touch d/a; find d -name a -exec /bin/echo got {} \\;; /usr/bin/find d -name a"),
    ("tool-layer: -x holds for a program path",
     "[ -x /bin/cat ] && echo a; test -x /usr/bin/jq && echo b; [[ -x /usr/bin/sort ]] && echo c; "
     "for c in /bin/cat /usr/bin/head; do [ -x \"$c\" ] && echo \"$c ok\"; done"),
    ("tool-layer: -x on a directory and on other files",
     "mkdir d; [ -x d ] && echo dir; printf x > g; [ -x g ] || echo notx; [ -x ./g ] || echo notx2; "
     "[ -x /bin/nosuchcommand ] || echo none; [ -x /bin/cd ] || echo no-cd; "
     "[ -x /bin/export ] || echo no-export"),
    ("tool-layer: a path that names no command is no program",
     "/bin/nosuchcommand; echo st=$?; /bin/cd /; echo st=$?; /bin/export A=1; echo st=$?; "
     "/bin/cat nosuch; echo st=$?"),
    ("tool-layer: which and type name a program path",
     "which cat sed; which /bin/cat; echo st=$?; which cd; echo st=$?; which /bin/nosuch; echo st=$?; "
     "type -P cat; type /bin/cat"),
    ("tool-layer: a program path is resolved as a path",
     "/bin/../bin/cat /dev/null; echo st=$?; /bin/./echo x; exec /bin/echo via-exec"),
    ("tool-layer: command runs a program path", "command /bin/echo via-command; /bin/kill -0 $$; echo st=$?"),
    ("tool-layer: printenv prints the environment",
     "env -i A=1 B=2 printenv; echo st=$?; env -i A=1 printenv -0 | od -c"),
    ("tool-layer: printenv prints named variables",
     "export A='x y'; printenv A NOPE A; echo st=$?; printenv 'A=x y'; echo st=$?; "
     "printenv ''; echo st=$?; B=local; printenv B; echo st=$?; B=2 printenv B; "
     "printenv -- A; export B; printenv -0 A B | od -c"),
    ("printenv: tool-layer options and their errors",
     "printenv -x; echo st=$?; printenv --bogus; echo st=$?; printenv --help | head -1",
     ["error"]),
    ("tool-layer: printenv writes value bytes",
     r"""x=$'\xff'; export X="a${x}b"; printenv X | od -c"""),
    ("tool-layer: /usr/bin and /bin both hold every command",
     "printf 'a\\n' > f; /usr/bin/cat f; /usr/bin/env A=1 /usr/bin/printenv A; echo st=$?; "
     "/bin/bash -c 'echo b'; /usr/bin/sed -n 1p f; /usr/bin/./cat f"),
    ("tool-layer: which, type -P and command -v name the commands",
     "which jq; type -P jq; command -v jq; type jq; jq -n 1 >/dev/null; hash -t jq; type jq"),
    ("tool-layer: command -v and type report a program's file",
     "command -v cat; command -v echo; command -v cd; command -V cat; command -V echo; type cat; "
     "type echo; type -t cat echo read; type -p cat; type -p echo; echo st=$?; type -P echo cat"),
    ("tool-layer: the command -v idiom finds a program",
     """[ -x "$(command -v cat)" ] && echo have-cat; [ -x "$(command -v echo)" ] || echo builtin; """
     """if ! [ -x "$(command -v nosuchtool)" ]; then echo no-tool; fi"""),
    ("tool-layer: hash a program",
     "hash cat; echo st=$?; hash -t cat; hash echo; echo st=$?; hash read; echo st=$?; "
     "hash nosuch; echo st=$?; f() { :; }; hash f; echo st=$?; hash -r; hash -t cat; echo st=$?"),
    ("tool-layer: running a program hashes it",
     "cat /dev/null; type cat; hash -t cat; type -a cat; type -a echo"),
    ("tool-layer: a builtin only a shell can be has no program path",
     "/bin/read x <<< hi; echo st=$? x=$x; /bin/cd /; echo st=$?; /bin/export A=1; echo st=$?; "
     "/bin/declare x; echo st=$?; /usr/bin/read; echo st=$?; [ -x /bin/read ] || echo no-read; "
     "which echo read cd; echo st=$?; echo x | xargs read; echo st=$?; echo x | xargs echo got"),
    ("tool-layer: bash builtins that are programs run by path",
     "/bin/echo hi; /usr/bin/printf '%s\\n' p; /usr/bin/test 1 = 1 && echo t; "
     "/usr/bin/[ 1 = 1 ] && echo t2; /bin/true && /bin/false || echo f; /bin/pwd; "
     "/bin/kill -0 $$ && echo k; [ -x /bin/echo ] && echo x-echo; [ -x /usr/bin/test ] && echo x-test"),
    ("tool-layer: the program directories are not directories",
     "[ -d /bin ] || echo nodir; [ -x /bin ] || echo nox; [ -e /bin/cat ] || echo nofile; "
     "ls /bin > /dev/null 2>&1 || echo nolist; cd /usr/bin 2>/dev/null || echo nocd"),
]

# `builtin` and `enable` treat only bash's own builtins as shell builtins: this shell's commands
# (`cat`, `jq`, `env`) stand for programs, as they are on Linux.
CASES += [
    ("tool-layer: builtin runs only a shell builtin",
     "builtin env; echo st=$?; builtin cat /dev/null; echo st=$?; builtin jq -n 1; echo st=$?; "
     "builtin sh -c 'echo x'; echo st=$?; builtin timeout 1 true; echo st=$?; "
     "builtin echo hi; echo st=$?; builtin nosuch; echo st=$?; builtin type -t cat; "
     "builtin test 1 = 1 && echo t"),
    ("tool-layer: enable names only a shell builtin",
     "enable -n cat; echo st=$?; enable cat; echo st=$?; enable -n nosuch; echo st=$?; "
     "enable echo cat; echo st=$?; enable -n cat jq nosuch; echo st=$?"),
    ("tool-layer: compgen -b lists only shell builtins",
     "compgen -b; compgen -A builtin c; compgen -A enabled | wc -l; compgen -A disabled | wc -l; "
     "compgen -b | grep -c -e '^cat$' -e '^jq$' -e '^timeout$'"),
    ("tool-layer: compgen -c lists this shell's commands",
     "compgen -c cat; compgen -c case; compgen -c jq; compgen -c timeo; compgen -c calle"),
    ("tool-layer: help knows only shell builtins",
     "help cat; echo st=$?; help jq sed; echo st=$?; help -s cat; echo st=$?; help -d cat; "
     "echo st=$?; help nosuch; echo st=$?; help nosuch cat; echo st=$?; "
     "help echo cat >/dev/null; echo st=$?; help -d cat echo >/dev/null; echo st=$?"),
    ("tool-layer: enable lists only shell builtins",
     "enable -p; enable | wc -l; enable -a | wc -l; enable -s; "
     "enable -p | grep -c -e ' cat$' -e ' jq$' -e ' ls$'; enable -n | wc -l"),
]

# Error paths of commands the coverage check found registered but unlisted (the checklist now
# lists them); each matches bash with GNU coreutils except where a fixture says why.
CASES += [
    ("dd: tool-layer error paths",
     "dd if=/nonexistent of=/dev/null; echo st=$?; dd bs=0 if=/dev/null; echo st=$?; "
     "dd foo=1; echo st=$?", ["error"]),
    ("du: tool-layer error paths", "du /nonexistent; echo st=$?; du --bogus; echo st=$?", ["error"]),
    ("nproc: tool-layer error paths", "nproc --bogus; echo st=$?", ["error"]),
    ("uname: tool-layer error paths", "uname -x; echo st=$?; uname --bogus; echo st=$?", ["error"]),
    ("hostname: tool-layer error paths", "hostname -x; echo st=$?", ["error"]),
]

# `timeout` runs a command with a time limit, as GNU's does (the call's own limit is the
# tool's `timeout` argument, tested in the session's unit tests).
CASES += [
    ("tool-layer: timeout stops a command",
     "timeout 0.3 sleep 5; echo st=$?; timeout 5 sleep 0.1; echo st=$?; "
     "timeout 0.3 bash -c 'while :; do :; done'; echo st=$?; "
     "timeout 0.3 bash -c 'while [[ 1 ]]; do x=$((x+1)); done'; echo st=$?"),
    ("tool-layer: timeout passes on the command's status and output",
     "timeout 1 sh -c 'exit 3'; echo st=$?; timeout 1 echo hi there; timeout 1 printf '%s\\n' a b; "
     "echo in | timeout 1 cat; timeout 1 bash -c 'echo $0' x; timeout 0 sh -c 'echo ran'; "
     "echo st=$?; timeout inf true; echo st=$?"),
    ("tool-layer: timeout signals and statuses",
     "timeout --preserve-status 0.3 sleep 5; echo st=$?; timeout -s INT 0.3 sleep 5; echo st=$?; "
     "timeout --signal=HUP 0.3 sleep 5; echo st=$?; timeout -s 0 0.2 sleep 1; echo st=$?; "
     "timeout -v 0.3 sleep 5; echo st=$?; timeout 1 timeout 0.2 sleep 3; echo st=$?"),
    ("tool-layer: timeout durations",
     "timeout 1s true; timeout 1h true; timeout 1d true; timeout .5 true; timeout 1e0 true; "
     "echo st=$?; timeout 0.001m sleep 5; echo st=$?; timeout -- 1 true; echo st=$?"),
    ("timeout: its own errors",
     "timeout; echo st=$?; timeout 5; echo st=$?; timeout x sleep 1; echo st=$?; "
     "timeout -1 true; echo st=$?; timeout 1z true; echo st=$?; timeout -s FOO 1 true; echo st=$?; "
     "timeout -s 99 1 true; echo st=$?; timeout --bogus 1 true; echo st=$?; "
     "timeout -x 1 true; echo st=$?; timeout -k 1; echo st=$?; timeout --kill-after=x 1 true; "
     "echo st=$?",
     ["error"]),
    ("tool-layer: timeout of a command that cannot run",
     "timeout 1 nosuch; echo st=$?; timeout 1 /tmp; echo st=$?; timeout 1 cd /; echo st=$?; "
     "timeout 1 ./nofile; echo st=$?; f() { echo fn; }; timeout 1 f; echo st=$?; "
     "timeout 1 sleep -5; echo st=$?"),
    ("tool-layer: timeout runs the command as a new process",
     "x=5; timeout 1 bash -c 'echo ${x-unset}'; export x; timeout 1 bash -c 'echo $x'; "
     "cd /tmp; timeout 1 pwd; timeout 1 xargs echo <<< 'a b'; timeout 2 true & wait $!; echo st=$?"),
    ("tool-layer: timeout --help",
     "timeout --help | sed -n '1,/the exit status of COMMAND otherwise/p'; timeout --he | head -1; "
     "timeout --version >/dev/null; echo st=$?"),
    ("tool-layer: timeout that KILLs",
     "timeout -s KILL 0.3 sleep 5; echo st=$?"),
    ("tool-layer: timeout of a shell with its own TERM handling",
     "timeout 0.3 sh -c 'trap \"echo caught; exit 5\" TERM; while :; do :; done'; echo st=$?; "
     "timeout -k 0.3 0.3 sh -c 'trap \"\" TERM; sleep 5'; echo st=$?"),
]

# curl against a port that refuses, so both sides fail the same way before any byte
# arrives; -w and files show what curl sets up first.
CURL_REFUSED = "http://127.0.0.1:1/"
CURL_ERR = "2>&1 | sed -e \"s/[\u2018\u2019]/'/g\" -e 's/after [0-9]* ms/after N ms/'"
CASES += [
    ("tool-layer: curl --data-urlencode forms",
     "curl -s -G --data-urlencode 'a=b c&d' --data-urlencode '\u00e9' --data-urlencode '=x y' "
     "--data-urlencode 'k=' -w '%{url_effective}\\n' http://127.0.0.1:1/p; echo status=$?"),
    ("tool-layer: curl --data-urlencode from a file",
     "echo 'q r' > /tmp/d; curl -s -G --data-urlencode 'n@/tmp/d' --data-urlencode '@/tmp/d' "
     "-w '%{url_effective}\\n' http://127.0.0.1:1/p; echo status=$?; "
     "curl -G --data-urlencode 'n@/tmp/nosuch' http://127.0.0.1:1/p; echo status=$?"),
    ("tool-layer: curl -K reads options from a file",
     "printf '%s\\n' '# options' 'url = \"" + CURL_REFUSED + "\"' '-w \"[%{http_code}]\\n\"' "
     "'silent' '' 'max-time: 5' > /tmp/c; curl -K /tmp/c; echo status=$?; "
     "printf '%s\\n' '--url=" + CURL_REFUSED + "' 'write-out \"<%{exitcode}>\"' > /tmp/c; "
     "curl -s --config /tmp/c; echo status=$?"),
    ("tool-layer: curl -K file errors",
     "for line in bogus max-time '  -s' '-K /tmp/c' 's'; do printf '%s\\n' \"$line\" > /tmp/c; "
     "curl -K /tmp/c " + CURL_REFUSED + " " + "2>&1; echo status=$?; done; "
     "curl -K /tmp/nosuch -s " + CURL_REFUSED + " 2>&1; echo status=$?; "
     "curl -s -K /tmp/nosuch " + CURL_REFUSED + " 2>&1; echo status=$?"),
    ("tool-layer: curl unreadable data files",
     "curl -d @/tmp/nosuch " + CURL_REFUSED + " 2>&1; echo status=$?; "
     "curl -s --data @/tmp/nosuch " + CURL_REFUSED + " 2>&1; echo status=$?; "
     "curl --data-binary @/tmp/nosuch -s " + CURL_REFUSED + " 2>&1; echo status=$?; "
     "curl -T /tmp/nosuch " + CURL_REFUSED + " 2>&1; echo status=$?; "
     "curl -F 'f=@/tmp/nosuch' " + CURL_REFUSED + " 2>&1; echo status=$?; "
     "curl -sF 'f=@/tmp/nosuch' " + CURL_REFUSED + " 2>&1; echo status=$?"),
    ("tool-layer: curl badly used and blank arguments",
     "curl --form novalue " + CURL_REFUSED + " 2>&1; echo status=$?; "
     "curl -s -F novalue " + CURL_REFUSED + " 2>&1; echo status=$?; "
     "curl -o '' " + CURL_REFUSED + " 2>&1; echo status=$?; "
     "curl -s -- '' 2>&1; echo status=$?; curl " + CURL_REFUSED + " '' 2>&1; echo status=$?; "
     "curl -sS -F =x " + CURL_REFUSED + " " + CURL_ERR + "; "
     "curl --retry -1 " + CURL_REFUSED + " 2>&1; echo status=$?; "
     "curl --retry 1x " + CURL_REFUSED + " 2>&1; echo status=$?; curl -Q 2>&1; echo status=$?"),
    ("tool-layer: curl several URLs pair with their outputs",
     "cd /tmp; curl -s -o o1 -O " + CURL_REFUSED + " " + CURL_REFUSED + " " + CURL_REFUSED
     + " -w '%{exitcode} %{url_effective}\\n'; echo status=$?; ls /tmp"),
    ("tool-layer: curl sets up its output before connecting",
     "cd /tmp; curl --create-dirs -o a/b/c " + CURL_REFUSED + " " + CURL_ERR + "; "
     "curl -O http://127.0.0.1:1 " + CURL_ERR + "; curl -O http://127.0.0.1:1/ " + CURL_ERR
     + "; curl -O http://127.0.0.1:1/d/ " + CURL_ERR + "; curl -s -O http://127.0.0.1:1; "
     "echo status=$?; curl -b 'a=b' -c jar " + CURL_REFUSED + " " + CURL_ERR + "; "
     "cat jar; ls -R /tmp"),
    ("tool-layer: curl -w on a refused connection",
     "curl -s -w '%{bogus}[%{http_code}]%{exitcode}\\n' " + CURL_REFUSED + " 2>&1; "
     "echo status=$?; curl -w '%{nope}' " + CURL_REFUSED + " " + CURL_ERR + "; "
     "curl -s -k --fail-with-body -r 0-1 -A a -e r -u u:p -H 'X: y' "
     "-w '%{http_code}\\n' https://127.0.0.1:1/; echo status=$?"),
]

# wget against a port that refuses, or URLs it cannot use; its dated progress lines
# (`--2026-...--  URL`) are dropped.
WGET_ERR = "2>&1 | grep -v '^--'"
CASES += [
    ("tool-layer: wget URLs it cannot use",
     "for u in ftpx://x/ http://127.0.0.1:99999/ http:// '' 'http://h:x/'; do wget \"$u\" 2>&1; "
     "echo \"status=$?\"; done; wget -q ftpx://x/ 2>&1; echo status=$?; wget -q '' 2>&1; "
     "echo status=$?"),
    ("tool-layer: wget several URLs and their statuses",
     "wget -q ftpx://x/ http://127.0.0.1:1/; echo status=$?; wget -q http://127.0.0.1:1/ ftpx://x/; "
     "echo status=$?; wget -q ftpx://x/ ''; echo status=$?; "
     "wget http://127.0.0.1:1/ ftpx://x/ " + "2>&1 | grep -v '^--'; echo status=${PIPESTATUS[0]}"),
    ("tool-layer: wget -i and -nc",
     "cd /tmp; wget -i /tmp/nosuch 2>&1; echo status=$?; printf 'http://127.0.0.1:1/\\n\\n' > u; "
     "wget -i u " + WGET_ERR + "; echo status=${PIPESTATUS[0]}; : > u; wget -i u 2>&1; "
     "echo status=$?; touch index.html; wget -nc http://127.0.0.1:1/ 2>&1; echo status=$?; "
     "wget -q -nc http://127.0.0.1:1/ 2>&1; echo status=$?"),
    ("tool-layer: wget option errors",
     "wget -t -2 http://127.0.0.1:1/ 2>&1; echo status=$?; "
     "wget --max-redirect=-1 http://127.0.0.1:1/ 2>&1; echo status=$?; "
     "wget -q -t 3x --max-redirect=x --tries=inf http://127.0.0.1:1/ 2>&1; echo status=$?; "
     "echo x > /tmp/f; wget --post-data a --post-file /tmp/f http://127.0.0.1:1/ 2>&1; "
     "echo status=$?; wget --post-file /tmp/nosuch http://127.0.0.1:1/ " + WGET_ERR
     + "; echo status=${PIPESTATUS[0]}; wget -V >/dev/null; echo status=$?"),
    ("tool-layer: wget sets up its output before connecting",
     "cd /tmp; wget -O /tmp/nosuch/x http://127.0.0.1:1/ 2>&1; echo status=$?; "
     "wget -q -O out http://127.0.0.1:1/ http://127.0.0.1:1/; echo status=$?; "
     "wget -q -P dl http://127.0.0.1:1/; echo status=$?; ls -l out | cut -d' ' -f5; ls /tmp"),
    ("tool-layer: wget's HSTS store needs HOME",
     "wget ftpx://x/ 2>&1; HOME=/tmp wget ftpx://x/ 2>&1; export HOME=/tmp; wget ftpx://x/ 2>&1; "
     "HOME= wget ftpx://x/ 2>&1; ls -a /tmp"),
    ("tool-layer: wget refused and spider",
     "wget http://127.0.0.1:1/ " + WGET_ERR + "; echo status=${PIPESTATUS[0]}; "
     "wget --spider http://127.0.0.1:1/ " + WGET_ERR + "; echo status=${PIPESTATUS[0]}"),
]

# env's -S, -v, -a and signal options, as GNU env runs them.
CASES += [
    ("tool-layer: env -S splits as GNU env does",
     "X=hi env -S 'printf %s|%s|%s\\n ${X} a\\_b \"c\\_d\"'; env -S\"printf '%s|' 'a\\'b' "
     "'x\\\\y' \\\"q\\\\\\\"r\\\" \\\\t\"; echo; env -S'echo a#b #c' x; "
     "env -S'-i B=2 printenv'; env -S'echo x \\c y'; env -S '' true; echo st=$?"),
    ("tool-layer: env -S errors",
     "for s in 'echo $X' 'echo \"q' 'echo \\\\' 'echo \\q'; do env -S \"$s\"; echo st=$?; done 2>&1; "
     "env -S 2>&1; echo st=$?"),
    ("tool-layer: env -v traces each step",
     "{ env -i -v -u X -C / A=1 true; env -v -u HOME true; env -vS'echo \"a b\" c' x; } 2>&1"),
    ("tool-layer: env signal options",
     "{ env --ignore-signal=2 --list-signal-handling true; "
     "env --ignore-signal=INT,sigterm --block-signal=TERM,HUP --list-signal-handling true; "
     "env --default-signal=INT --list-signal-handling true; env --ignore-signal=KILL true; "
     "echo st=$?; env --ignore-signal=NOPE true; echo st=$?; env --ignore-signal=INT, true; "
     "echo st=$?; env --ignore-signal --list-signal-handling true 2>&1 | wc -l; "
     "trap '' INT; env --list-signal-handling true; } 2>&1"),
    ("tool-layer: env signal options reach the command",
     "env --ignore-signal=TERM bash -c 'trap -p TERM'; "
     "trap '' INT; env --default-signal=INT bash -c 'trap -p INT; echo [default]'; "
     "env bash -c 'trap -p INT'"),
    ("tool-layer: env -a names bash -c's $0",
     "env -a custom bash -c 'echo $0'; env --argv0=long bash -c 'echo $0'; "
     "env -a custom bash -c 'echo $0 $1' x y; env -a 2>&1; echo st=$?"),
    ("tool-layer: env diagnostics quote as GNU does",
     "env =x true; echo st=$?; env nosuch; echo st=$?; env -C /nosuch true; echo st=$?"),
]

PIPED_INPUT = (
    "bash-tool's limit: an embedded utility reads at most 64 MiB of piped input before it runs "
    "(it runs synchronously); reading up to that cut is an error rather than a wrong answer"
)
SORT_INPUT = (
    "bash-tool's limit: sort holds its input in memory on WASI (no threads for an external merge), "
    "so it refuses more than 64 MiB or 2 million lines"
)
NO_DEV_LISTING = (
    "the virtual /dev answers paths but cannot be listed (README: /dev and /dev/fd stat as "
    "directories that cannot be listed)"
)
NO_PROCESSES = (
    "refusal: WASI has no processes, so a program or script file cannot be executed; bash runs it "
    "when it is executable and says Permission denied when it is not"
)
PROGRAM_DIRS = (
    "bash-tool's commands answer at both /bin/<name> and /usr/bin/<name>, as a merged-/usr Linux "
    "system's do; the oracle (Alpine) has no /usr/bin/cat, /usr/bin/printenv or /usr/bin/sed and "
    "keeps bash in /usr/local/bin"
)
PROGRAM_NAMES = (
    "bash-tool reports every command at /bin/<name> (which, type, command -v, hash); the oracle "
    "installs jq in /usr/bin"
)
NO_PROGRAM_DIRS = (
    "bash-tool's commands run in-process and are not files: /bin/<name> runs one and -x holds "
    "for it, but /bin and /usr/bin are not directories, cannot be listed, and hold no files (-e)"
)
KILLED_PID = (
    "bash names the process KILL ended; process numbers here count from the call's own, so "
    "timeout's child is 2 where the oracle's is a system pid"
)
NESTED_SHELL_SIGNAL = (
    "limit: `sh` and `bash` here run inside timeout's child process rather than replacing it (no "
    "exec), so the signal that ends the child ends the nested shell with it: a TERM trap or "
    "ignored TERM in a nested shell does not run or hold; the call's own TERM trap does"
)
HOSTNAME_OPTIONS = (
    "the oracle's hostname is BusyBox's (Alpine), whose usage text differs from GNU's; this "
    "shell's hostname prints the name only and refuses options it does not implement"
)
EXPECTED = {
    "hostname: tool-layer error paths": (
        0, b"st=2\n", b"hostname: -x: unsupported in bash-tool\n", HOSTNAME_OPTIONS,
    ),
    "tool-layer: timeout that KILLs": (
        0, b"st=137\n",
        b"bash: line 1:     2 Killed                     timeout -s KILL 0.3 sleep 5\n",
        KILLED_PID,
    ),
    "tool-layer: timeout of a shell with its own TERM handling": (
        0, b"st=124\nst=124\n", b"", NESTED_SHELL_SIGNAL,
    ),
    "tool-layer: /usr/bin and /bin both hold every command": (
        0, b"a\n1\nst=0\nb\na\na\n", b"", PROGRAM_DIRS,
    ),
    "tool-layer: which, type -P and command -v name the commands": (
        0, b"/bin/jq\n/bin/jq\n/bin/jq\njq is /bin/jq\n/bin/jq\njq is hashed (/bin/jq)\n", b"",
        PROGRAM_NAMES,
    ),
    "tool-layer: the program directories are not directories": (
        0, b"nodir\nnox\nnofile\nnolist\nnocd\n", b"", NO_PROGRAM_DIRS,
    ),
    "tool-layer: listing /dev": (
        0, b"st=2\n", b"ls: cannot open directory '/dev': Permission denied\n", NO_DEV_LISTING,
    ),
    "tool-layer: a file as a command": (
        0, b"st=126\n", b"bash: line 1: /tmp/s.sh: executing files is unsupported in bash-tool\n",
        NO_PROCESSES,
    ),
    "tool-layer: piped input over the limit": (
        0, b"abc3977c2c709626b57927dc7388a9fc  -\nst=141 141 1\n",
        b"md5sum: standard input over 64 MiB is unsupported in bash-tool\n", PIPED_INPUT,
    ),
    "tool-layer: sort of a file over the limit": (
        0, b"st=2 0\n", b"sort: input over 64 MiB or 2000000 lines is unsupported in bash-tool\n",
        SORT_INPUT,
    ),
    "tool-layer: command substitution over the limit": (
        1, b"",
        b"bash: line 1: command substitution: output over 64 MiB is unsupported in bash-tool\n",
        BUFFERED,
    ),
    "tool-layer: process substitution read to the end": (
        0, b"67108864\nst=1 0\n",
        b"cat: /dev/fd/63: process substitution output over 64 MiB is unsupported in bash-tool\n",
        BUFFERED,
    ),
    "tool-layer: 400 nested braces": (
        2, b"", b"bash: shell code is nested too deeply for bash-tool\n", NESTING,
    ),
    "tool-layer: endless recursion ends loudly": (
        1, b"",
        b"bash: line 1: f: maximum function nesting level exceeded (150): deeper nesting is "
        b"unsupported in bash-tool\n",
        NESTING,
    ),
    # Surfaced by giving this case its first-ever recorded oracle golden (see the `stale` fix in
    # this same round -- it had none before, so `check` never actually compared it): GNU tac's
    # own message for reading a directory is "read error: Invalid argument" (EISDIR remapped);
    # uutils' tac here says "read error: Is a directory" instead, wording only -- every other
    # tool in this same case (uniq/cut/tail/jq/head/sort/od/sed/nl/grep) already matches exactly.
    # tac's own wording belongs to whoever owns the coreutils fork, not this round.
    "tool-layer: directory operands": (
        0,
        b"uniq: error reading '/tmp/d': Is a directory\nuniq st=1\ncut: /tmp/d: Is a directory\n"
        b"cut -c1 st=1\n==> /tmp/d <==\ntail: error reading '/tmp/d': Is a directory\n\n"
        b"==> /tmp/f <==\ntail -n1 st=1\nrev st=0\ntac: /tmp/d: read error: Is a directory\n"
        b"tac st=1\njq: error: Is a directory\njq . st=2\n==> /tmp/d <==\n"
        b"head: error reading '/tmp/d': Is a directory\n\n==> /tmp/f <==\nhead -c1 st=1\n"
        b"sort: read failed: /tmp/d: Is a directory\nsort st=2\nod: /tmp/d: Is a directory\n"
        b"od -c st=1\nsed: read error on /tmp/d: Is a directory\nsed p st=4\n"
        b"nl: /tmp/d: Is a directory\nnl st=1\ngrep: /tmp/d: Is a directory\ngrep x st=2\n",
        b"",
        "tac's directory-read error says \"Is a directory\" here, \"Invalid argument\" in GNU "
        "tac -- wording only, and the only line of this case that differs; not this round's "
        "corpus (coreutils fork), flagged for whoever owns tac",
    ),
}

EXPECTED_STDERR = {
    "tool-layer: closed standard input": (
        b"cat: -: Bad file descriptor\nnl: -: Bad file descriptor\n",
        "GNU utilities report a closed standard input a second time when they close it at exit "
        "(`cat: closing standard input: …`); bash-tool reports the failed read once",
    ),
}
