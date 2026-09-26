"""Real-world usage: tldr-pages examples for Bash's builtins and compound commands.

Source: tldr-pages (https://github.com/tldr-pages/tldr, CC BY 4.0), pages/common and pages/linux
at commit 8cf22035e60b167ebf224c1c527b27e0b5dca24d; see ../NOTICE. Each example runs with its
placeholders bound, then over the values it meets in practice (empty, unset, spaces, globs,
numbers that are not numbers, missing files). Examples that need a terminal, a login shell,
history, job control, other users, or print the whole environment are left out, as are the
builtins the tool refuses (umask, ulimit, `enable -n`, `exec -a/-c/-l`, bg, fg, fc, coproc).
"""

TIER = "sweep"

W = "mkdir -p /tmp/w && cd /tmp/w"
CASES = []


def add(name, slug, body, error=False):
    if name in BUILTIN_TAGS:
        tags = ("builtin." + name,)
    else:
        tags = {"[[": ("compound.cond",), "time": ()}.get(name, ("compound." + name,))
    tags = tags + (("error",) if error else ())
    CASES.append(("real tldr " + name + ": " + slug, body, tags))


BUILTIN_TAGS = set("""
. : [ alias break builtin caller cd command compgen continue declare dirs disown echo eval exec
exit export false getopts hash jobs kill let local mapfile popd pushd pwd read readarray readonly
return set shift shopt source test times trap true type typeset unalias unset wait
""".split())

# -- [ and [[ -----------------------------------------------------------------------------------
VALUES = [("a word", "word"), ("an empty string", ""), ("spaces", "a b"), ("a glob", "*"), ("a dash", "-n"), ("a number", "10")]
for _label, _v in VALUES:
    _set = "variable='" + _v + "'\n"
    add("[", "equal to a string with " + _label, _set + '[ "$variable" = "word" ]; echo "status=$?"')
    add("[", "not equal to a string with " + _label, _set + '[ "$variable" != "word" ]; echo "status=$?"')
    add("[", "non-empty with " + _label, _set + '[ -n "$variable" ]; echo "status=$?"')
    add("[", "empty with " + _label, _set + '[ -z "$variable" ]; echo "status=$?"')
    add("[[", "equal to a string or glob with " + _label, _set + '[[ $variable == "word" ]]; echo "status=$?"; [[ $variable == w* ]]; echo "status=$?"')
    add("[[", "non-empty with " + _label, _set + "[[ -n $variable ]]; echo \"status=$?\"")
    add("[[", "empty with " + _label, _set + "[[ -z $variable ]]; echo \"status=$?\"")
    add("[[", "regex with " + _label, _set + "[[ $variable =~ ^[a-z]+$ ]]; echo \"status=$? ${BASH_REMATCH[0]}\"")
add("[", "equal to a string with the variable unset", '[ "$variable" = "word" ]; echo "status=$?"')
add("[", "unquoted empty variable", 'variable=\n[ -n $variable ]; echo "status=$?"')
add("[", "unquoted variable with spaces", 'variable="a b"\n[ $variable = "a b" ]; echo "status=$?"', error=True)
for _op in ("eq", "ne", "gt", "lt", "ge", "le"):
    for _v in ("5", "10", "-3"):
        add("[", "-" + _op + " with " + _v, "variable=" + _v + '\n[ "$variable" -' + _op + ' 5 ]; echo "status=$?"')
        add("[[", "-" + _op + " with " + _v, "variable=" + _v + "\n[[ $variable -" + _op + " 5 ]]; echo \"status=$?\"")
for _v in ("abc", "", "1.5", " 7 ", "0x10", "010"):
    add("[", "-eq with a non-integer '" + _v + "'", "variable='" + _v + "'\n[ \"$variable\" -eq 5 ]; echo \"status=$?\"", error=True)
    add("[[", "-eq with a non-integer '" + _v + "'", "variable='" + _v + "'\n[[ $variable -eq 16 ]]; echo \"status=$?\"")
PATHS = W + "\nmkdir d && : >f && ln -s f l && ln -s nope dangling"
for _p in ("f", "d", "l", "dangling", "nope", "/dev/null", ""):
    add("[", "file tests on '" + _p + "'", PATHS + "\nfor t in -f -d -e -L -r -w; do [ $t '" + _p + "' ] && printf '%s ' \"$t\"; done; echo")
    add("[[", "file tests on '" + _p + "'", PATHS + "\n[[ -f '" + _p + "' ]] && echo f; [[ -d '" + _p + "' ]] && echo d; [[ -e '" + _p + "' ]] && echo e; echo end")
add("[", "missing closing bracket", '[ 1 = 1; echo "status=$?"', error=True)
add("[", "too many arguments", '[ a b c ]; echo "status=$?"', error=True)
add("[", "unknown binary operator", '[ a -foo b ]; echo "status=$?"', error=True)
add("[[", "combined conditions", 'x=5\n[[ $x -gt 1 && ( $x -lt 3 || $x == 5 ) ]] && echo yes')
add("[[", "string ordering", "[[ apple < banana ]] && echo lt; [[ b > a ]] && echo gt")
add("[[", "regex with a capture group", 's=v1.22.3\n[[ $s =~ ^v([0-9]+)\\.([0-9]+) ]] && echo "${BASH_REMATCH[1]} ${BASH_REMATCH[2]}"')
add("[[", "regex quoted literally", 's="a.b"\n[[ $s =~ "a.b" ]] && echo lit; [[ axb =~ "a.b" ]] || echo no')
add("[[", "invalid regex", '[[ x =~ ( ]]; echo "status=$?"', error=True)

# -- alias / unalias ------------------------------------------------------------------------------
AL = "shopt -s expand_aliases\n"
add("alias", "list all aliases", AL + "alias ll='ls -1'\nalias gs='echo status'\nalias")
add("alias", "list with none defined", "alias; echo \"status=$?\"")
add("alias", "create a generic alias", AL + "alias greet=\"echo hello\"\ngreet world")
add("alias", "create without expand_aliases", "alias greet=\"echo hello\"\ngreet world; echo \"status=$?\"")
add("alias", "view an alias", AL + "alias greet=\"echo hello\"\nalias greet")
add("alias", "view a missing alias", "alias nosuch; echo \"status=$?\"", error=True)
add("alias", "remove an aliased command", AL + "alias greet=\"echo hello\"\nunalias greet\nalias greet; echo \"status=$?\"")
add("alias", "turn rm into an interactive command", AL + W + "\n: >f\nalias rm=\"rm --interactive\"\necho n | rm f; ls")
add("alias", "shortcut for ls --all", AL + W + "\n: >.hidden\nalias la=\"ls --all\"\nla")
add("alias", "alias with a trailing space", AL + "alias run='echo ' word='expanded'\nrun word")
add("alias", "alias with quotes in its value", AL + "alias q=\"echo 'single' \\\"double\\\"\"\nalias q\nq")
add("alias", "alias defined on the same line", AL + "alias now='echo now'; now; echo \"status=$?\"")
add("unalias", "remove an alias", AL + "alias a1='echo 1'\nunalias a1\nalias")
add("unalias", "remove all aliases", AL + "alias a1='echo 1' a2='echo 2'\nunalias -a\nalias; echo \"status=$?\"")
add("unalias", "remove a missing alias", "unalias nosuch; echo \"status=$?\"", error=True)

