"""Option sweep, part G: every option of Bash's builtins as this shell provides them.

Left out, because they are refused or deliberately different by documentation: bg, fg, fc,
umask, ulimit, `enable -n`, `exec -a/-c/-l` as exec's own options, `wait -p/-f`, prompt
expansion, `trap -l` (the wasm target lists fewer signals), listings of every builtin or command
(commands here are builtins, not files on PATH) and the interactive-only builtins (bind, help,
history). Everything else is compared with Bash exactly.
"""

TIER = "sweep"
CASES = []


def add(name, label, script):
    CASES.append((f"opt {name}: {label}", script, [f"builtin.{name}"]))


ST = "; echo \"status=$?\""

# --- . and source ---------------------------------------------------------------------------------
for b in [".", "source"]:
    add(b, "with arguments", "printf 'echo \"$# $1 $2\"\\n' >/tmp/s; " + b + " /tmp/s a b")
    add(b, "keeps caller arguments", "printf 'echo \"$1\"\\n' >/tmp/s; set -- outer; " + b + " /tmp/s")
    add(b, "missing file", b + " /tmp/nosuch" + ST)
    add(b, "no argument", b + ST)
    add(b, "return status", "printf 'return 3\\necho no\\n' >/tmp/s; " + b + " /tmp/s" + ST)
    add(b, "last status", "printf 'false\\n' >/tmp/s; " + b + " /tmp/s" + ST)
    add(b, "directory", b + " /tmp" + ST)
    add(b, "relative name in cwd", "cd /tmp; printf 'echo found\\n' >rel.sh; " + b + " rel.sh" + ST)
    add(b, "path search", "mkdir /tmp/bin; printf 'echo via path\\n' >/tmp/bin/p.sh; PATH=/tmp/bin; " + b + " p.sh" + ST)
    add(b, "sourcepath off", "mkdir /tmp/bin; printf 'echo via path\\n' >/tmp/bin/p.sh; PATH=/tmp/bin; shopt -u sourcepath; " + b + " p.sh" + ST)
    add(b, "sets variables", "printf 'X=1\\nf() { echo f$X; }\\n' >/tmp/s; " + b + " /tmp/s; f")
    add(b, "syntax error in file", "printf 'if\\n' >/tmp/s; " + b + " /tmp/s" + ST)
    add(b, "double dash", "printf 'echo dd\\n' >/tmp/s; " + b + " -- /tmp/s")
    add(b, "BASH_SOURCE", "printf 'echo \"${BASH_SOURCE[0]}\"\\n' >/tmp/s; " + b + " /tmp/s")
    add(b, "invalid option", b + " -z /tmp/s" + ST)

# --- : true false -----------------------------------------------------------------------------------
add(":", "arguments expanded", ": ${X:=set}; echo $X")
add(":", "redirection creates file", ": >/tmp/c; ls /tmp")
add(":", "status after failure", "false; :" + ST)
add("true", "arguments ignored", "true --help -x" + ST)
add("false", "arguments ignored", "false --help" + ST)
add("true", "in pipeline", "true | false" + ST)
add("false", "negated", "! false" + ST)

# --- test and [ -----------------------------------------------------------------------------------
SET = ("mkdir -p /tmp/t/d; cd /tmp/t; printf x >f; : >e; ln -s f l; ln -s nosuch dl; touch -d 2000-01-01 old; touch -d 2010-01-01 new; ")
EXPRS = ["-e f", "-e nosuch", "-f f", "-f d", "-d d", "-d f", "-s f", "-s e", "-h l", "-L l", "-L f", "-h dl", "-e dl", "-b f", "-c /dev/null",
         "-c f", "-p f", "-S f", "-t 5", "f -nt old", "old -nt f", "old -ot new", "new -ot old",
         "nosuch -nt f", "f -nt nosuch", "f -ef l", "f -ef e", "-z ''", "-z x", "-n ''", "-n x", "x", "''", "a = a", "a == a", "a != b", "a = b",
         "a '<' b", "b '>' a", "1 -eq 1", "1 -ne 2", "2 -lt 10", "10 -le 10", "3 -gt 2", "3 -ge 4", "' 1 ' -eq 1", "-1 -lt 0", "0x10 -eq 16",
         "010 -eq 8", "a -eq 1", "1 -eq", "-v HOME", "-v PWD", "-R x", "! -e f", "! ''", "-e f -a -d d", "-e f -a -e nosuch", "-e nosuch -o -d d",
         "\\( -e f \\)", "\\( -e f -o -e g \\) -a -d d", "! \\( -z x \\)", "-o errexit", "-o bogus", "a b c", "-x", "-f", "=", "a =", "-e f -a",
         "! ! x", "x -a ''", "'(' x ')'", "-n", "-z", "a -a b -o ''", "99999999999999999999 -gt 1", "1 -eq 1.0", "-l", "-a f"]
for e in EXPRS:
    add("test", f"expression {e}", SET + "test " + e + ST)
for e in EXPRS[:60]:
    add("[", f"expression {e}", SET + "[ " + e + " ]" + ST)
add("[", "missing bracket", "[ x" + ST)
add("[", "extra bracket", "[ x ] ]" + ST)
add("[", "empty", "[ ]" + ST)
add("test", "no arguments", "test" + ST)
add("test", "variable set with array index", "a=(1 2); test -v 'a[1]'" + ST + "; test -v 'a[5]'" + ST)
add("test", "nameref -R", "declare -n r=x; test -R r" + ST)

# --- alias / unalias --------------------------------------------------------------------------------
add("alias", "define and list", "alias ll='ls -l' a1=b; alias")
add("alias", "print one", "alias x='echo hi'; alias x")
add("alias", "p option", "alias x=y; alias -p")
add("alias", "missing", "alias nosuch" + ST)
add("alias", "not expanded without expand_aliases", "alias hi='echo alias'; hi" + ST)
add("alias", "expanded with expand_aliases", "shopt -s expand_aliases\nalias hi='echo alias'\nhi")
add("alias", "trailing space chains", "shopt -s expand_aliases\nalias a='echo ' b=bee\na b")
add("alias", "quoting in listing", "alias q=\"it's\"; alias q")
add("alias", "invalid name", "alias 'a b=c'" + ST)
add("alias", "empty value", "alias e=; alias e")
add("alias", "invalid option", "alias -z" + ST)
add("unalias", "remove", "alias x=y; unalias x; alias x" + ST)
add("unalias", "all", "alias x=y z=w; unalias -a; alias; echo done")
add("unalias", "missing", "unalias nosuch" + ST)
add("unalias", "no argument", "unalias" + ST)

