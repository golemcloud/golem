"""Bash's own builtins, as a script uses them: options, statuses and error paths."""

CASES = [
    ("builtin: file tests on an empty path", "[ -e '' ] || echo e; [[ -d '' ]] || echo d; test -f '' || echo f"),
    # alias / unalias: aliases expand only with expand_aliases, and only on later lines.
    ("builtin: alias needs expand_aliases", "alias hi='echo hello'\nhi 2>/dev/null; echo status=$?"),
    ("builtin: alias expands on a later line", "shopt -s expand_aliases\nalias hi='echo hello'\nhi there", ["syntax.alias"]),
    ("builtin: alias on the same line does not expand", "shopt -s expand_aliases; alias hi='echo hello'; hi 2>/dev/null; echo status=$?", ["syntax.alias"]),
    ("builtin: alias listing and unalias", "alias a='echo a' b='echo b'; alias; unalias a; alias; unalias nosuch; echo status=$?", ["builtin.unalias"]),
    ("builtin: alias -p and missing alias", "alias x='ls -l'; alias -p; alias nosuch; echo status=$?"),
    # builtin / command / type / hash / enable
    ("builtin: builtin runs the builtin", "echo() { printf 'fn\\n'; }; echo x; builtin echo y; builtin nosuch; echo status=$?"),
    ("builtin: command bypasses functions", "ls() { echo fn; }; ls; command -v ls >/dev/null && echo found; command echo direct"),
    ("builtin: command -v and -V", "f() { :; }; command -v f cd echo; command -V f 2>&1 | head -n 1"),
    ("builtin: type -t forms", "f() { :; }; alias a=b; type -t f cd if a nosuch; echo status=$?", ["builtin.type"]),
    ("builtin: type output", "type cd; type if; f() { echo x; }; type f"),
    ("builtin: hash with no path", "hash; echo status=$?; hash -r; echo status=$?"),
    ("builtin: enable lists and disables", "enable -n echo; echo hi 2>/dev/null; echo status=$?; enable echo; echo back"),
    # caller, eval, source
    ("builtin: caller outside a function", "caller; echo status=$?"),
    ("builtin: caller inside a function", "f() { caller 0 >/dev/null; echo status=$?; }; f", ["param.callstack"]),
    ("builtin: eval builds commands", "x='echo'; y='hi there'; eval \"$x \\\"$y\\\"\"; eval 'a=1; b=2'; echo $((a+b))"),
    ("builtin: eval syntax error", "eval 'if'; echo status=$?", ["syntax.error"]),
    ("builtin: eval status and exit", "eval 'false'; echo status=$?; eval 'exit 3'; echo not-reached"),
    ("builtin: source a file with arguments", "printf 'echo \"sourced $1 $#\"; v=set\\n' >/tmp/lib.sh; source /tmp/lib.sh a b; echo $v; . /tmp/lib.sh"),
    ("builtin: source return status", "printf 'return 4\\necho no\\n' >/tmp/r.sh; . /tmp/r.sh; echo status=$?"),
    ("builtin: source a missing file", "source /tmp/nosuch.sh; echo status=$?"),
    # continue / break
    ("builtin: continue and break levels", "for i in 1 2 3; do for j in a b c; do [ $j = b ] && continue 2; [ $i = 3 ] && break 2; echo $i$j; done; done; echo end"),
    ("builtin: continue outside a loop", "continue; echo status=$?; break; echo status=$?"),
    ("builtin: break with a bad count", "for i in 1 2; do break 0; done; echo status=$?"),
    # declare / typeset / local / readonly
    ("builtin: declare -p forms", "declare -i n=5; declare -a a=(1 2); declare -A m=([k]=v); declare -r r=1; declare -x e=2; declare -p n a m r e", ["param.attributes", "param.array.assoc", "param.readonly"]),
    ("builtin: declare -i evaluates", "declare -i n; n=2+3*4; echo $n; n+=1; echo $n; n=abc; echo $n", ["param.attributes"]),
    ("builtin: declare -l and -u", "declare -l lo=HeLLo; declare -u up=HeLLo; echo $lo $up; lo=AGAIN; echo $lo", ["param.attributes"]),
    ("builtin: declare -f and -F", "f() { echo body; }; g() { :; }; declare -F; declare -f f"),
    ("builtin: declare -g inside a function", "f() { declare -g G=global; declare L=local; }; f; echo \"[$G] [${L-unset}]\""),
    ("builtin: typeset is declare", "typeset -i t=1+1; echo $t; typeset -p t", ["builtin.typeset"]),
    ("builtin: local outside a function", "local x=1; echo status=$?", ["error"]),
    ("builtin: local scoping is dynamic", "x=global; inner() { echo $x; }; outer() { local x=outer; inner; }; outer; echo $x", ["compound.function.local"]),
    ("builtin: local -", "f() { local -; set -f; echo *; }; f; set -o | grep -E '^noglob'"),
    ("builtin: readonly blocks assignment", "readonly r=1; r=2; echo status=$?; echo $r; unset r; echo status=$?", ["param.readonly"]),
    ("builtin: readonly -p", "readonly a=1 b; readonly -p | grep -E ' (a|b)'"),
    # dirs / pushd / popd
    ("builtin: pushd popd dirs", "cd /tmp; mkdir -p a b; pushd a >/dev/null; pushd /tmp/b; dirs; popd >/dev/null; pwd; popd; dirs -c; dirs", ["builtin.pushd", "builtin.popd", "builtin.dirs"]),
    ("builtin: popd on an empty stack", "popd; echo status=$?", ["builtin.popd"]),
    ("builtin: pushd to a missing directory", "pushd /nope; echo status=$?", ["builtin.pushd"]),
    # disown / times / jobs
    # wait skips a disowned job; the last sleep lets it end before the call does.
    ("builtin: disown a job", "sleep 0.05 & disown; jobs | wc -l; wait; echo status=$?; sleep 0.2", ["builtin.disown"]),
    ("builtin: disown with no job", "disown %5; echo status=$?", ["builtin.disown"]),
    ("builtin: times", "times >/dev/null; echo status=$?", ["builtin.times"]),
    # getopts
    ("builtin: getopts basic", "set -- -a -b val -c rest; while getopts ab:c o; do echo \"$o ${OPTARG-}\"; done; shift $((OPTIND-1)); echo \"rest=$*\"", ["builtin.shift"]),
    ("builtin: getopts clustered and attached", "set -- -abval -c; while getopts ab:c o; do echo \"$o ${OPTARG-}\"; done; echo OPTIND=$OPTIND"),
    ("builtin: getopts unknown option", "set -- -x; while getopts ab o; do echo \"got $o\"; done; echo status=$?"),
    ("builtin: getopts silent mode", "set -- -x -b; while getopts :ab: o; do echo \"$o ${OPTARG-}\"; done"),
    ("builtin: getopts missing argument", "set -- -b; while getopts b: o; do echo \"got $o\"; done"),
    ("builtin: getopts in a function", "f() { local OPTIND o; while getopts n: o \"$@\"; do echo \"$o=$OPTARG\"; done; }; f -n 1; f -n 2"),
    ("builtin: getopts stops at --", "set -- -a -- -b; while getopts ab o; do echo $o; done; shift $((OPTIND-1)); echo \"$@\""),
    # let
    ("builtin: let arithmetic and status", "let a=2*3 b=a+1; echo $a $b; let 0; echo status=$?; let 'c = 1 + 1'; echo $c"),
    # mapfile / readarray
    ("builtin: readarray options", "printf 'a\\nb\\nc\\nd\\n' >/tmp/f; readarray -t -s 1 -n 2 arr </tmp/f; declare -p arr; mapfile -t -O 5 arr </tmp/f; echo ${#arr[@]} ${arr[5]}", ["builtin.readarray"]),
    ("builtin: mapfile with a delimiter", "printf 'a,b,c' | { mapfile -d , -t parts; printf '[%s]' \"${parts[@]}\"; echo; }"),
    # shift / set --
    ("builtin: shift counts", "set -- a b c d; shift; echo \"$*\"; shift 2; echo \"$*\"; shift 5; echo status=$? \"$*\""),
    ("builtin: set -- and positional count", "set -- 'a b' c; echo $#; for x in \"$@\"; do echo \"[$x]\"; done; set --; echo $#"),
    # echo / printf builtin forms
    ("builtin: echo options", "echo -n no-newline; echo; echo -e 'a\\tb\\x41\\0101'; echo -E 'raw\\t'; echo -- -n"),
    ("builtin: printf -v", "printf -v out '%05d|%-4s|' 42 ab; echo \"$out\"; printf -v 'arr[1]' '%s' x; echo ${arr[1]}", ["cmd.printf"]),
    ("builtin: printf reuses its format", "printf '%s=%s\\n' a 1 b 2 c; printf '%b\\n' 'x\\ty'; printf '%q\\n' 'a b' \"it's\""),
    ("builtin: printf errors", "printf '%d\\n' 1.5; echo status=$?; printf '%z'; echo status=$?", ["cmd.printf", "error"]),
    # read
    ("builtin: read splits on IFS", "echo 'a b  c d' | { read x y z; echo \"[$x][$y][$z]\"; }; echo 'a:b:c' | { IFS=: read -r x y; echo \"[$x][$y]\"; }", ["expansion.word-splitting"]),
    ("builtin: read -r and backslashes", "printf 'a\\\\b\\n' | { read v; echo \"$v\"; }; printf 'a\\\\b\\n' | { read -r v; echo \"$v\"; }"),
    ("builtin: read -a -d -n -N", "echo 'x y z' | { read -a arr; echo ${#arr[@]} ${arr[2]}; }; printf 'ab;cd' | { read -d ';' v; echo $v; }; echo abcdef | { read -n 3 v; echo $v; }; printf 'a\\nb' | { read -N 3 v; printf '[%s]\\n' \"$v\"; }"),
    ("builtin: read at EOF", "printf 'last' | { read v; echo status=$? v=$v; }; : | { read v; echo status=$?; }"),
    ("builtin: while read loop", "printf '1 a\\n2 b\\n3 c\\n' | while read -r n l; do echo \"$l$n\"; done"),
    ("builtin: read -p to stderr", "echo x | { read -p 'prompt: ' v 2>/dev/null; echo $v; }"),
    ("builtin: read -u from a descriptor", "echo from3 >/tmp/f; exec 3</tmp/f; read -u 3 v; echo $v; exec 3<&-"),
    # set / shopt
    ("builtin: set -o listing", "set -o | grep -E '^(errexit|nounset|pipefail|noglob) ' "),
    ("builtin: set +o round trip", "set -e -u; set +o | grep -E 'errexit|nounset'"),
    ("builtin: set with a bad option", "set -o nosuchoption; echo status=$?", ["error"]),
    ("builtin: shopt query forms", "shopt -q extglob; echo status=$?; shopt -s nullglob; shopt nullglob; shopt -p nullglob; shopt -u nullglob; shopt -q nullglob; echo status=$?"),
    ("builtin: shopt unknown option", "shopt -s nosuchopt; echo status=$?", ["error"]),
    # test / [
    ("builtin: test string operators", "test -z '' && echo z; test -n x && echo n; [ a = a ] && echo eq; [ a != b ] && echo ne; [ a \\< b ] && echo lt; [ b \\> a ] && echo gt", ["builtin.test"]),
    ("builtin: test numeric operators", "[ 2 -eq 2 ] && [ 1 -lt 2 ] && [ 3 -ge 3 ] && [ 4 -ne 5 ] && echo ok; [ 1 -eq x ]; echo status=$?", ["builtin.test"]),
    # A directory's reported size (what -s checks) is filesystem metadata, not content, and
    # isn't portable even oracle-to-oracle (it's 0 on some backing filesystems, nonzero on
    # others) -- so -s is skipped for the directory entry; -e/-f/-d/-L cover it fully already.
    ("builtin: test file operators", "cd /tmp; : >e; echo x >s; mkdir -p d; ln -sf s l; for f in e s d l nope; do printf '%s:' $f; [ -e $f ] && printf e; [ -f $f ] && printf f; [ -d $f ] && printf d; [ \"$f\" != d ] && [ -s $f ] && printf s; [ -L $f ] && printf L; echo; done", ["builtin.test"]),
    ("builtin: test combined and negated", "[ ! -e /nope ] && echo absent; [ 1 -eq 1 -a 2 -eq 2 ] && echo and; [ 1 -eq 2 -o 2 -eq 2 ] && echo or; [ \\( 1 -eq 1 \\) ] && echo group"),
    ("builtin: test syntax errors", "[ 1 -eq ]; echo status=$?; [ a b c ]; echo status=$?; test; echo status=$?", ["builtin.test", "error"]),
    ("builtin: test newer and older", "cd /tmp; touch -d '2020-01-01' old; touch -d '2021-01-01' new; [ new -nt old ] && echo nt; [ old -ot new ] && echo ot; [ old -ef old ] && echo ef", ["builtin.test"]),
    # unset / return / exit / exec
    ("builtin: unset variables and functions", "x=1; f() { :; }; unset x; echo \"[${x-unset}]\"; unset -f f; f 2>/dev/null; echo status=$?; unset -v nosuch; echo status=$?"),
    ("builtin: unset an array element", "a=(1 2 3); unset 'a[1]'; echo ${#a[@]} ${!a[@]}", ["param.array.indexed"]),
    ("builtin: return outside a function", "return 3; echo status=$?"),
    ("builtin: return statuses", "f() { return 7; }; f; echo $?; g() { false; return; }; g; echo $?; h() { return 300; }; h; echo $?"),
    ("builtin: exit status wraps", "(exit 256); echo $?; (exit -1); echo $?"),
    ("builtin: exec a command ends the script", "echo before; exec echo replaced; echo not-reached"),
    ("builtin: exec with no command keeps going", "exec 3>/tmp/x; echo kept >&3; exec 3>&-; cat /tmp/x"),
    # true, false, :
    ("builtin: colon true false", ": ignored args; echo $?; true; echo $?; false; echo $?"),
    # wait / kill / jobs
    ("builtin: jobs listing forms", "sleep 0.1 & jobs >/dev/null; echo status=$?; jobs -l | wc -l; wait"),
    ("builtin: kill -l", "kill -l 15; kill -l TERM; kill -l 999; echo status=$?", ["builtin.kill"]),
    # compgen
    ("builtin: compgen word lists", "compgen -W 'alpha beta alps' al; compgen -W 'x y' z; echo status=$?", ["builtin.compgen"]),
    ("builtin: compgen variables", "myvar1=1 myvar2=2; compgen -v myvar", ["builtin.compgen"]),
]


REFUSED_GAP = 'refused (runtime-gaps): bash-tool refuses a feature it does not implement, up front when the script names it'
EXPECTED = {
    'builtin: enable lists and disables': (
        2, b'', b'bash: enable -n is unsupported in bash-tool\n', REFUSED_GAP,
    ),
}