# -- break / continue -----------------------------------------------------------------------------
add("break", "out of a single loop", "while :; do echo once; break; done; echo after")
add("break", "out of nested loops", "while :; do while :; do echo inner; break 2; done; echo not-reached; done; echo after")
add("break", "out of a for loop by count", "for i in 1 2 3 4; do [ $i = 3 ] && break; echo $i; done")
add("break", "with a count larger than the nesting", "for i in 1 2; do break 5; done; echo \"status=$?\"")
add("break", "outside a loop", "break; echo \"status=$?\"")
add("break", "with a bad count", "for i in 1; do break 0; done; echo \"status=$?\"", error=True)
add("continue", "skip to the next iteration", "i=0; while [ $i -lt 3 ]; do i=$((i+1)); continue; echo \"This will never be reached\"; done; echo $i")
add("continue", "from within a nested loop", "for i in {1..3}; do echo $i; while :; do continue 2; done; done")
add("continue", "outside a loop", "continue; echo \"status=$?\"")
add("continue", "in an until loop", "n=0; until [ $n -ge 4 ]; do n=$((n+1)); [ $((n % 2)) -eq 0 ] && continue; echo odd $n; done")

# -- builtin / caller / command -------------------------------------------------------------------
add("builtin", "run a shell builtin", "builtin echo hello")
add("builtin", "bypass a function", "echo() { printf 'fn:%s\\n' \"$*\"; }\necho x\nbuiltin echo y")
add("builtin", "a non-builtin", "builtin nosuch; echo \"status=$?\"", error=True)
add("builtin", "cd through builtin", "builtin cd /tmp && pwd")
add("caller", "line and file of the caller", "f() { caller; }\nf")
add("caller", "line function and file", "g() { caller 0; }\nh() { g; }\nh")
add("caller", "n frames back", "a() { caller 1; }\nb() { a; }\nc() { b; }\nc")
add("caller", "frames beyond the stack", "a() { caller 5; echo \"status=$?\"; }\na")
add("caller", "outside a function", "caller; echo \"status=$?\"")
add("command", "run a command literally despite an alias", "shopt -s expand_aliases\nalias ls='echo aliased'\nmkdir -p /tmp/w && cd /tmp/w && : >f\nls\ncommand ls")
add("command", "run a command despite a function", "pwd() { echo function; }\npwd\ncd /tmp && command pwd")
add("command", "default path", "command -p echo hello")
add("command", "path of a builtin", "command -v echo")
add("command", "path of a function", "f() { :; }\ncommand -v f")
add("command", "path of an alias", "alias ll='ls -1'\ncommand -v ll")
add("command", "path of a keyword", "command -v if")
add("command", "path of a missing command", "command -v nosuchcmd; echo \"status=$?\"")
add("command", "verbose description", "f() { :; }\ncommand -V f | head -n 1; command -V echo")
add("command", "missing command", "command nosuchcmd; echo \"status=$?\"", error=True)

# -- compgen --------------------------------------------------------------------------------------
add("compgen", "match against a wordlist", 'compgen -W "apple orange banana" a')
add("compgen", "match against a wordlist with no prefix", 'compgen -W "apple orange banana"')
add("compgen", "match against a wordlist with no match", 'compgen -W "apple orange banana" z; echo "status=$?"')
add("compgen", "wordlist with expansions", 'fruit=kiwi\ncompgen -W "$fruit key \\$HOME" k')
add("compgen", "into a variable", 'compgen -V COMPREPLY -W "alpha beta album" al; echo "${COMPREPLY[@]}"')
add("compgen", "list aliases", "alias zz1='echo 1' zz2='echo 2'\ncompgen -a")
add("compgen", "list functions", "zfa() { :; }\nzfb() { :; }\ncompgen -A function")
add("compgen", "list functions starting with a string", "zfa() { :; }\nzfb() { :; }\nother() { :; }\ncompgen -A function zf")
add("compgen", "keywords", "compgen -k")
add("compgen", "aliases starting with a string", "alias lsx='ls' lsy='ls' other='ls'\ncompgen -a ls")
add("compgen", "variables with a prefix", "ZZVAR_A=1 ZZVAR_B=2\ncompgen -v ZZVAR_")
add("compgen", "files", "mkdir -p /tmp/w && cd /tmp/w && : >alpha && : >beta && mkdir adir\ncompgen -f a | sort")
add("compgen", "directories", "mkdir -p /tmp/w && cd /tmp/w && : >alpha && mkdir adir bdir\ncompgen -d | sort")

# -- cd / pwd / dirs / pushd / popd ---------------------------------------------------------------
TREE = "mkdir -p /tmp/w/a/b /tmp/w/c 'tmp/w/sp ace' && cd /tmp/w\n"
add("cd", "go to a directory", TREE + "cd a/b && pwd")
add("cd", "go to a missing directory", TREE + "cd nosuch; echo \"status=$?\"; pwd", error=True)
add("cd", "go to a file", TREE + ": >f && cd f; echo \"status=$?\"", error=True)
add("cd", "go to a directory with a space", "mkdir -p '/tmp/w/sp ace' && cd '/tmp/w/sp ace' && pwd")
add("cd", "go up to the parent", TREE + "cd a/b && cd .. && pwd")
add("cd", "go up beyond the root", "cd / && cd .. && pwd")
add("cd", "go to the home directory", TREE + "cd; echo \"status=$?\"; pwd", error=True)
add("cd", "go to the home directory with HOME set", TREE + "HOME=/tmp/w/c\ncd && pwd")
add("cd", "home directory of a user", TREE + "cd ~nosuchuser; echo \"status=$?\"", error=True)
add("cd", "the previous directory", TREE + "cd a && cd /tmp && cd - && pwd")
add("cd", "the previous directory with none", "cd -; echo \"status=$?\"", error=True)
add("cd", "the root directory", TREE + "cd / && pwd")
add("cd", "cdpath", TREE + "CDPATH=/tmp/w/a\ncd b && pwd")
add("cd", "physical through a link", TREE + "ln -s a/b lnk && cd -P lnk && pwd && cd /tmp/w && cd -L lnk && pwd")
add("cd", "too many arguments", TREE + "cd a c; echo \"status=$?\"", error=True)
add("cd", "OLDPWD and PWD", TREE + "cd a && echo \"$OLDPWD -> $PWD\"")
add("pwd", "print the current directory", "cd /tmp && pwd")
add("pwd", "physical", TREE + "ln -s a/b lnk && cd lnk && pwd && pwd -P")
add("pwd", "logical", TREE + "ln -s a/b lnk && cd lnk && pwd -L")
add("pwd", "after the directory is removed", TREE + "cd c && rmdir /tmp/w/c && pwd; echo \"status=$?\"")
add("dirs", "the stack with spaces", TREE + "pushd a >/dev/null && pushd /tmp/w/c >/dev/null && dirs")
add("dirs", "one entry per line", TREE + "pushd a >/dev/null && pushd /tmp/w/c >/dev/null && dirs -p")
add("dirs", "numbered", TREE + "pushd a >/dev/null && pushd /tmp/w/c >/dev/null && dirs -v")
add("dirs", "without tilde", TREE + "HOME=/tmp/w\npushd a >/dev/null && dirs && dirs -l")
add("dirs", "nth entry", TREE + "pushd a >/dev/null && pushd /tmp/w/c >/dev/null && dirs +1")
add("dirs", "nth entry from the last", TREE + "pushd a >/dev/null && pushd /tmp/w/c >/dev/null && dirs -0")
add("dirs", "entry out of range", TREE + "dirs +5; echo \"status=$?\"", error=True)
add("dirs", "clear the stack", TREE + "pushd a >/dev/null && dirs -c && dirs")
add("pushd", "switch and push", TREE + "pushd a && pwd")
add("pushd", "switch the top two", TREE + "pushd a >/dev/null && pushd && pwd")
add("pushd", "switch with an empty stack", TREE + "pushd; echo \"status=$?\"", error=True)
add("pushd", "rotate the stack", TREE + "pushd a >/dev/null; pushd /tmp/w/c >/dev/null; pushd /tmp/w/a/b >/dev/null; pushd +2; pwd")
add("pushd", "rotate out of range", TREE + "pushd a >/dev/null; pushd +4; echo \"status=$?\"", error=True)
add("pushd", "add without changing directory", TREE + "pushd -n /tmp/w/c; pwd")
add("pushd", "a missing directory", TREE + "pushd nosuch; echo \"status=$?\"", error=True)
add("popd", "remove the top and cd", TREE + "pushd a >/dev/null && pushd /tmp/w/c >/dev/null && popd && pwd")
add("popd", "remove the nth from the left", TREE + "pushd a >/dev/null; pushd /tmp/w/c >/dev/null; popd +1; pwd")
add("popd", "remove the nth from the right", TREE + "pushd a >/dev/null; pushd /tmp/w/c >/dev/null; popd -0; pwd")
add("popd", "remove without changing directory", TREE + "pushd a >/dev/null; pushd /tmp/w/c >/dev/null; popd -n; pwd")
add("popd", "empty stack", "popd; echo \"status=$?\"", error=True)