# --- break / continue -----------------------------------------------------------------------------
add("break", "nested levels", "for i in 1 2; do for j in a b; do echo $i$j; break 2; done; done")
add("break", "level larger than depth", "for i in 1 2; do break 5; done; echo after $i")
add("break", "zero", "for i in 1 2; do break 0; done" + ST)
add("break", "negative", "for i in 1; do break -1; done" + ST)
add("break", "non numeric", "for i in 1; do break x; done" + ST)
add("break", "outside loop", "break" + ST)
add("break", "in while condition", "i=0; while break; do echo body; done; echo out")
add("break", "in function called from loop", "f() { break; }; for i in 1 2; do f; echo $i; done" + ST)
add("continue", "nested levels", "for i in 1 2; do for j in a b; do continue 2; echo no; done; echo no; done; echo $i$j")
add("continue", "zero", "for i in 1; do continue 0; done" + ST)
add("continue", "outside loop", "continue" + ST)
add("continue", "in until", "i=0; until [ $i -ge 3 ]; do i=$((i+1)); [ $i = 2 ] && continue; echo $i; done")
add("continue", "too many arguments", "for i in 1; do continue 1 2; done" + ST)

# --- builtin / command / type / hash --------------------------------------------------------------
add("builtin", "bypasses function", "echo() { printf 'fn\\n'; }; builtin echo real")
add("builtin", "not a builtin", "builtin nosuch" + ST)
add("builtin", "no arguments", "builtin" + ST)
add("builtin", "status of builtin", "builtin false" + ST)
add("builtin", "keyword is not builtin", "builtin if" + ST)
add("command", "bypasses function", "cd() { echo fn; }; command cd /tmp; pwd")
add("command", "v builtin", "command -v cd")
add("command", "v function", "f() { :; }; command -v f")
add("command", "v keyword", "command -v if")
add("command", "v alias", "alias a='echo x'; command -v a")
add("command", "v missing", "command -v nosuch" + ST)
add("command", "V builtin", "command -V cd")
add("command", "V function", "f() { :; }; command -V f")
add("command", "V keyword", "command -V while")
add("command", "V missing", "command -V nosuch" + ST)
add("command", "p with builtin", "command -p echo ok")
add("command", "missing command", "command nosuch" + ST)
add("command", "no arguments", "command" + ST)
add("command", "v several", "f() { :; }; command -v f cd nosuch if" + ST)
add("command", "invalid option", "command -z" + ST)
add("type", "builtin", "type cd")
add("type", "keyword", "type if")
add("type", "function", "f() { echo x; }; type f")
add("type", "alias", "alias a='echo x'; type a")
add("type", "missing", "type nosuch" + ST)
add("type", "t forms", "f() { :; }; alias a=b; type -t f a cd if; type -t nosuch" + ST)
add("type", "f skips functions", "cd() { :; }; type -f cd")
add("type", "p nothing for builtin", "type -p cd" + ST)
add("type", "P missing", "type -P nosuch" + ST)
add("type", "several with missing", "type cd nosuch" + ST)
add("type", "invalid option", "type -z" + ST)
add("type", "function with body", "f() { if true; then echo a; fi; for x in 1; do :; done; }; type f")
add("hash", "empty table", "hash" + ST)
add("hash", "not found", "hash nosuch" + ST)
add("hash", "p and t", "hash -p /tmp/x myname; hash -t myname")
add("hash", "p and listing", "hash -p /tmp/x myname; hash")
add("hash", "l listing", "hash -p /tmp/x myname; hash -l")
add("hash", "d removes", "hash -p /tmp/x myname; hash -d myname; hash -t myname" + ST)
add("hash", "r clears", "hash -p /tmp/x myname; hash -r; hash" + ST)
add("hash", "t missing", "hash -t nosuch" + ST)
add("hash", "d missing", "hash -d nosuch" + ST)
add("hash", "hashall off", "set +h; hash nosuch" + ST)

# --- caller ---------------------------------------------------------------------------------------
add("caller", "outside function", "caller" + ST)
add("caller", "in function", "f() { caller; }; f")
add("caller", "frame zero", "f() { caller 0; }; f")
add("caller", "nested frames", "g() { caller 0; caller 1; caller 2" + ST + "; }; f() { g; }; f")
add("caller", "invalid frame", "f() { caller x; }; f" + ST)
add("caller", "in sourced file", "printf 'caller\\n' >/tmp/s; . /tmp/s")

# --- cd / pwd / pushd / popd / dirs ----------------------------------------------------------------
CD = "mkdir -p /tmp/c/real/sub; cd /tmp/c; ln -s real link; "
add("cd", "logical", CD + "cd link && pwd && cd .. && pwd")
add("cd", "physical", CD + "cd -P link && pwd && cd .. && pwd")
add("cd", "L option", CD + "cd -L link/sub && pwd")
add("cd", "e option", CD + "cd -Pe link" + ST)
add("cd", "dash", CD + "cd real; cd -; cd -")
add("cd", "dash without OLDPWD", "unset OLDPWD; cd -" + ST)
add("cd", "home not set", "unset HOME; cd" + ST)
add("cd", "home set", "HOME=/tmp; cd; pwd")
add("cd", "cdpath", CD + "CDPATH=/tmp/c; cd /; cd real; pwd")
add("cd", "cdpath dot", CD + "CDPATH=:/tmp/c; cd real; pwd")
add("cd", "missing", "cd /tmp/nosuch" + ST)
add("cd", "not a directory", "printf x >/tmp/f; cd /tmp/f" + ST)
add("cd", "too many arguments", "cd /tmp /" + ST)
add("cd", "empty argument", "cd /tmp; cd ''" + ST + "; pwd")
add("cd", "double dash", "cd -- /tmp; pwd")
add("cd", "invalid option", "cd -z /tmp" + ST)
add("cd", "cdable vars", "shopt -s cdable_vars; d=/tmp; cd d; pwd")
add("cd", "updates PWD and OLDPWD", "cd /tmp; cd /; echo \"$PWD $OLDPWD\"")
add("cd", "dot dot from root", "cd /..; pwd")
add("cd", "trailing slashes", "cd /tmp///; pwd")
add("cd", "relative with dot dot", CD + "cd real/sub/../sub/.; pwd")
add("cd", "physical option set", CD + "set -P; cd link; pwd")
add("pwd", "logical", CD + "cd link; pwd -L")
add("pwd", "physical", CD + "cd link; pwd -P")
add("pwd", "PWD tampered", "cd /tmp; PWD=/nope; pwd")
add("pwd", "invalid option", "pwd -z" + ST)
add("pwd", "extra argument", "pwd x" + ST)
add("pwd", "directory removed", "mkdir /tmp/g; cd /tmp/g; rmdir /tmp/g; pwd" + ST)
add("pushd", "push and list", "mkdir -p /tmp/a /tmp/b; cd /; pushd /tmp/a; pushd /tmp/b; dirs")
add("pushd", "swap", "mkdir -p /tmp/a; cd /; pushd /tmp/a >/dev/null; pushd; pwd")
add("pushd", "no other directory", "pushd" + ST)
add("pushd", "rotate plus", "mkdir -p /tmp/a /tmp/b; cd /; pushd /tmp/a >/dev/null; pushd /tmp/b >/dev/null; pushd +1; pwd")
add("pushd", "rotate minus", "mkdir -p /tmp/a /tmp/b; cd /; pushd /tmp/a >/dev/null; pushd /tmp/b >/dev/null; pushd -0; pwd")
add("pushd", "no change", "mkdir -p /tmp/a; cd /; pushd -n /tmp/a; pwd")
add("pushd", "missing directory", "pushd /tmp/nosuch" + ST)
add("pushd", "index out of range", "pushd +3" + ST)
add("pushd", "invalid argument", "pushd -x" + ST)
add("popd", "pop", "mkdir -p /tmp/a; cd /; pushd /tmp/a >/dev/null; popd; pwd")
add("popd", "empty stack", "popd" + ST)
add("popd", "plus index", "mkdir -p /tmp/a /tmp/b; cd /; pushd /tmp/a >/dev/null; pushd /tmp/b >/dev/null; popd +1; pwd")
add("popd", "minus index", "mkdir -p /tmp/a /tmp/b; cd /; pushd /tmp/a >/dev/null; pushd /tmp/b >/dev/null; popd -1; pwd")
add("popd", "no change", "mkdir -p /tmp/a; cd /; pushd /tmp/a >/dev/null; popd -n; pwd")
add("popd", "out of range", "mkdir -p /tmp/a; cd /; pushd /tmp/a >/dev/null; popd +5" + ST)
add("dirs", "plain", "cd /tmp; dirs")
add("dirs", "clear", "mkdir -p /tmp/a; cd /; pushd /tmp/a >/dev/null; dirs -c; dirs")
add("dirs", "long", "HOME=/tmp; mkdir -p /tmp/a; cd /tmp/a; dirs; dirs -l")
add("dirs", "per line", "mkdir -p /tmp/a; cd /; pushd /tmp/a >/dev/null; dirs -p")
add("dirs", "verbose", "mkdir -p /tmp/a; cd /; pushd /tmp/a >/dev/null; dirs -v")
add("dirs", "plus index", "mkdir -p /tmp/a; cd /; pushd /tmp/a >/dev/null; dirs +1; dirs -0")
add("dirs", "out of range", "dirs +3" + ST)
add("dirs", "invalid option", "dirs -z" + ST)
add("dirs", "DIRSTACK array", "mkdir -p /tmp/a; cd /; pushd /tmp/a >/dev/null; echo \"${DIRSTACK[@]}\"")