# -- declare / typeset / local / readonly / export / unset -----------------------------------------
add("declare", "string variable", 'declare variable="value"; declare -p variable')
add("declare", "integer variable", 'declare -i variable="4 + 5"; echo $variable; variable=abc; echo $variable; variable+=3; echo $variable')
add("declare", "integer variable with a bad expression", 'declare -i variable="4 +"; echo "status=$?"', error=True)
add("declare", "array variable", "declare -a variable=(item_a item_b 'item c'); declare -p variable; echo ${#variable[@]}")
add("declare", "associative array", "declare -A variable=([key_a]=item_a [key_b]=item_b [key_c]=item_c); for k in \"${!variable[@]}\"; do echo \"$k=${variable[$k]}\"; done | sort")
add("declare", "associative array printed", "declare -A variable=([k]=v); declare -p variable")
add("declare", "readonly string", 'declare -r variable="value"; variable=other; echo "status=$? $variable"', error=True)
add("declare", "global inside a function", 'f() { declare -g variable="value"; declare local_one=x; }; f; echo "[$variable] [$local_one]"')
add("declare", "print a function", "function_name() { echo hi; local x=1; }; declare -f function_name")
add("declare", "print a missing function", "declare -f nosuch; echo \"status=$?\"")
add("declare", "print a variable", "variable_name='a \"b\" $c'; declare -p variable_name")
add("declare", "print a missing variable", "declare -p nosuch; echo \"status=$?\"", error=True)
add("declare", "lower and upper case attributes", "declare -l lo=MiXeD; declare -u up=MiXeD; echo $lo $up")
add("declare", "nameref", "target=1; declare -n ref=target; ref=2; echo $target")
add("declare", "export attribute", "declare -x EXPORTED=yes; bash -c 'echo $EXPORTED'")
add("declare", "function names only", "zz_a() { :; }; zz_b() { :; }; declare -F | grep zz_")
add("typeset", "same as declare", "typeset -i n=2*3; typeset -p n")
add("local", "string variable", 'f() { local variable="value"; echo "$variable"; }; f; echo "[${variable-unset}]"')
add("local", "integer variable", 'f() { local -i variable="2 * 3"; echo $variable; }; f')
add("local", "array variable", "f() { local variable=(item_a item_b item_c); echo ${variable[1]} ${#variable[@]}; }; f")
add("local", "associative array", "f() { local -A variable=([key_a]=item_a [key_b]=item_b); echo ${variable[key_b]}; }; f")
add("local", "readonly variable", 'f() { local -r variable="value"; variable=x; echo "status=$?"; }; f', error=True)
add("local", "outside a function", 'local variable="value"; echo "status=$?"', error=True)
add("local", "shadows a global", 'v=global; f() { local v=local; g; }; g() { echo $v; }; f; echo $v')
add("readonly", "set a read-only variable", "readonly variable_name=value; echo $variable_name; variable_name=x; echo \"status=$?\"", error=True)
add("readonly", "mark an existing variable", "existing_variable=1; readonly existing_variable; unset existing_variable; echo \"status=$? $existing_variable\"", error=True)
add("readonly", "print read-only variables", "readonly zz_ro=1; readonly -p | grep zz_ro")
add("readonly", "assignment in a subshell does not leak", "readonly r=1; (r=2) 2>/dev/null; echo \"status=$? $r\"")
add("export", "set an environment variable", "export VARIABLE=value; bash -c 'echo $VARIABLE'")
add("export", "append to PATH", "export PATH=$PATH:/tmp/w/bin; case $PATH in *:/tmp/w/bin) echo appended;; esac")
add("export", "unset an environment variable", "export VARIABLE=value; export -n VARIABLE; bash -c 'echo \"[${VARIABLE-unset}]\"'; echo $VARIABLE")
add("export", "export a function", "FUNCTION_NAME() { echo \"in child: $1\"; }; export -f FUNCTION_NAME; bash -c 'FUNCTION_NAME arg'")
add("export", "print exported variables", "export ZZ_EXP='a b'; export -p | grep ZZ_EXP")
add("export", "an invalid name", "export 1abc=x; echo \"status=$?\"", error=True)
add("unset", "remove a variable", "variable=1; unset variable; echo \"[${variable-unset}]\"")
add("unset", "remove a function of the same name", "f() { echo fn; }; unset f; f; echo \"status=$?\"")
add("unset", "remove variables", "v1=1 v2=2; unset -v v1 v2; echo \"[${v1-u}${v2-u}]\"")
add("unset", "remove functions", "f1() { :; }; f2() { :; }; unset -f f1 f2; declare -F f1; echo \"status=$?\"")
add("unset", "remove an array element", "a=(x y z); unset 'a[1]'; echo \"${a[@]} ${#a[@]} ${!a[@]}\"")
add("unset", "remove a readonly variable", "readonly r=1; unset r; echo \"status=$?\"", error=True)