# --- compgen / complete / compopt ------------------------------------------------------------------
add("compgen", "words with prefix", "compgen -W 'apple apricot banana' ap")
add("compgen", "words no match", "compgen -W 'a b' z" + ST)
add("compgen", "functions", "fa() { :; }; fb() { :; }; compgen -A function f")
add("compgen", "variables", "zz1=1 zz2=2; compgen -v zz")
add("compgen", "aliases", "alias al1=x al2=y; compgen -a al")
add("compgen", "keywords", "compgen -k | sort")
add("compgen", "keywords prefix", "compgen -k th")
add("compgen", "files", "mkdir -p /tmp/cg/sub; cd /tmp/cg; touch f1 f2 .h; compgen -f | sort")
add("compgen", "files prefix", "mkdir -p /tmp/cg; cd /tmp/cg; touch f1 f2 g1; compgen -f f | sort")
add("compgen", "directories", "mkdir -p /tmp/cg/d1 /tmp/cg/d2; cd /tmp/cg; touch file; compgen -d | sort")
add("compgen", "directory action", "mkdir -p /tmp/cg/d1; cd /tmp/cg; compgen -A directory d")
add("compgen", "exported", "export ZEXP=1; compgen -e ZEX")
add("compgen", "export action", "export ZEXP=1; compgen -A export ZE")
add("compgen", "setopt action", "compgen -A setopt err")
add("compgen", "shopt action", "compgen -A shopt ext")
add("compgen", "arrayvar action", "arr1=(1); compgen -A arrayvar arr")
add("compgen", "prefix and suffix", "compgen -W 'a b' -P '<' -S '>'")
add("compgen", "filter pattern", "compgen -W 'ab ac bd' -X 'a*'")
add("compgen", "negated filter", "compgen -W 'ab ac bd' -X '!a*'")
add("compgen", "variable action", "zq=1; compgen -A variable zq")
add("compgen", "word list expansion", "x=1; compgen -W '$x \"a b\"'")
add("compgen", "jobs action", "sleep 1 & compgen -j; kill %1; wait" + ST)
add("compgen", "invalid action", "compgen -A bogus" + ST)
add("compgen", "invalid option", "compgen -z" + ST)
add("compgen", "function name", "g() { COMPREPLY=(one two); }; compgen -F g" + ST)
add("compgen", "no options", "compgen x" + ST)
add("compgen", "variable output", "compgen -V arr -W 'a b c'; echo \"${arr[@]}\"")
add("complete", "list empty", "complete -p" + ST)
add("complete", "define and print", "complete -W 'a b' mycmd; complete -p mycmd")
add("complete", "function and options", "complete -o nospace -F f mycmd; complete -p mycmd")
add("complete", "remove", "complete -W x mycmd; complete -r mycmd; complete -p mycmd" + ST)
add("complete", "print missing", "complete -p nosuch" + ST)
add("complete", "remove all", "complete -W x a; complete -W y b; complete -r; complete -p; echo done")
add("complete", "default and empty", "complete -D -W d; complete -E -W e; complete -p")
add("complete", "invalid option", "complete -z x" + ST)
add("compopt", "outside completion", "compopt -o nospace" + ST)
add("compopt", "for named command", "complete -W x mycmd; compopt -o nospace mycmd; complete -p mycmd")
add("compopt", "invalid option name", "compopt -o bogus x" + ST)

# --- declare / typeset / local / readonly / export / unset ---------------------------------------
for d in ["declare", "typeset"]:
    add(d, "print variable", d + " x=1; " + d + " -p x")
    add(d, "integer", d + " -i n; n='2+3'; echo $n; n=abc; echo $n")
    add(d, "lowercase", d + " -l s=HeLLo; echo $s; s=ABC; echo $s")
    add(d, "uppercase", d + " -u s=hi; echo $s")
    add(d, "indexed array", d + " -a a=(x y); " + d + " -p a")
    add(d, "associative array", d + " -A m=([k]=v [a]=b); echo \"${m[k]} ${#m[@]}\"")
    add(d, "readonly", d + " -r r=1; r=2" + ST)
    add(d, "export", d + " -x E=1; sh -c 'echo $E'")
    add(d, "remove export", "export E=1; " + d + " +x E; sh -c 'echo \"[$E]\"'")
    add(d, "nameref", d + " -n ref=target; target=5; echo $ref; ref=6; echo $target")
    add(d, "functions names", "f() { :; }; g() { :; }; " + d + " -F")
    add(d, "function body", "f() { echo \"$1\"; }; " + d + " -f f")
    add(d, "function missing", d + " -f nosuch" + ST)
    add(d, "invalid identifier", d + " 1x=2" + ST)
    add(d, "print missing", d + " -p nosuch" + ST)