# -- echo -----------------------------------------------------------------------------------------
add("echo", "text message", 'echo "Hello World"')
add("echo", "text message without quotes", "echo Hello    World")
add("echo", "message with a variable", 'MYVAR=/tmp/w; echo "My path is $MYVAR"')
add("echo", "without a trailing newline", 'echo -n "Hello World"; echo "|"')
add("echo", "append to a file", 'mkdir -p /tmp/w && cd /tmp/w && echo "Hello World" >> file.txt && echo "Again" >> file.txt && cat file.txt')
add("echo", "backslash escapes", 'echo -e "Column 1\\tColumn 2"')
for _esc in ("\\n", "\\a\\b", "\\c stop", "\\x41\\x4a", "\\0101", "\\u00e9", "\\e[0m", "\\\\", "\\q"):
    add("echo", "backslash escape " + _esc, "echo -e 'x" + _esc + "y' | od -c")
add("echo", "backslash escapes disabled", "echo -E 'a\\tb'")
add("echo", "exit status of the last command", "false; echo $?; true; echo $?")
add("echo", "pass text to another program", 'echo "Hello World" | tr a-z A-Z')
add("echo", "options that are not options", "echo -x -n- --; echo -- -n")
add("echo", "xpg_echo", "shopt -s xpg_echo; echo 'a\\tb'")

# -- eval / exec / exit / false / true ------------------------------------------------------------
add("eval", "call echo", 'eval "echo foo"')
add("eval", "set a variable", 'eval "foo=bar"; echo $foo')
add("eval", "indirect variable name", 'name=target; eval "$name=42"; echo $target')
add("eval", "a syntax error", 'eval "if then"; echo "status=$?"', error=True)
add("eval", "concatenated arguments", "eval echo '$((1+2))' '\"a  b\"'")
add("eval", "exit status", "eval false; echo \"status=$?\"; eval; echo \"status=$?\"")
add("exec", "a command", "exec echo replaced; echo not-reached")
add("exec", "a command that fails", "exec false; echo not-reached")
add("exec", "a missing command", "exec nosuchcmd; echo not-reached", error=True)
add("exec", "redirection only", "mkdir -p /tmp/w && cd /tmp/w\nexec 3>out.txt\necho to-three >&3\nexec 3>&-\ncat out.txt")
add("exec", "in a subshell", "(exec echo in-subshell); echo after")
add("exit", "with the last status", "false; exit")
add("exit", "with the last status of a subshell", "(false; exit); echo \"status=$?\"")
add("exit", "with a specific status", "exit 42")
add("exit", "with a status above 255", "(exit 300); echo \"status=$?\"")
add("exit", "with a negative status", "(exit -1); echo \"status=$?\"")
add("exit", "with a non-numeric status", "(exit abc); echo \"status=$?\"", error=True)
add("exit", "with too many arguments", "(exit 1 2); echo \"status=$?\"", error=True)
add("exit", "runs the EXIT trap", "trap 'echo trapped' EXIT; exit 3")
add("false", "non-zero exit code", "false; echo \"status=$?\"")
add("false", "make a command always exit with 1", "echo ran && false; echo \"status=$?\"")
add("false", "ignores arguments", "false --help; echo \"status=$?\"")
add("true", "successful exit code", "true; echo \"status=$?\"")
add("true", "make a command always exit with 0", "ls /nonexistent 2>/dev/null || true; echo \"status=$?\"")
add("true", "under set -e", "set -e; false || true; echo survived")

# -- getopts --------------------------------------------------------------------------------------
add("getopts", "first set option", 'set -- -x; getopts x opt; echo $opt')
add("getopts", "first set option when absent", 'set -- file; getopts x opt; echo "status=$? [$opt]"')
add("getopts", "option in a string", 'getopts x opt "-x text"; echo $opt; getopts x opt2 -x; echo $opt2')
add("getopts", "option with an argument", 'set -- -x value; getopts x: opt; echo $opt $OPTARG')
add("getopts", "option with an attached argument", 'set -- -xvalue; getopts x: opt; echo $opt $OPTARG')
add("getopts", "option missing its argument", 'set -- -x; getopts x: opt; echo "[$opt] [${OPTARG-unset}]"')
add("getopts", "multiple options", "set -- -x -y -z; while getopts xyz opt; do case $opt in x) echo x is set;; y) echo y is set;; z) echo z is set;; esac; done")
add("getopts", "grouped options", "set -- -zyx rest; while getopts xyz opt; do echo $opt; done; shift $((OPTIND-1)); echo \"rest: $*\"")
add("getopts", "unknown option", "set -- -q; while getopts xyz opt; do echo \"[$opt]\"; done", error=True)
add("getopts", "silent mode handling errors", "set -- -x; while getopts :x: opt; do case $opt in x) ;; :) echo \"Argument required\";; \\?) echo \"Invalid argument\";; esac; done")
add("getopts", "silent mode with an invalid option", "set -- -q; while getopts :x: opt; do case $opt in x) ;; :) echo \"Argument required\";; \\?) echo \"Invalid argument $OPTARG\";; esac; done")
add("getopts", "reset", "set -- -a; getopts a o; echo $o $OPTIND; OPTIND=1; set -- -b; getopts b o; echo $o $OPTIND")
add("getopts", "stops at a double dash", "set -- -a -- -b; while getopts ab o; do echo $o; done; echo $OPTIND")
add("getopts", "inside a function", "f() { local OPTIND o; while getopts n: o; do echo \"$o=$OPTARG\"; done; }; f -n 1; f -n 2")

# -- hash / type ----------------------------------------------------------------------------------
add("hash", "view the table when empty", "hash; echo \"status=$?\"")
add("hash", "clear the table", "hash -r; echo \"status=$?\"")
add("hash", "delete a missing command", "hash -d nosuchcmd; echo \"status=$?\"", error=True)
add("hash", "a missing command", "hash nosuchcmd; echo \"status=$?\"", error=True)
add("type", "a builtin", "type echo")
add("type", "a keyword", "type if")
add("type", "a function", "f() { echo hi; }; type f")
add("type", "an alias", "alias ll='ls -1'; type ll")
add("type", "a missing command", "type nosuchcmd; echo \"status=$?\"", error=True)
add("type", "all locations of a builtin", "type -a echo")
add("type", "all locations of a function shadowing a builtin", "echo() { :; }; type -a echo")
add("type", "file name of a builtin", "type -p echo; echo \"status=$?\"")
add("type", "type name of each kind", "f() { :; }; alias al=x; type -t f al if echo nosuch; echo \"status=$?\"")