declare_cases = [
    ("print attributes", "declare -ix n=3; declare -p n"), ("print several", "a=1 b=2; declare -p a b"),
    ("print quoting", "v=\"it's \\\"q\\\" $\"; declare -p v"), ("print newline value", "v=$'a\\nb'; declare -p v"),
    ("print array sparse", "a[3]=x a[7]=y; declare -p a"), ("print assoc", "declare -A m=([b]=2); declare -p m"),
    ("print empty array", "declare -a e=(); declare -p e"), ("print declared unset", "declare u; declare -p u"),
    ("global from function", "f() { declare -g G=1; }; f; echo $G"), ("local by default in function", "f() { declare L=1; }; f; echo \"[$L]\""),
    ("trace attribute", "f() { :; }; declare -t f; declare -F f" + ST), ("indexed to assoc error", "a=(1); declare -A a" + ST),
    ("assoc to indexed error", "declare -A m; declare -a m" + ST), ("readonly removal error", "declare -r r=1; declare +r r" + ST),
    ("integer arithmetic append", "declare -i n=5; n+=3; echo $n"), ("lowercase then uppercase", "declare -l x; declare -u x; x=aB; echo $x"),
    ("nameref to array element", "a=(1 2); declare -n r='a[1]'; echo $r"), ("nameref loop", "declare -n a=b; declare -n b=a; echo $a" + ST),
    ("nameref invalid", "declare -n r=1bad" + ST), ("I option inherits", "x=out; f() { declare -I x; echo \"[$x]\"; }; f"),
    ("p all attributes filtered", "declare -r RO=1; declare -p RO"), ("array append", "declare -a a=(1); a+=(2 3); declare -p a"),
    ("assoc append", "declare -A m=([a]=1); m+=([b]=2); echo ${m[b]}"), ("compound assign with index", "declare -a a=([2]=c [0]=a); echo ${a[@]}"),
    ("p function flag", "f() { :; }; declare -pf f"), ("F with name", "f() { :; }; declare -F f nosuch" + ST),
    ("invalid option", "declare -z x" + ST), ("uppercase integer", "declare -iu x=3; echo $x"),
    ("export function", "f() { echo exported; }; declare -fx f; bash -c f"), ("plus i removes", "declare -i n; declare +i n; n=1+1; echo $n"),
    ("print arrays of attribute", "declare -a a=(1) b=(2); declare -pa | grep -E ' (a|b)='"),
]
for label, s in declare_cases:
    add("declare", label, s)
add("local", "outside function", "local x=1" + ST)
add("local", "shadowing", "x=g; f() { local x=l; echo $x; }; f; echo $x")
add("local", "dynamic scope", "f() { local x=f; g; }; g() { echo $x; }; x=g; f")
add("local", "array", "f() { local -a a=(1 2); echo ${#a[@]}; }; f")
add("local", "assoc", "f() { local -A m=([k]=v); echo ${m[k]}; }; f")
add("local", "readonly", "f() { local -r x=1; x=2; }; f" + ST)
add("local", "integer", "f() { local -i n=2*3; echo $n; }; f")
add("local", "nameref", "f() { local -n r=$1; r=set; }; f out; echo $out")
add("local", "no value unset", "x=g; f() { local x; echo \"[${x-unset}]\"; }; f")
add("local", "list locals", "f() { local a=1 b; local; }; f")
add("local", "dash saves options", "f() { local -; set -e; }; f; echo $- | grep -c e")
add("local", "status of command substitution", "f() { local x=$(false); echo $?; }; f")
add("local", "invalid name", "f() { local 1x; }; f" + ST)
add("local", "unset local reveals global", "x=g; f() { local x=l; unset x; echo \"[${x-unset}]\"; }; f")
add("local", "localvar_inherit", "shopt -s localvar_inherit; x=g; f() { local x; echo \"[$x]\"; }; f")
add("local", "localvar_unset", "shopt -s localvar_unset; x=g; f() { local x=l; unset x; echo \"[${x-unset}]\"; }; f")
add("readonly", "assign error", "readonly r=1; r=2" + ST + "; echo $r")
add("readonly", "print", "readonly r=1; readonly -p | grep ' r='")
add("readonly", "array", "readonly -a a=(1 2); a[0]=3" + ST)
add("readonly", "assoc", "readonly -A m=([k]=v); echo ${m[k]}")
add("readonly", "function", "f() { echo f; }; readonly -f f; f() { echo g; }" + ST)
add("readonly", "unset error", "readonly r=1; unset r" + ST)
add("readonly", "existing variable", "x=1; readonly x; x=2" + ST)
add("readonly", "invalid name", "readonly 1x=2" + ST)
add("readonly", "invalid option", "readonly -z x" + ST)
add("readonly", "local shadow error", "readonly r=1; f() { local r=2; }; f" + ST)
add("export", "name value", "export E=1; sh -c 'echo $E'")
add("export", "existing", "E=2; export E; sh -c 'echo $E'")
add("export", "remove", "export E=1; export -n E; sh -c 'echo \"[$E]\"'")
add("export", "function", "f() { echo fx; }; export -f f; sh -c f")
add("export", "function missing", "export -f nosuch" + ST)
add("export", "print filtered", "export ZZE=1; export -p | grep ZZE")
add("export", "invalid name", "export 1x=1" + ST)
add("export", "invalid option", "export -z" + ST)
add("export", "array export", "export A=(1 2); sh -c 'echo \"[$A]\"'")
add("export", "unset but exported", "export U; sh -c 'echo \"[${U-unset}]\"'; export -p | grep -c ' U$'")
add("export", "allexport", "set -a; AX=1; sh -c 'echo $AX'")
add("unset", "variable", "x=1; unset x; echo \"[${x-unset}]\"")
add("unset", "function", "f() { :; }; unset -f f; f" + ST)
add("unset", "v option", "x=1; unset -v x; echo \"[${x-unset}]\"")
add("unset", "function fallback", "f() { echo f; }; unset f; f" + ST)
add("unset", "array element", "a=(1 2 3); unset 'a[1]'; echo \"${a[@]} ${#a[@]}\"")
add("unset", "whole array", "a=(1 2); unset a; echo \"${#a[@]}\"")
add("unset", "assoc element", "declare -A m=([a]=1 [b]=2); unset 'm[a]'; echo \"${!m[@]}\"")
add("unset", "nameref n", "declare -n r=x; x=1; unset -n r; echo \"$x [${r-unset}]\"")
add("unset", "through nameref", "declare -n r=x; x=1; unset r; echo \"[${x-unset}]\"")
add("unset", "missing ok", "unset nosuch" + ST)
add("unset", "invalid name", "unset 1x" + ST)
add("unset", "both f and v", "unset -f -v x" + ST)
add("unset", "special variable", "unset PWD; echo \"[${PWD-unset}]\"")
add("unset", "negative index", "a=(1 2 3); unset 'a[-1]'; echo \"${a[@]}\"")

# --- echo -----------------------------------------------------------------------------------------
for a in ["-n x", "-e 'a\\tb'", "-E 'a\\tb'", "-ne 'x\\n'", "-en 'y'", "'a\\tb'", "-e 'a\\cb'", "-e '\\0101'", "-e '\\101'", "-e '\\x41\\x4'",
          "-e '\\u00e9'", "-e '\\U0001F600'", "-e '\\e[0m' | od -An -c", "-e '\\\\'", "-e 'a\\'", "--", "-", "-x", "-nx", "-n -e 'a\\n'", "-e -n 'b'",
          "'-n'", "\"-n \"", "-eE 'a\\tb'", "-Ee 'a\\tb'", "", "a  b", "-e '\\a\\b\\f\\v\\r' | od -An -c"]:
    add("echo", f"arguments {a or 'none'}", "echo " + a + "; echo \"[status=$?]\"")
add("echo", "xpg_echo", "shopt -s xpg_echo; echo 'a\\tb'; echo -e 'c\\td'")
add("echo", "write error", "echo x >&-" + ST)

# --- enable ---------------------------------------------------------------------------------------
add("enable", "known builtin", "enable echo" + ST)
add("enable", "unknown builtin", "enable nosuch" + ST)
add("enable", "special builtins", "enable -s")
add("enable", "disabled list empty", "enable -n" + ST)
add("enable", "print one", "enable -p cd 2>&1 | head -n 3" + ST)
add("enable", "invalid option", "enable -z" + ST)
add("enable", "load from file", "enable -f /tmp/nosuch.so x" + ST)
add("enable", "delete", "enable -d echo" + ST)

# --- eval -----------------------------------------------------------------------------------------
add("eval", "concatenates", "eval echo '$((1+2))' 'a'")
add("eval", "status", "eval false" + ST)
add("eval", "empty", "eval" + ST + "; eval ''" + ST)
add("eval", "syntax error", "eval 'if'" + ST + "; echo continued")
add("eval", "defines function", "eval 'f() { echo defined; }'; f")
add("eval", "nested quoting", "x='a b'; eval \"echo \\\"$x\\\"\"")
add("eval", "exit inside", "eval 'exit 3'; echo no")
add("eval", "double dash", "eval -- echo ok")
add("eval", "invalid option", "eval -z" + ST)
add("eval", "LINENO in eval", "eval 'echo $LINENO'")

# --- exec -----------------------------------------------------------------------------------------
add("exec", "redirect then close", "exec 3>/tmp/x; echo a >&3; exec 3>&-; cat /tmp/x")
add("exec", "read descriptor", "printf 'l1\\nl2\\n' >/tmp/x; exec 4</tmp/x; read -u 4 a; read -u 4 b; echo $b$a")
add("exec", "move descriptor", "exec 5>/tmp/x; exec 6>&5-; echo moved >&6; cat /tmp/x; echo hi >&5" + ST)
add("exec", "missing command", "exec nosuch; echo after")
add("exec", "command with dash-l argument", "printf 'a\\nb\\n' >/tmp/f; exec wc -l /tmp/f")
add("exec", "command with dash-c argument", "printf 'a\\nb\\n' >/tmp/f; exec grep -c a /tmp/f")
add("exec", "command with dash-a argument", "exec printf '%s\\n' -a")
add("exec", "command after double dash", "exec -- echo dd")
add("exec", "stdout redirect persists", "exec >/tmp/o; echo into; exec >&2; cat /tmp/o >&2")
add("exec", "stderr to stdout", "exec 2>&1; ls /tmp/nosuch" + ST)
add("exec", "invalid option", "exec -z echo" + ST)
add("exec", "status of exec", "exec true; echo no")
add("exec", "bad descriptor", "exec 3<&9" + ST)
add("exec", "execfail shopt", "shopt -s execfail; exec nosuch; echo survived $?")
add("exec", "in subshell", "(exec echo sub); echo parent")

# --- exit / return --------------------------------------------------------------------------------
add("exit", "code", "exit 5")
add("exit", "wraps 256", "exit 256")
add("exit", "negative", "exit -1")
add("exit", "non numeric", "exit abc; echo after")
add("exit", "too many arguments", "exit 1 2; echo after $?")
add("exit", "default last status", "false; exit")
add("exit", "in subshell", "(exit 4); echo $?")
add("exit", "in function", "f() { exit 6; }; f; echo no")
add("exit", "with exit trap", "trap 'echo trap $?' EXIT; exit 3")
add("exit", "large number", "exit 99999999999999999999")
add("exit", "plus sign", "exit +7")
add("exit", "in command substitution", "x=$(exit 9); echo $?")
add("return", "outside function", "return 1" + ST)
add("return", "value", "f() { return 7; }; f" + ST)
add("return", "wraps", "f() { return 300; }; f" + ST)
add("return", "default last", "f() { false; return; }; f" + ST)
add("return", "non numeric", "f() { return x; }; f" + ST)
add("return", "negative", "f() { return -2; }; f" + ST)
add("return", "too many arguments", "f() { return 1 2; }; f" + ST)
add("return", "from sourced file", "printf 'return 4\\n' >/tmp/s; . /tmp/s" + ST)
add("return", "in subshell of function", "f() { (return 3); echo sub=$?; }; f")

# --- getopts --------------------------------------------------------------------------------------
GO = "f() { while getopts %s o; do echo \"$o[${OPTARG-}]\"; done; echo \"ind=$OPTIND\"; }; f %s"
for spec, args in [("ab:", "-a -b x rest"), ("ab:", "-ab x"), ("ab:", "-bfoo"), ("ab:", "-z"), ("ab:", "-b"), (":ab:", "-z"), (":ab:", "-b"),
                   ("a", "-a -- -a"), ("a", "rest -a"), ("a", "-"), ("ab", "-ba"), ("a:", "-a ''"), ("a", "-aa"), ("x:y:", "-x 1 -y 2"),
                   ("a", "--"), ("a", "-a --bogus"), ("a", ""), ("", "-a")]:
    add("getopts", f"spec {spec or 'empty'} args {args or 'none'}", GO % ("'" + spec + "'", args))
add("getopts", "OPTERR zero", "f() { OPTERR=0; getopts a o; echo \"$o $?\"; }; f -z")
add("getopts", "explicit arguments", "getopts ab: o -b val; echo \"$o $OPTARG $OPTIND\"")
add("getopts", "OPTIND reset", "getopts a o -a; OPTIND=1; getopts a o -a; echo $o $OPTIND")
add("getopts", "missing arguments", "getopts" + ST)
add("getopts", "invalid name", "getopts a 1x -a" + ST)
add("getopts", "end status", "set --; getopts a o" + ST + "; echo \"[$o]\"")
add("getopts", "digit option", "getopts 1 o -1; echo $o")
add("getopts", "colon in spec position", "getopts 'a:' o -a; echo \"$o [$OPTARG]\"" + ST)