# -- jobs / kill / wait / disown ------------------------------------------------------------------
add("jobs", "status of all jobs", "sleep 0.3 & sleep 0.3 & jobs; wait")
add("jobs", "status of a particular job", "sleep 0.3 & sleep 0.3 & jobs %2; wait")
add("jobs", "status of a missing job", "jobs %3; echo \"status=$?\"", error=True)
add("jobs", "no jobs", "jobs; echo \"status=$?\"")
add("jobs", "process ids", "sleep 0.3 & p=$!; [ \"$(jobs -p)\" = \"$p\" ] && echo matches; wait")
add("jobs", "long listing shape", "sleep 5 & jobs -l | sed 's/[0-9][0-9]*/N/g; s/  */ /g'; kill %1; wait 2>/dev/null")
add("jobs", "running only", "sleep 0.3 & jobs -r; wait")
add("jobs", "stopped only", "sleep 0.3 & jobs -s; echo \"status=$?\"; wait")
add("jobs", "changed status only", "sleep 0.3 & jobs -n >/dev/null; echo \"status=$?\"; wait")
add("jobs", "after completion", "(exit 3) & wait; jobs; echo \"status=$?\"")
add("kill", "terminate with SIGTERM", "sleep 5 & p=$!; kill $p; wait $p; echo \"status=$?\"")
add("kill", "list signal names", "kill -l")
add("kill", "list the table", "kill -L | head -n 3")
add("kill", "name of a signal number", "kill -l 15; kill -l 9; kill -l 143")
add("kill", "number of a signal name", "kill -l TERM; kill -l SIGHUP")
for _sig in ("-1", "-HUP", "-2", "-INT", "-9", "-KILL", "-s TERM", "-SIGUSR1", "-10", "-0"):
    add("kill", "signal " + _sig, "sleep 5 & p=$!; kill " + _sig + " $p; echo \"kill=$?\"; wait $p 2>/tmp/err; echo \"status=$?\"; sed -E 's/:  *[0-9]+ /: PID /' /tmp/err")
add("kill", "a background job by number", "sleep 5 & kill %1; wait; echo \"status=$?\"")
add("kill", "a missing job", "kill %4; echo \"status=$?\"", error=True)
add("kill", "a bad signal", "sleep 5 & kill -BOGUS $!; echo \"status=$?\"; kill $!", error=True)
add("kill", "no arguments", "kill; echo \"status=$?\"", error=True)
add("kill", "a trapped signal in a job", "(trap 'echo caught; exit 7' TERM; sleep 5 & wait) & p=$!; sleep 0.2; kill $p; wait $p; echo \"status=$?\"")
add("wait", "a process id", "(sleep 0.1; exit 4) & wait $!; echo \"status=$?\"")
add("wait", "all processes", "(sleep 0.1; echo one) & (sleep 0.05; echo two) & wait; echo \"status=$?\"")
add("wait", "a job number", "(exit 5) & wait %1; echo \"status=$?\"")
add("wait", "a missing process", "wait 99999; echo \"status=$?\"", error=True)
add("wait", "any job", "(sleep 0.3; exit 1) & (exit 2) & wait -n; echo \"status=$?\"; wait")
add("wait", "a pid that is not a child", "wait $$ 2>/tmp/err; echo \"status=$?\"; sed \"s/pid $$ /pid PID /\" /tmp/err", error=True)
add("disown", "the current job", "sleep 0.2 & disown; jobs; echo \"status=$?\"")
add("disown", "a specific job", "sleep 0.2 & sleep 0.2 & disown %1; jobs | wc -l; wait")
add("disown", "all jobs", "sleep 0.2 & sleep 0.2 & disown -a; jobs; echo done")
add("disown", "mark a job to keep", "sleep 0.2 & disown -h %1; jobs | wc -l; wait")
add("disown", "a missing job", "disown %3; echo \"status=$?\"", error=True)

# -- let ------------------------------------------------------------------------------------------
add("let", "simple expression", 'a=2 b=3; let "result = a + b"; echo $result')
add("let", "post-increment", 'x=5; let "x++"; echo $x; let "y = x++"; echo $x $y')
add("let", "conditional operator", 'x=12; let "result = (x > 10) ? x : 0"; echo $result; x=3; let "result = (x > 10) ? x : 0"; echo $result')
add("let", "status when zero", 'let "z = 0"; echo "status=$?"; let "z = 1"; echo "status=$?"')
add("let", "several expressions", "let a=1 b=a+1 c=b*10; echo $a $b $c")
add("let", "division by zero", 'let "d = 1 / 0"; echo "status=$?"', error=True)
add("let", "syntax error", 'let "1 +"; echo "status=$?"', error=True)
add("let", "bases and bitwise operators", 'let "v = 16#ff & 0x0f | 2#100 ^ 010"; echo $v; let "s = 1 << 4 >> 1"; echo $s')

# -- read / readarray / mapfile -------------------------------------------------------------------
for _label, _in in (("a line", "hello world"), ("an empty line", ""), ("backslashes", "a\\\\b c\\\\"), ("leading spaces", "   x  y  "), ("no newline", None)):
    _src = ("printf '%s' 'tail'" if _in is None else "printf '%s\\n' '" + _in + "'")
    add("read", "store a line with " + _label, _src + " | { read variable; echo \"status=$? [$variable]\"; }")
    add("read", "raw with " + _label, _src + " | { read -r variable; echo \"[$variable]\"; }")
    add("read", "into an array with " + _label, _src + " | { read -a array; echo \"${#array[@]} [${array[*]}]\"; }")
add("read", "number of characters", "echo abcdef | { read -n 3 variable; echo \"[$variable]\"; }")
add("read", "number of characters past a newline", "printf 'ab\\ncd\\n' | { read -n 3 variable; echo \"[$variable]\"; }")
add("read", "exact number of characters", "printf 'ab\\ncd\\n' | { read -N 4 variable; echo \"[$variable]\" | od -c | head -n 1; }")
add("read", "multiple variables from a here string", 'read <<< "The surname is Bond" _ variable1 _ variable2; echo "$variable1 $variable2"')
add("read", "more words than variables", 'read a b <<< "one two three four"; echo "[$a] [$b]"')
add("read", "fewer words than variables", 'read a b c <<< "one"; echo "[$a] [$b] [$c]"')
add("read", "prompt without a terminal", 'read -p "Enter your input here: " variable <<< "typed"; echo "[$variable]"')
add("read", "silent mode", 'read -s variable <<< "secret"; echo "[$variable]"')
add("read", "each line of output", "printf 'a b\\n c\\n\\nd' | while IFS= read -r line; do echo \"<$line>\"; done")
add("read", "each line with the last line lacking a newline", "printf 'a\\nb' | while IFS= read -r line || [ -n \"$line\" ]; do echo \"<$line>\"; done")
add("read", "custom delimiter", "printf 'a:b:c' | { read -d : x; echo \"[$x]\"; }")
add("read", "IFS splitting", "IFS=, read -r a b <<< 'x, y,z'; echo \"[$a] [$b]\"")
add("read", "end of input", "read v </dev/null; echo \"status=$? [$v]\"")
add("read", "timeout on available input", "read -t 1 v <<< 'fast'; echo \"status=$? [$v]\"")
add("read", "default REPLY", "read <<< ' spaced '; echo \"[$REPLY]\"")
add("read", "invalid option", "read -z v <<< x; echo \"status=$?\"", error=True)
RA = "mkdir -p /tmp/w && cd /tmp/w && printf 'one\\ntwo\\nthree\\nfour\\n' >file.txt\n"
add("readarray", "lines from a file", RA + "readarray array_name < file.txt; declare -p array_name")
add("readarray", "remove trailing delimiters", RA + "readarray < file.txt -t array_name; declare -p array_name")
add("readarray", "at most n lines", RA + "readarray < file.txt -n 2 -t array_name; declare -p array_name")
add("readarray", "skip n lines", RA + "readarray < file.txt -s 1 -t array_name; declare -p array_name")
add("readarray", "custom delimiter", "mkdir -p /tmp/w && cd /tmp/w && printf 'a,b,c' >file.txt\nreadarray < file.txt -d , array_name; declare -p array_name")
add("readarray", "custom delimiter with -t", "mkdir -p /tmp/w && cd /tmp/w && printf 'a,b,,c,' >file.txt\nreadarray < file.txt -d , -t array_name; declare -p array_name")
add("readarray", "empty file", "mkdir -p /tmp/w && cd /tmp/w && : >file.txt\nreadarray -t array_name < file.txt; echo ${#array_name[@]}")
add("readarray", "missing file", "readarray -t array_name < /tmp/nosuch.txt; echo \"status=$?\"", error=True)
add("readarray", "from a pipe with lastpipe", RA + "shopt -s lastpipe\ncat file.txt | readarray -t arr; echo ${#arr[@]}")
add("readarray", "origin index", RA + "arr=(x); readarray -t -O 1 arr < file.txt; declare -p arr")
add("mapfile", "lines from a process substitution", "mapfile -t lines < <(printf 'a\\nb\\n'); declare -p lines")
add("mapfile", "default MAPFILE", "mapfile <<< $'x\\ny'; echo ${#MAPFILE[@]}")
add("mapfile", "callback every line", "mapfile -t -C 'echo cb' -c 1 arr <<< $'x\\ny'")

# -- return / shift / set / shopt / source --------------------------------------------------------
add("return", "exit a function prematurely", 'func_name() { echo "This is reached"; return; echo "This is not"; }; func_name; echo "status=$?"')
add("return", "a return value", "func_name() { return 3; }; func_name; echo \"status=$?\"")
add("return", "a return value above 255", "func_name() { return 257; }; func_name; echo \"status=$?\"")
add("return", "a non-numeric value", "func_name() { return abc; }; func_name; echo \"status=$?\"", error=True)
add("return", "outside a function", "return; echo \"status=$?\"", error=True)
add("return", "from a sourced file", "mkdir -p /tmp/w && cd /tmp/w && printf 'echo in\\nreturn 5\\necho not\\n' >lib.sh\n. ./lib.sh; echo \"status=$?\"")
add("return", "the last status by default", "f() { false; return; }; f; echo \"status=$?\"")
add("shift", "remove the first parameter", "set -- a b c; shift; echo \"$@\"")
add("shift", "remove n parameters", "set -- a b c d; shift 2; echo \"$@ $#\"")
add("shift", "more than there are", "set -- a b; shift 3; echo \"status=$? $#\"")
add("shift", "negative count", "set -- a; shift -1; echo \"status=$?\"", error=True)
add("shift", "in a function", "f() { shift; echo \"$*\"; }; f x y z")
add("set", "export new variables", "set -a; NEWVAR=exported; bash -c 'echo $NEWVAR'")
add("set", "notify of finished jobs", "set -b; echo \"$-\" | grep -q b && echo notify-on")
add("set", "vi mode", "set -o vi; set -o | grep -E '^(vi|emacs) '")
add("set", "emacs mode", "set -o emacs; set -o | grep -E '^(vi|emacs) '")
add("set", "list all modes", "set -o")
add("set", "list all modes as commands", "set +o")
add("set", "exit when a command fails", "set -e; echo one; false; echo two")
add("set", "errexit in a condition", "set -e; if false; then :; fi; false || echo handled; echo survived")
add("set", "errexit in a function called from a condition", "set -e; f() { false; echo after-false; }; f && echo ok")
add("set", "reset positional parameters", "set -- argument1 'argument 2' argument3; echo $#; printf '<%s>' \"$@\"; echo")
add("set", "reset to nothing", "set -- a b; set --; echo $#")
add("set", "unknown option", "set -q; echo \"status=$?\"", error=True)
add("set", "unknown long option", "set -o nosuchoption; echo \"status=$?\"", error=True)
add("set", "the current flags", "set -u; set -f; echo $-")
add("shopt", "list all options", "shopt")
add("shopt", "set an option", "shopt -s nullglob; mkdir -p /tmp/w && cd /tmp/w && echo [*.nope]")
add("shopt", "unset an option", "shopt -s nullglob; shopt -u nullglob; mkdir -p /tmp/w && cd /tmp/w && echo [*.nope]")
add("shopt", "print as runnable commands", "shopt -p")
add("shopt", "print one option", "shopt -p extglob; shopt extglob; echo \"status=$?\"")
add("shopt", "query quietly", "shopt -q extglob; echo \"status=$?\"; shopt -s extglob; shopt -q extglob; echo \"status=$?\"")
add("shopt", "an invalid option", "shopt -s nosuchopt; echo \"status=$?\"", error=True)
add("shopt", "set -o options", "shopt -o -p errexit; shopt -so pipefail; set -o | grep pipefail")
add("shopt", "globstar", "mkdir -p /tmp/w/a/b && cd /tmp/w && : >a/b/deep.txt && : >top.txt && shopt -s globstar && echo **/*.txt")
add("shopt", "dotglob", "mkdir -p /tmp/w && cd /tmp/w && : >.h && : >v && shopt -s dotglob && echo *")
add("shopt", "nocaseglob", "mkdir -p /tmp/w && cd /tmp/w && : >README && shopt -s nocaseglob && echo read*")
add("shopt", "failglob", "mkdir -p /tmp/w && cd /tmp/w && shopt -s failglob; echo *.nope; echo \"status=$?\"", error=True)
SRC = "mkdir -p /tmp/w && cd /tmp/w && printf 'X=from_file\\nf() { echo \"f got $*\"; }\\n' >lib.sh\n"
add("source", "evaluate a file", SRC + "source lib.sh; echo $X; f 1 2")
add("source", "evaluate a file with a dot", SRC + ". ./lib.sh; echo $X")
add("source", "evaluate a file with arguments", "mkdir -p /tmp/w && cd /tmp/w && printf 'echo \"args: $# $1\"\\n' >a.sh\nsource a.sh one two")
add("source", "evaluate a missing file", "source /tmp/nosuch.sh; echo \"status=$?\"", error=True)
add("source", "evaluate a file with a syntax error", "mkdir -p /tmp/w && cd /tmp/w && printf 'echo before\\nif then\\n' >bad.sh\nsource bad.sh; echo \"status=$?\"", error=True)
add("source", "evaluate a directory", "source /tmp; echo \"status=$?\"", error=True)
add("source", "evaluate a file found on PATH", "mkdir -p /tmp/w/bin && printf 'echo found-on-path\\n' >/tmp/w/bin/lib.sh\nPATH=/tmp/w/bin:$PATH\nsource lib.sh")