# --- jobs / kill / wait / disown -----------------------------------------------------------------
add("jobs", "none", "jobs" + ST)
add("jobs", "running", "sleep 1 & jobs; kill %1; wait")
add("jobs", "running only", "sleep 1 & jobs -r; kill %1; wait")
add("jobs", "stopped only", "sleep 1 & jobs -s; echo st=$?; kill %1; wait")
add("jobs", "pids count", "sleep 1 & jobs -p | wc -l; kill %1; wait")
add("jobs", "spec missing", "jobs %3" + ST)
add("jobs", "invalid option", "jobs -z" + ST)
add("jobs", "finished job", "true & wait; jobs" + ST)
add("jobs", "two jobs marks", "sleep 1 & sleep 1 & jobs; kill %1 %2; wait")
add("jobs", "x option", "sleep 1 & jobs -x echo %1 | grep -c '^[0-9]'; kill %1; wait")
add("kill", "list number", "kill -l 9; kill -l 15")
add("kill", "list name", "kill -l KILL; kill -l SIGTERM")
add("kill", "list exit status", "kill -l 143")
add("kill", "invalid signal", "kill -s BOGUS 1" + ST)
add("kill", "signal name option", "{ " + "sleep 5 & kill -s TERM $!; wait $!" + ST + "; } 2>&1 | sed 's/[0-9][0-9]*/N/g'")
add("kill", "signal number option", "{ " + "sleep 5 & kill -n 9 $!; wait $!" + ST + "; } 2>/dev/null")
add("kill", "dash name", "{ " + "sleep 5 & kill -INT $!; wait $!" + ST + "; } 2>&1 | sed 's/[0-9][0-9]*/N/g'")
add("kill", "dash sig name", "{ " + "sleep 5 & kill -SIGHUP $!; wait $!" + ST + "; } 2>/dev/null")
add("kill", "job spec", "sleep 5 & kill %1; wait %1" + ST)
add("kill", "zero signal", "sleep 1 & kill -0 $!" + ST + "; kill %1; wait")
add("kill", "no such job", "kill %4" + ST)
add("kill", "no arguments", "kill" + ST)
add("kill", "invalid pid", "kill abc" + ST)
add("kill", "L option", "kill -L 2 | head -n 1")
add("kill", "list bad number", "kill -l 200" + ST)
add("wait", "no jobs", "wait" + ST)
add("wait", "status of job", "(exit 4) & wait $!" + ST)
add("wait", "job spec", "(exit 5) & wait %1" + ST)
add("wait", "n any", "(exit 6) & wait -n" + ST)
add("wait", "n no jobs", "wait -n" + ST)
add("wait", "not a child", "wait 99999" + ST)
add("wait", "no such job", "wait %3" + ST)
add("wait", "invalid id", "wait abc" + ST)
add("wait", "several", "(exit 1) & a=$!; (exit 2) & b=$!; wait $a $b" + ST)
add("wait", "already waited", "(exit 3) & p=$!; wait $p; wait $p" + ST)
add("disown", "no jobs", "disown" + ST)
add("disown", "job", "sleep 0.05 & disown %1; jobs" + ST + "; sleep 0.3")
add("disown", "all", "sleep 0.05 & sleep 0.05 & disown -a; jobs; echo end; sleep 0.3")
add("disown", "h keeps listing", "sleep 1 & disown -h %1; jobs | wc -l; kill %1; wait")
add("disown", "running only", "sleep 0.05 & disown -r; jobs | wc -l; sleep 0.3")
add("disown", "no such job", "disown %5" + ST)

# --- let ------------------------------------------------------------------------------------------
add("let", "assign", "let x=2*3; echo $x")
add("let", "several", "let a=1 b=a+1; echo $a $b")
add("let", "zero status", "let 0" + ST)
add("let", "nonzero status", "let 5" + ST)
add("let", "spaces quoted", "let 'y = 4 + 5'; echo $y")
add("let", "no arguments", "let" + ST)
add("let", "syntax error", "let '1 +'" + ST)
add("let", "division by zero", "let 1/0" + ST)
add("let", "increment", "i=1; let i++ ++i; echo $i")
add("let", "base notation", "let 'x = 16#ff + 2#10'; echo $x")
add("let", "comma operator", "let 'x = (1, 2)'; echo $x")
add("let", "ternary", "let 'x = 0 ? 1 : 2'; echo $x")
add("let", "invalid base", "let 'x = 99#1'" + ST)
add("let", "exponent negative", "let 'x = 2 ** -1'" + ST)

# --- logout / times -------------------------------------------------------------------------------
add("logout", "not login shell", "logout" + ST)
add("times", "status", "times >/dev/null" + ST)
add("times", "line count", "times | wc -l")
add("times", "extra argument", "times x >/dev/null" + ST)

# --- mapfile / readarray ---------------------------------------------------------------------------
for m in ["mapfile", "readarray"]:
    add(m, "lines", m + " a <<< $'x\\ny'; declare -p a")
    add(m, "strip", m + " -t a <<< $'x\\ny'; declare -p a")
    add(m, "count", "printf '1\\n2\\n3\\n' | { " + m + " -n 2 a; declare -p a; }")
    add(m, "origin", "a=(k k k); " + m + " -O 1 -t a <<< $'x\\ny'; declare -p a")
    add(m, "skip", m + " -s 1 -t a <<< $'x\\ny\\nz'; declare -p a")
    add(m, "delimiter", "printf 'a,b,c' | { " + m + " -d , -t a; declare -p a; }")
    add(m, "nul delimiter", "printf 'a\\0b\\0' | { " + m + " -d '' a; declare -p a; }")
    add(m, "descriptor", "printf 'q\\nr\\n' >/tmp/m; " + m + " -t -u 3 a 3</tmp/m; declare -p a")
    add(m, "default MAPFILE", m + " <<< $'m\\n'; declare -p MAPFILE")
    add(m, "callback", "printf 'a\\nb\\nc\\n' | { " + m + " -t -C 'echo cb' -c 2 a; declare -p a; }")
    add(m, "invalid array", m + " 1x <<< a" + ST)
    add(m, "bad descriptor", m + " -u 9 a" + ST)
    add(m, "invalid count", m + " -n x a <<< a" + ST)
    add(m, "no trailing newline", "printf 'a\\nb' | { " + m + " a; declare -p a; }")
    add(m, "empty input", m + " a </dev/null; declare -p a")
    add(m, "invalid quantum", m + " -c 0 -C echo a <<< a" + ST)