# -- test -----------------------------------------------------------------------------------------
for _v in ("/bin/zsh", "", "a b", "-n"):
    add("test", "equal to a string with '" + _v + "'", "MY_VAR='" + _v + "'\ntest \"$MY_VAR\" = \"/bin/zsh\"; echo \"status=$?\"")
    add("test", "empty with '" + _v + "'", "GIT_BRANCH='" + _v + "'\ntest -z \"$GIT_BRANCH\"; echo \"status=$?\"")
add("test", "a file exists", W + "\n: >file && test -f \"file\"; echo \"status=$?\"; test -f \"nope\"; echo \"status=$?\"")
add("test", "a directory does not exist", W + "\nmkdir d && test ! -d \"d\"; echo \"status=$?\"; test ! -d \"nope\"; echo \"status=$?\"")
add("test", "then and else", "test 1 -lt 2 && echo \"true\" || echo \"false\"; test 3 -lt 2 && echo \"true\" || echo \"false\"")
add("test", "in a conditional statement", W + "\n: >f\nif test -f \"f\"; then echo \"File exists\"; else echo \"File does not exist\"; fi\nif test -f \"g\"; then echo \"File exists\"; else echo \"File does not exist\"; fi")
add("test", "no arguments and one argument", "test; echo \"status=$?\"; test ''; echo \"status=$?\"; test x; echo \"status=$?\"")
add("test", "and or with -a -o", "test 1 -eq 1 -a 2 -eq 3; echo \"status=$?\"; test 1 -eq 1 -o 2 -eq 3; echo \"status=$?\"")
add("test", "parentheses", "test \\( 1 -eq 2 \\) -o \\( a = a \\); echo \"status=$?\"")
add("test", "string ordering", "test a \\< b; echo \"status=$?\"; test b \\> c; echo \"status=$?\"")
add("test", "files newer and older", W + "\ntouch -d 2020-01-01 old && touch -d 2021-01-01 new\ntest new -nt old; echo \"status=$?\"; test new -ot old; echo \"status=$?\"; test new -ef new; echo \"status=$?\"")
add("test", "variable set", "v=; test -v v; echo \"status=$?\"; unset v; test -v v; echo \"status=$?\"")
add("test", "integer expected", "test abc -gt 1; echo \"status=$?\"", error=True)

# -- trap / times / time --------------------------------------------------------------------------
add("trap", "list traps", "trap 'echo bye' EXIT; trap 'echo hup' HUP; trap")
add("trap", "list with none set", "trap; echo \"status=$?\"")
add("trap", "run on a signal", "trap 'echo \"Caught signal SIGHUP\"' HUP; kill -HUP $$; echo after")
add("trap", "run on a signal named with SIG", "trap 'echo \"Caught signal SIGHUP\"' SIGHUP; kill -s HUP $$; echo after")
add("trap", "remove commands", "trap 'echo hup' HUP; trap 'echo int' INT; trap - HUP INT; trap; echo \"status=$?\"")
add("trap", "remove with SIG names", "trap 'echo hup' SIGHUP; trap - SIGHUP SIGINT; trap")
add("trap", "list event names", "trap -l")
add("trap", "ignore a signal", "trap '' INT; kill -INT $$; echo still-here; trap")
add("trap", "ignore a signal named with SIG", "trap '' SIGINT; kill -s INT $$; echo still-here")
add("trap", "print one trap", "trap 'echo x' USR1; trap -p USR1")
add("trap", "an invalid signal", "trap 'echo x' NOSUCHSIG; echo \"status=$?\"", error=True)
add("trap", "EXIT in a subshell", "(trap 'echo sub-exit' EXIT; echo in-sub); echo after")
add("trap", "ERR", "trap 'echo \"err at $LINENO\"' ERR; false; true; echo end")
add("trap", "RETURN", "f() { :; }; trap 'echo returned' RETURN; f; echo end")
add("times", "print usage shape", "times | sed 's/[0-9][0-9.]*/N/g'")
add("time", "a command", "{ time echo timed; } 2>/dev/null")
add("time", "a command's status", "{ time false; } 2>/dev/null; echo \"status=$?\"")
add("time", "format shape", "{ time true; } 2>&1 | sed 's/[0-9][0-9.]*/N/g'")
add("time", "TIMEFORMAT", "TIMEFORMAT='took'; { time true; } 2>&1")
add("time", "a pipeline", "{ time echo a | tr a b; } 2>/dev/null")
add("time", "posix format", "{ time -p true; } 2>&1 | sed 's/[0-9][0-9.]*/N/g'")
add("time", "read", "{ time read v; } <<< 'input' 2>/dev/null; echo \"[$v]\"")

# -- if / for / while / until / case / function / select ------------------------------------------
for _c, _label in (("true", "true"), ("false", "false"), ("grep -q x <<< x", "a grep"), ("nosuchcmd", "a missing command")):
    add("if", "condition is " + _label, "if " + _c + "; then echo \"Condition is true\"; fi; echo \"status=$?\"")
    add("if", "negated condition is " + _label, "if ! " + _c + "; then echo \"Condition is true\"; fi")
    add("if", "else branch when " + _label, "if " + _c + "; then echo \"Condition is true\"; else echo \"Condition is false\"; fi")
IFS_ = W + "\nmkdir d && : >f"
for _t, _p in (("-f", "f"), ("-f", "d"), ("-d", "d"), ("-d", "f"), ("-e", "f"), ("-e", "nope")):
    add("if", "check " + _t + " on " + _p, IFS_ + "\nif [[ " + _t + " " + _p + " ]]; then echo \"Condition is true\"; fi; echo \"status=$?\"")
add("if", "a variable is defined", 'variable=x\nif [[ -n "$variable" ]]; then echo "Condition is true"; fi')
add("if", "a variable is not defined", 'if [[ -n "$variable" ]]; then echo "Condition is true"; else echo no; fi')
add("if", "elif chain", "n=2; if [ $n = 1 ]; then echo one; elif [ $n = 2 ]; then echo two; else echo other; fi")
add("if", "status when no branch runs", "if false; then :; fi; echo \"status=$?\"")
add("if", "command list as condition", "if false; true; then echo last-wins; fi")
add("for", "iterate command line parameters", "set -- a 'b c' d; for variable; do echo $variable; done")
add("for", "iterate items", 'for variable in item1 "item 2" item3; do echo "Loop is executed: $variable"; done')
add("for", "iterate an empty list", "for variable in; do echo never; done; echo \"status=$?\"")
add("for", "iterate a range", "for variable in {1..10..3}; do echo \"Loop is executed $variable\"; done")
add("for", "iterate a descending range", "for variable in {5..1..2}; do echo $variable; done")
add("for", "iterate a letter range", "for variable in {a..e}; do printf $variable; done; echo")
add("for", "iterate a zero-padded range", "for variable in {08..11}; do echo $variable; done")
add("for", "iterate files", W + "\n: >file1 && : >'file 2'\nfor variable in file1 'file 2' nope; do [ -e \"$variable\" ] && echo \"exists: $variable\" || echo \"missing: $variable\"; done")
add("for", "iterate directories", W + "\nmkdir dir1 dir2\nfor variable in dir1/ dir2/; do echo \"Loop is executed $variable\"; done")
add("for", "every directory", W + "\nmkdir -p d1 d2 && : >d1/x && : >notadir\nfor variable in */; do (cd \"$variable\" || continue; echo \"in $variable: $(ls)\") done")
add("for", "every directory when there are none", W + "\nfor variable in */; do echo \"[$variable]\"; done")
add("for", "arithmetic for", "for ((i = 0; i < 3; i++)); do echo $i; done")
add("for", "glob with no match", W + "\nfor f in *.nope; do echo \"[$f]\"; done")
add("while", "read stdin", "printf 'l1\\nl2\\n' | while read line; do echo \"$line\"; done")
add("while", "read stdin with backslashes and spaces", "printf '  a\\\\b  \\n' | while read line; do echo \"[$line]\"; done")
add("while", "a command forever with a break", "n=0; while :; do n=$((n+1)); [ $n -ge 3 ] && break; sleep 0.01; done; echo $n")
add("while", "until a command fails", W + "\nprintf '3' >n\nwhile [ \"$(cat n)\" -gt 0 ]; do v=$(cat n); echo $v; echo $((v-1)) >n; done")
add("while", "reading a file with redirection", W + "\nprintf 'a\\nb\\n' >f\nwhile IFS= read -r l; do echo \"<$l>\"; done < f")
add("until", "until a command succeeds", W + "\nn=0; until [ -e ready ]; do n=$((n+1)); [ $n -eq 3 ] && : >ready; done; echo $n")
add("until", "status of an until loop", "until true; do :; done; echo \"status=$?\"")
add("until", "waiting with a sleep", "i=0; until [ $i -ge 2 ]; do echo \"Waiting...\"; i=$((i+1)); sleep 0.01; done; echo \"Launched!\"")
CS = W + "\nprintf 'one two\\nthree\\n' >README\n"
for _v in ("words", "lines", "w", "L", "other", ""):
    add("case", "string literals with '" + _v + "'", CS + "COUNTRULE='" + _v + "'\ncase $COUNTRULE in words) wc --words README ;; lines) wc --lines README ;; esac; echo \"status=$?\"")
    add("case", "patterns and a fallback with '" + _v + "'", CS + "COUNTRULE='" + _v + "'\ncase $COUNTRULE in [wW]|words) wc --words README ;; [lL]|lines) wc --lines README ;; *) echo \"what?\" ;; esac")
for _a in ("cat", "dog", "cow"):
    add("case", "match multiple patterns with " + _a, "ANIMAL=" + _a + "\ncase $ANIMAL in cat) echo \"It's a cat\" ;;& cat|dog) echo \"It's a cat or a dog\" ;;& *) echo \"Fallback\" ;; esac")
    add("case", "fall through with " + _a, "ANIMAL=" + _a + "\ncase $ANIMAL in cat) echo \"It's a cat\" ;& dog) echo \"It's either a dog or cat fell through\" ;& *) echo \"Fallback\" ;; esac")
add("case", "quoted pattern", "v='*'\ncase $v in '*') echo literal-star;; *) echo other;; esac")
add("case", "extglob pattern", "shopt -s extglob\nv=abc123\ncase $v in +([a-z])+([0-9])) echo matched;; esac")
add("case", "nocasematch", "shopt -s nocasematch\ncase ABC in abc) echo matched;; esac")
add("case", "parenthesized pattern", "case x in (x) echo paren;; esac")
add("function", "define with the keyword", 'function func_name { echo "Function contents here"; }\nfunc_name')
add("function", "run a function", 'func_name() { echo "ran with $#: $*"; }\nfunc_name a "b c"')
add("function", "define without the keyword", 'func_name() { echo "Function contents here"; }\nfunc_name; declare -f func_name')
add("function", "keyword with parentheses", "function f() { echo both; }\nf")
add("function", "recursion", "fact() { if [ $1 -le 1 ]; then echo 1; else echo $(( $1 * $(fact $(( $1 - 1 ))) )); fi; }\nfact 6")
add("function", "a subshell body", "f() ( cd /tmp; pwd )\nf; pwd")
add("function", "redefine", "f() { echo one; }\nf() { echo two; }\nf")
add("function", "FUNCNAME", "outer() { inner; }\ninner() { echo \"${FUNCNAME[@]}\"; }\nouter")
add("function", "an invalid name", "function 1=2 { :; }; echo \"status=$?\"", error=True)
for _choice, _label in (("1", "the first"), ("3", "the third"), ("9", "out of range"), ("", "an empty line"), ("x", "not a number")):
    add("select", "menu of words choosing " + _label, "select word in apple orange pear banana; do echo \"[$word] [$REPLY]\"; break; done <<< '" + _choice + "'")
add("select", "menu from command output", "select line in $(printf 'x\\ny\\n'); do echo $line; break; done <<< 2")
add("select", "prompt and files", W + "\n: >a && : >b\nPS3=\"Select a file: \"; select file in *; do echo $file; break; done <<< 2")
add("select", "menu from an array", "fruits=(apple orange pear banana); select word in ${fruits[@]}; do echo $word; break; done <<< 4")
add("select", "end of input", "select word in a b; do echo $word; done < /dev/null; echo \"status=$?\"")
add("select", "several answers", "printf '9\\n1\\n' | { select word in a b; do [ -n \"$word\" ] && { echo \"got $word\"; break; }; echo \"retry $REPLY\"; done; }")


# Dropped after recording, with the reason for each group.
# `type -a` lists /bin/echo on the oracle; here commands are builtins, not files on PATH (the documented
# deliberate difference BUILTIN_LOOKUP in env_facts.py).
_DROPPED_T = (
    'real tldr type: all locations of a builtin',
    'real tldr type: all locations of a function shadowing a builtin',
)
_DROPPED = set(_DROPPED_T)
CASES = [case for case in CASES if case[0] not in _DROPPED]

# Divergences that are decisions, not bugs.
ALL_OR_NOTHING = "fixture: bash-tool's documented all-or-nothing handling of syntax errors (README) runs nothing of a sourced file with a syntax error, where bash runs the commands before it as it reads them; the same decision as ALL_OR_NOTHING in refusals.py"
STOPPED = (
    "a job cannot outlive its call: bash-tool stops every job still running when the call ends, "
    "wherever it was started (disowned, or orphaned by its subshell), and reports it; bash "
    "leaves it running"
)
EXPECTED = {
    "real tldr disown: the current job": (
        0, b"status=0\n", b"bash: stopped job [1] (pid 2, hangup): sleep 0.2\n", STOPPED,
    ),
    "real tldr disown: all jobs": (
        0, b"done\n",
        b"bash: stopped job [1] (pid 2, hangup): sleep 0.2\nbash: stopped job [2] (pid 3, hangup): sleep 0.2\n",
        STOPPED,
    ),
    "real tldr kill: a trapped signal in a job": (
        0, b"caught\nstatus=7\n", b"bash: stopped job (pid 3, hangup): sleep 5\n", STOPPED,
    ),
    'real tldr source: evaluate a file with a syntax error': (
        0, b'status=2\n', b"bad.sh: line 2: syntax error near unexpected token `then'\nbad.sh: line 2: `if then'\n",
        ALL_OR_NOTHING,
    ),
}