# --- read -----------------------------------------------------------------------------------------
for label, s in [
    ("default REPLY", "read <<< '  a b  '; echo \"[$REPLY]\""), ("fields", "read a b <<< 'x y z'; echo \"[$a][$b]\""),
    ("raw", "read -r a <<< 'a\\b'; echo \"$a\""), ("backslash processing", "read a <<< 'a\\b'; echo \"$a\""),
    ("continuation", "printf 'a\\\\\\nb\\n' | { read a; echo \"$a\"; }"), ("array", "read -a arr <<< 'p q r'; declare -p arr"),
    ("delimiter", "read -d , a <<< 'x,y'; echo \"$a\""), ("empty delimiter", "printf 'a\\nb\\0c' | { read -d '' a; echo \"$a\"; }"),
    ("n chars", "read -n 2 a <<< 'abcd'; echo \"$a\""), ("N chars", "read -N 3 a <<< $'a\\nbc'; echo \"$a\" | od -An -c"),
    ("n with fields", "read -n 3 a b <<< 'x yz'; echo \"[$a][$b]\""), ("silent", "read -s a <<< 'hid'; echo \"$a\""),
    ("prompt not shown without tty", "read -p 'prompt> ' a <<< 'v'; echo \"$a\""), ("timeout zero", "read -t 0 <<< 'x'" + ST),
    ("timeout with input", "read -t 1 a <<< 'x'; echo \"$a\""), ("descriptor", "exec 3<<<'fd3'; read -u 3 a; echo $a"),
    ("eof status", "read a </dev/null" + ST + "; echo \"[$a]\""), ("partial line status", "printf 'nonl' | { read a; echo \"$? $a\"; }"),
    ("custom IFS", "IFS=: read a b <<< 'x:y:z'; echo \"[$a][$b]\""), ("IFS whitespace trim", "IFS=' ' read a <<< '  pad  '; echo \"[$a]\""),
    ("empty IFS", "IFS= read a <<< '  pad  '; echo \"[$a]\""), ("invalid name", "read 1x <<< a" + ST),
    ("invalid option", "read -z a <<< a" + ST), ("bad descriptor", "read -u 9 a" + ST), ("invalid timeout", "read -t x a <<< a" + ST),
    ("invalid count", "read -n x a <<< a" + ST), ("i without e", "read -i def a <<< 'v'; echo \"$a\""), ("e without tty", "read -e a <<< 'v'; echo \"$a\""),
    ("array with IFS", "IFS=, read -ra arr <<< 'a,,b'; declare -p arr"), ("fewer fields", "read a b c <<< 'one'; echo \"[$a][$b][$c]\""),
    ("in while loop", "printf '1\\n2\\n' | while read l; do echo \"<$l>\"; done"), ("n zero", "read -n 0 a <<< 'x'" + ST + "; echo \"[$a]\""),
    ("delimiter multi-char uses first", "read -d ab a <<< 'xaby'; echo \"$a\""), ("N ignores delimiter", "read -N 4 -d b a <<< 'abcdef'; echo \"$a\""),
]:
    add("read", label, s)

# --- set ------------------------------------------------------------------------------------------
for label, s in [
    ("positional", "set -- a 'b c'; echo \"$# $2\""), ("clear positional", "set -- a; set --; echo $#"),
    ("dash turns off x and v", "set -xv; set -; echo \"$-\" | tr -d hBc"), ("dash dash with option-like", "set -- -x; echo $1"),
    ("o listing", "set -o"), ("plus o listing", "set +o"), ("o one option", "set -o errexit; set -o | grep errexit"),
    ("invalid option", "set -Q" + ST), ("invalid o name", "set -o bogus" + ST), ("errexit", "set -e; false; echo no"),
    ("nounset", "set -u; echo $nope; echo no"), ("xtrace", "set -x; echo hi; set +x"), ("verbose", "set -v; echo hi"),
    ("noglob", "cd /tmp; touch g1; set -f; echo g*"), ("noclobber", "echo a >/tmp/n; set -C; echo b >/tmp/n" + ST + "; cat /tmp/n"),
    ("noclobber override", "echo a >/tmp/n; set -C; echo b >|/tmp/n; cat /tmp/n"), ("allexport", "set -a; V=1; sh -c 'echo $V'"),
    ("noexec", "set -n; echo not run"), ("physical", "mkdir -p /tmp/r; cd /tmp; ln -s r l2; set -P; cd l2; pwd"),
    ("keyword", "set -k; f() { echo $A; }; f A=1"), ("hashall off", "set +h; echo $- | grep -c h"), ("braceexpand off", "set +B; echo {a,b}"),
    ("errtrace", "set -E; trap 'echo err' ERR; f() { false; }; f"), ("functrace", "set -T; trap 'echo dbg' DEBUG; f() { :; }; f 2>/dev/null | head -n 3"),
    ("pipefail", "set -o pipefail; false | true" + ST), ("posix", "set -o posix; shopt -qo posix && echo on"),
    ("monitor", "set -m" + ST), ("notify", "set -b" + ST), ("onecmd", "set -t; echo one; echo two"),
    ("histexpand", "set -H; echo 'a!b'"), ("emacs option", "set -o emacs" + ST), ("vi option", "set -o vi" + ST),
    ("ignoreeof", "set -o ignoreeof" + ST), ("interactive-comments off", "set +o interactive-comments; echo a #b"),
    ("privileged", "set -p" + ST), ("nolog", "set -o nolog" + ST), ("history option", "set -o history" + ST),
    ("dollar dash", "echo $- | tr -d c"), ("combined", "set -eu -o pipefail; echo $- | tr -d hBc"), ("plus combined", "set -eu; set +eu; echo $- | tr -d hBc"),
    ("set x with PS4", "PS4='+ '; set -x; : a"), ("set x nested level", "set -x; echo $(echo in)"), ("positional over nine", "set -- 1 2 3 4 5 6 7 8 9 ten; echo ${10} $10"),
    ("dash first word", "set - a b; echo $# $1"), ("o without argument", "set -o >/dev/null" + ST), ("plus o invalid", "set +o bogus" + ST),
]:
    add("set", label, s)

# --- shift ----------------------------------------------------------------------------------------
add("shift", "default", "set -- a b c; shift; echo \"$*\"")
add("shift", "count", "set -- a b c; shift 2; echo \"$*\"")
add("shift", "too many", "set -- a; shift 2" + ST + "; echo $#")
add("shift", "negative", "set -- a; shift -1" + ST)
add("shift", "non numeric", "set -- a; shift x" + ST)
add("shift", "zero", "set -- a b; shift 0; echo $#")
add("shift", "shift_verbose", "shopt -s shift_verbose; set --; shift" + ST)
add("shift", "in function", "f() { shift; echo \"$@\"; }; f a b c")
add("shift", "too many arguments", "set -- a b; shift 1 1" + ST)

# --- shopt ----------------------------------------------------------------------------------------
add("shopt", "print one", "shopt extglob")
add("shopt", "p one", "shopt -p nullglob")
add("shopt", "set and query", "shopt -s nullglob; shopt -q nullglob" + ST)
add("shopt", "query unset", "shopt -q failglob" + ST)
add("shopt", "query several", "shopt -s dotglob; shopt -q dotglob nullglob" + ST)
add("shopt", "invalid name", "shopt -s bogus" + ST)
add("shopt", "set and unset together", "shopt -s -u extglob" + ST)
add("shopt", "o options", "shopt -o errexit; shopt -so errexit; shopt -po errexit; set +e")
add("shopt", "list enabled", "shopt -s | wc -l")
add("shopt", "list all count", "shopt | wc -l")
add("shopt", "p all count", "shopt -p | wc -l")
add("shopt", "list all", "shopt")
add("shopt", "invalid option", "shopt -z" + ST)
add("shopt", "read only login_shell", "shopt -s login_shell" + ST)
add("shopt", "restricted_shell", "shopt restricted_shell")
GLOB = "mkdir -p /tmp/gl/sub/deep; cd /tmp/gl; touch a.txt B.txt .hidden sub/c.txt sub/deep/d.txt; "
for label, s in [
    ("nullglob", "shopt -s nullglob; echo x*; echo end"), ("failglob", "shopt -s failglob; echo x*; echo after"),
    ("dotglob", "shopt -s dotglob; echo *"), ("nocaseglob", "shopt -s nocaseglob; echo b*"), ("globstar", "shopt -s globstar; echo **/*.txt"),
    ("globstar off", "echo **/*.txt"), ("extglob", "shopt -s extglob\necho !(a*)"), ("extglob at", "shopt -s extglob\necho @(a|B).txt"),
    ("globskipdots", "shopt -s dotglob; echo .*"), ("globskipdots off", "shopt -u globskipdots; echo .*"),
    ("globasciiranges", "shopt -s globasciiranges; echo [a-z]*"), ("nocasematch", "shopt -s nocasematch; [[ ABC == abc ]] && echo m; case X in x) echo c;; esac"),
    ("lastpipe", "shopt -s lastpipe; set +m; echo v | read x; echo \"[$x]\""), ("expand_aliases off", "shopt -u expand_aliases; alias q='echo a'; q" + ST),
    ("xpg_echo", "shopt -s xpg_echo; echo 'a\\nb'"), ("sourcepath", "shopt sourcepath"), ("cdable_vars", "shopt -s cdable_vars; v=/tmp; cd v; pwd"),
    ("inherit_errexit", "shopt -s inherit_errexit; set -e; x=$(false; echo no); echo \"[$x]\""), ("inherit_errexit off", "set -e; x=$(false; echo yes); echo \"[$x]\""),
    ("patsub_replacement", "x=abc; echo \"${x/b/[&]}\"; shopt -u patsub_replacement; echo \"${x/b/[&]}\""),
    ("extquote", "x=a; echo \"${x:-$'t\\tb'}\" | od -An -c"), ("assoc_expand_once", "shopt -s assoc_expand_once; declare -A m; k='a]'; m[$k]=1; echo ${!m[@]}"),
    ("checkjobs", "shopt -s checkjobs" + ST), ("huponexit", "shopt -s huponexit" + ST), ("execfail", "shopt execfail"),
    ("gnu_errfmt", "shopt -s gnu_errfmt; cd /nosuch"), ("varredir_close", "shopt -s varredir_close; { echo x >&$fd; } {fd}>/tmp/v; cat /tmp/v"),
    ("nullglob with array", "shopt -s nullglob; a=(zz*); echo ${#a[@]}"), ("failglob in case", "shopt -s failglob; case x in zz*) ;; esac; echo ok"),
    ("dotglob with globstar", "shopt -s dotglob globstar; echo ** | tr ' ' '\\n' | head -n 4"), ("noexpand_translation", "shopt -s noexpand_translation; echo $\"hi\""),
    ("compat option", "shopt -s compat42" + ST), ("array_expand_once", "shopt -s array_expand_once" + ST), ("bash_source_fullpath", "shopt bash_source_fullpath" + ST),
    ("localvar_inherit off", "shopt localvar_inherit"), ("progcomp", "shopt progcomp"), ("hostcomplete", "shopt hostcomplete"),
    ("interactive_comments", "shopt interactive_comments"), ("mailwarn", "shopt mailwarn"), ("nocaseglob ranges", "shopt -s nocaseglob; echo [A-B]*"),
    ("dirspell", "shopt -s dirspell" + ST), ("cdspell", "shopt -s cdspell; cd /tm" + ST),
]:
    add("shopt", label, GLOB + s)

# --- trap -----------------------------------------------------------------------------------------
add("trap", "print", "trap 'echo x' EXIT; trap -p; trap - EXIT")
add("trap", "print one", "trap 'echo t' TERM; trap -p TERM")
add("trap", "print unset", "trap -p INT" + ST)
add("trap", "ignore", "trap '' HUP; trap -p HUP")
add("trap", "reset", "trap 'echo t' INT; trap - INT; trap -p INT; echo end")
add("trap", "reset with signal only", "trap 'echo t' INT; trap INT; trap -p INT; echo end")
add("trap", "invalid signal", "trap 'echo' BOGUS" + ST)
add("trap", "signal number", "trap 'echo n' 15; trap -p TERM")
add("trap", "sig prefix", "trap 'echo s' SIGINT; trap -p INT")
add("trap", "lowercase name", "trap 'echo s' int; trap -p INT" + ST)
add("trap", "several signals", "trap 'echo m' INT TERM; trap -p INT TERM")
add("trap", "exit number zero", "trap 'echo zero' 0; echo body")
add("trap", "no arguments", "trap" + ST)
add("trap", "invalid option", "trap -z" + ST)
add("trap", "list in subshell", "trap 'echo x' TERM; (trap -p TERM); echo end")
add("trap", "err", "trap 'echo err $?' ERR; false; true")
add("trap", "debug", "trap 'echo dbg' DEBUG; :; trap - DEBUG")
add("trap", "return", "f() { :; }; trap 'echo ret' RETURN; f; trap - RETURN")
add("trap", "kill self term", "trap 'echo caught' TERM; kill -TERM $$; echo after")
add("trap", "P option", "trap 'echo x' INT; trap -P INT" + ST)
add("trap", "dash dash", "trap -- 'echo dd' EXIT")
add("trap", "numeric exit listing", "trap 'echo x' 0; trap -p EXIT")

# Divergences that are decisions, not bugs.
EXPECTED = {
    'opt enable: load from file': (
        2, b'', b'bash: enable -f is unsupported in bash-tool\n',
        "fixture: WASI has no dynamic loading, so enable -f (a builtin from a shared object) is refused with bash-tool's canonical refusal (README, Commands)",
    ),
}
