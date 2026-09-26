"""Grammar sweep: functions (definition forms, local and dynamic scope, return, printing with
declare -f), test and [[ ]] operators, getopts, declare/readonly/unset/type/command/builtin
(see sweep_grammar_param.py for the scheme)."""
import itertools

TIER = "sweep"

CASES = []
_NAMES = []


def _add(name, script, tags=()):
    name = "gram func " + name
    if name in _NAMES:
        raise ValueError("duplicate case name " + name)
    _NAMES.append(name)
    CASES.append((name, script, list(tags)))


# --- Definition forms -------------------------------------------------------------------------

BODIES = [
    ("brace group", "{ echo \"body $1\"; }"),
    ("subshell", "( echo \"sub $1\" )"),
    ("conditional", "[[ -n $1 ]]"),
    ("arithmetic", "(( $# > 0 ))"),
    ("if", "if [[ $1 ]]; then echo yes; else echo no; fi"),
    ("for", "for a in \"$@\"; do echo \"<$a>\"; done"),
    ("while", "while (( $# )); do echo \"$1\"; shift; done"),
    ("case", "case $1 in a) echo A;; *) echo other;; esac"),
    ("arith for", "for ((i=0; i<2; i++)); do echo $i; done"),
    ("select", "select s in x; do echo \"$s\"; break; done <<< 1"),
    ("group with redirect", "{ echo redirected; } > /tmp/fr"),
    ("multi-line group", "{\n  echo one\n  echo two\n}"),
]
FORMS = [
    ("parens", "f() BODY"),
    ("function keyword", "function f BODY"),
    ("function keyword parens", "function f() BODY"),
    ("spaced parens", "f ( ) BODY"),
]
for (bname, body), (fname, form) in itertools.product(BODIES, FORMS):
    if fname != "parens" and bname not in ("brace group", "subshell", "conditional", "if"):
        continue
    definition = form.replace("BODY", body)
    _add("define " + fname + " with " + bname,
         definition + "\nf a; echo \"status=$?\"; f; echo \"status=$?\"; [[ -f /tmp/fr ]] && cat /tmp/fr",
         ["compound.function"])
    # How bash prints each form back.
    _add("print " + fname + " with " + bname, definition + "\ndeclare -f f", ["compound.function", "builtin.declare"])

# declare -f reprints a canonical form: one case per construct inside a body.
PRINT_BODIES = [
    "echo a; echo b", "a=1 b=2", "x=1 echo y", "echo \"$1\" '$2' $'\\t'", "echo a | cat | cat",
    "echo a && echo b || echo c", "! true", "( echo sub )", "{ echo grp; }", "echo bg & wait",
    "if true; then echo t; elif false; then echo e; else echo f; fi", "while false; do :; done",
    "until true; do :; done", "for i in 1 2; do echo $i; done", "for ((i=0; i<1; i++)); do :; done",
    "case $1 in a|b) echo ab ;; c) echo c ;& d) echo d ;;& *) ;; esac", "[[ -n $1 && $1 == x* ]]",
    "(( x = 1 + 2 ))", "echo > /tmp/o 2>&1 < /dev/null", "cat <<EOF\nhere $1\nEOF", "cat <<'EOF'\nraw $1\nEOF",
    "cat <<< \"str\"", "exec 3>&-", "echo $(echo sub) `echo back`", "echo $((1+2))", "local l=1; declare -g g=2",
    "a=(1 2 3); a[4]=x", "declare -A m=([k]=v)", "echo ${v:-d} ${#v} ${v//a/b}", "echo {a,b} ~ *",
    "time true", "coproc_free=1", "echo 'it'\\''s'", "echo \"a\\\"b\"", "echo a # comment",
    "f2() { echo nested; }", "select s in a; do break; done", "echo <(true) >(true)", "{ echo x; } | { cat; }",
    "echo \\$ \\\\", "return 3", "echo a; # trailing comment\n  echo b", "[[ a =~ ^a(.)$ ]]",
    "test -n x && [ -z '' ]", "echo $@ $* $# $? $$ $!", "printf -v out '%s' x", "trap 'echo t' EXIT",
]
for index, body in enumerate(PRINT_BODIES):
    _add("print body " + str(index + 1) + ": " + body.split("\n")[0][:50],
         "f() {\n" + body + "\n}\ndeclare -f f", ["compound.function", "builtin.declare"])
for label, script in [
    ("type of a function", "f() { echo x; }; type f"),
    ("type -t kinds", "f() { :; }; type -t f if echo; type -t nosuch; echo status=$?"),
    ("declare -F", "f() { :; }; g() { :; }; declare -F; declare -F f"),
    ("declare -f of several", "a1() { echo 1; }; a2() { echo 2; }; declare -f a1 a2"),
    ("declare -f of a missing function", "declare -f nosuch; echo status=$?"),
    ("printed function reloads", "f() { local x=$1; echo \"[$x]\"; }; src=$(declare -f f); unset -f f; eval \"$src\"; f re"),
    ("function with redirect prints it", "f() { echo r; } > /tmp/fo; declare -f f"),
    ("export -f listing", "f() { :; }; export -f f; declare -Fx"),
    ("readonly function", "f() { echo ro; }; readonly -f f; f() { echo new; }; echo status=$?; f"),
    ("unset -f", "f() { :; }; unset -f f; f; echo status=$?"),
    ("unset prefers variable", "f=1; f() { echo func; }; unset f; f; echo \"[${f-unset}]\""),
    ("function named like a builtin", "echo() { builtin echo \"[$*]\"; }; echo hi; unset -f echo; echo plain"),
    ("function named like a keyword", "function if { echo kw; }; \\if 2>/dev/null; echo status=$?"),
    ("function with dashes and dots", "my-fn.x() { echo odd; }; my-fn.x"),
    ("function name with slash", "a/b() { echo slash; }; echo status=$?"),
    ("function name numeric", "1f() { echo num; }; 1f"),
    ("redefinition", "f() { echo 1; }; f() { echo 2; }; f"),
    ("function defined in a condition", "if true; then f() { echo cond; }; fi; f"),
    ("function defined in a loop", "for n in a b; do eval \"fn_$n() { echo $n; }\"; done; fn_a; fn_b"),
    ("recursive definition inside", "outer() { inner() { echo inner; }; }; inner 2>/dev/null; echo status=$?; outer; inner"),
    ("command skips functions", "echo() { builtin echo func; }; command echo cmd"),
    ("builtin with a non-builtin", "builtin nosuch; echo status=$?"),
    ("command -v kinds", "f() { :; }; command -v f; command -v if; command -v echo"),
    ("command -V function", "f() { echo x; }; command -V f"),
    ("function in a pipeline", "f() { read l; echo \"<$l>\"; }; echo piped | f"),
    ("function output captured", "f() { echo cap; }; x=$(f); echo \"$x\""),
    ("function with a here-string", "f() { cat; }; f <<< hs"),
    ("function redirect each call", "n=0; f() { echo call; } >> /tmp/fa; f; f; cat /tmp/fa"),
    ("function redirect with expansion", "f() { echo x; } > \"/tmp/f$n\"; n=1; f; n=2; f; echo /tmp/f?"),
]:
    _add(label, script, ["compound.function"])

# --- local, dynamic scope, return ------------------------------------------------------------

for label, script in [
    ("local shadows a global", "x=g; f() { local x=l; echo $x; }; f; echo $x"),
    ("dynamic scope reaches callees", "x=g; f() { local x=f; g; }; g() { echo $x; }; f; g"),
    ("callee assignment hits caller local", "x=g; f() { local x=f; g; echo $x; }; g() { x=changed; }; f; echo $x"),
    ("local without value", "x=g; f() { local x; echo \"[${x-unset}]\"; x=set; }; f; echo $x"),
    ("local inherits with -I", "x=g; f() { local -I x; echo \"[$x]\"; }; f"),
    ("local of an array", "a=(1 2); f() { local a=(x); echo ${a[@]}; }; f; echo ${a[@]}"),
    ("local -a and -A", "f() { local -a ia=(1 2); local -A aa=([k]=v); echo ${ia[1]} ${aa[k]}; }; f; echo \"[${ia-}]\""),
    ("local -i", "f() { local -i n=3+4; echo $n; }; f"),
    ("local -r", "f() { local -r c=1; c=2; echo status=$?; }; f"),
    ("local -x reaches children", "f() { local -x lx=child; bash -c 'echo ${lx-unset}'; }; f; echo ${lx-unset}"),
    ("local -n", "f() { local -n r=$1; r=set-by-ref; }; f target; echo $target"),
    ("local -", "set -u; f() { local -; set +u; echo ${nope-}inner; }; f; echo ${nope-outer}; ( echo $nope ) 2>/dev/null || echo still-u"),
    ("local twice", "f() { local x=1; local x=2; echo $x; }; f"),
    ("unset local reveals global", "x=g; f() { local x=l; unset x; echo \"[${x-unset}]\"; }; f"),
    ("unset caller local from callee", "x=g; f() { local x=f; g; echo \"f sees [${x-unset}]\"; }; g() { unset x; }; f"),
    ("declare in a function is local", "f() { declare d=in; }; f; echo \"[${d-unset}]\""),
    ("declare -g in a function", "f() { declare -g d=global; }; f; echo $d"),
    ("typeset is declare", "f() { typeset t=in; echo $t; }; f; echo \"[${t-unset}]\""),
    ("local with command substitution status", "f() { local x=$(false); echo $?; local y; y=$(false); echo $?; }; f"),
    ("local assignment splitting", "f() { v='a b'; local x=$v; echo \"$x\"; }; f"),
    ("local with array syntax", "f() { local 'arr=(1 2)'; declare -p arr; }; f"),
    ("positional parameters are per call", "set -- outer; f() { echo \"$# $1\"; set -- changed; }; f in; echo $1"),
    ("shift in a function", "f() { shift 2; echo \"$@\"; shift 5; echo status=$?; }; f a b c"),
    ("dollar zero in a function", "f() { echo $0; }; f"),
    ("FUNCNAME stack", "a() { b; }; b() { echo \"${FUNCNAME[*]}\"; }; a"),
    ("FUNCNAME at top level", "echo \"[${FUNCNAME[*]-}] ${#FUNCNAME[@]}\""),
    ("BASH_LINENO", "f() { echo \"${BASH_LINENO[0]}\"; }\nf\n\nf"),
    ("LINENO in a function", "f() {\necho $LINENO\n}\nf"),
    ("caller", "f() { caller; caller 0; }; g() { f; }\ng"),
    ("recursion factorial", "fact() { (( $1 <= 1 )) && { echo 1; return; }; echo $(( $1 * $(fact $(( $1 - 1 ))) )); }; fact 10"),
    ("recursion with locals", "fib() { local n=$1; (( n < 2 )) && { r=$n; return; }; fib $((n-1)); local a=$r; fib $((n-2)); r=$((a + r)); }; fib 15; echo $r"),
    ("recursion depth 200", "d() { (( $1 > 0 )) && d $(( $1 - 1 )) || echo bottom; }; d 200"),
    ("FUNCNEST limit", "FUNCNEST=3; r() { r; }; r; echo status=$?"),
    ("return value", "f() { return 7; }; f; echo $?"),
    ("return without value", "f() { false; return; }; f; echo $?"),
    ("return 256", "f() { return 256; }; f; echo $?"),
    ("return negative", "f() { return -1; }; f; echo $?"),
    ("return non-numeric", "f() { return abc; }; f; echo $?"),
    ("return too many arguments", "f() { return 1 2; }; f; echo $?"),
    ("return in a subshell of a function", "f() { ( return 4 ); echo \"after $?\"; }; f"),
    ("return in a loop", "f() { for i in 1 2 3; do (( i == 2 )) && return $i; done; }; f; echo $?"),
    ("return in a pipeline", "f() { echo x | return 5; echo \"after $?\"; }; f"),
    ("return from a nested function", "f() { g() { return 2; }; g; echo \"g=$?\"; return 1; }; f; echo $?"),
    ("function status is last command", "f() { true; false; }; f; echo $?"),
    ("function call with assignment prefix", "v=global; f() { echo $v; }; v=temp f; echo $v"),
    ("assignment prefix persists in posix mode", "set -o posix; f() { :; }; v=temp f; echo \"[${v-unset}]\""),
    ("export -f reaches bash -c", "f() { echo exported; }; export -f f; bash -c f"),
    ("function arguments with spaces", "f() { echo $#; for a; do echo \"<$a>\"; done; }; f 'a b' '' c"),
    ("dollar at and star in a function", "f() { IFS=-; echo \"$*\" \"$@\"; }; f 1 2 3"),
    ("RETURN trap per function", "f() { trap 'echo ret-f' RETURN; echo in-f; }; f; f"),
]:
    _add(label, script, ["compound.function", "compound.function.local"])

# --- test and [ ] --------------------------------------------------------------------------------

UNARY_TESTS = ["-n", "-z", "-e", "-f", "-d", "-s", "-L", "-h", "-v", "-a", "-o", "-t", "-p", "-S", "-b", "-c", "-R"]
OPERANDS = [("empty", "''"), ("word", "word"), ("file", "/tmp/t/file"), ("dir", "/tmp/t/dir"),
            ("empty file", "/tmp/t/empty"), ("link", "/tmp/t/link"), ("missing", "/tmp/t/missing"),
            ("set var", "sv"), ("array elem", "arr[1]"), ("fd", "1"), ("devnull", "/dev/null"),
            ("option name", "errexit")]
TSETUP = "mkdir -p /tmp/t/dir; echo x > /tmp/t/file; : > /tmp/t/empty; ln -s file /tmp/t/link; sv=1; arr=(a b); "
for op in UNARY_TESTS:
    for oname, operand in OPERANDS:
        if (len(op) * 7 + len(oname)) % 5 and op not in ("-e", "-n"):
            continue
        if op == "-s" and oname == "dir":
            continue  # a directory's size depends on the filesystem (builtins.py fixture)
        _add("test " + op + " " + oname,
             TSETUP + "test " + op + " " + operand + "; echo \"test=$?\"; [ " + op + " " + operand + " ]; echo \"bracket=$?\"; [[ " + op + " " + operand + " ]]; echo \"cond=$?\"",
             ["builtin.test", "compound.cond"])

BINARY_TESTS = ["=", "==", "!=", "<", ">", "-eq", "-ne", "-lt", "-le", "-gt", "-ge", "-nt", "-ot", "-ef", "=~"]
PAIRS = [("equal words", "abc", "abc"), ("different words", "abc", "abd"), ("numbers", "10", "9"),
         ("negative numbers", "-1", "-01"), ("empty left", "''", "x"), ("non-numeric", "a", "1"),
         ("spaces in number", "' 5'", "'5 '"), ("files", "/tmp/t/file", "/tmp/t/link"),
         ("glob right", "abc", "'a*'"), ("case", "B", "a")]
for op in BINARY_TESTS:
    for pname, left, right in PAIRS:
        if pname not in ("equal words", "numbers") and PAIRS.index((pname, left, right)) % 8 != BINARY_TESTS.index(op) % 8:
            continue
        quoted = "\\" + op if op in ("<", ">") else op
        _add("test binary " + op + " " + pname,
             TSETUP + "[ " + left + " " + quoted + " " + right + " ]; echo \"bracket=$?\"; [[ " + left + " " + op + " " + right + " ]]; echo \"cond=$?\"",
             ["builtin.test", "compound.cond"])

for label, args in [
    ("no arguments", ""), ("one empty", "''"), ("one word", "x"), ("one dash", "-"), ("bang", "!"),
    ("bang empty", "! ''"), ("bang word", "! x"), ("two words", "a b"), ("three words and", "a -a ''"),
    ("three words or", "'' -o b"), ("parens", "\\( x \\)"), ("parens with op", "\\( a = a \\)"),
    ("bang bang", "! ! x"), ("four args", "! a = b"), ("five args", "\\( a = a \\) -a x"),
    ("precedence and or", "x -o '' -a ''"), ("equals as operand", "= = ="), ("dash n alone", "-n"),
    ("dash z alone", "-z"), ("missing bracket", "MISSING"), ("unknown binary", "a -xx b"),
    ("integer error", "a -eq 1"), ("too many", "a b c d e f"), ("-a as unary", "-a /tmp"),
    ("-o option", "-o errexit"), ("-o unknown option", "-o nosuch"), ("-v array", "-v 'arr[1]'"),
    ("-v missing element", "-v 'arr[9]'"), ("-R not nameref", "-R sv"),
]:
    if args == "MISSING":
        script = "[ a = a; echo \"status=$?\""
    else:
        script = TSETUP + "test " + args + "; echo \"test=$?\"; [ " + args + " ]; echo \"bracket=$?\""
    _add("test forms " + label, script, ["builtin.test", "builtin.["])

# [[ ]] specifics.
for label, script in [
    ("and or precedence", "[[ -n '' || -n x && -z '' ]] && echo t"),
    ("parentheses", "[[ ( -n x || -n '' ) && ! -z x ]] && echo t"),
    ("negation of a group", "[[ ! ( a == b ) ]] && echo t"),
    ("no word splitting", "v='a b'; [[ $v == 'a b' ]] && echo t"),
    ("no globbing", "cd /tmp; : > gg; [[ g* == g* ]] && echo t; [[ gg == g* ]] && echo pat"),
    ("quoted right side is literal", "[[ abc == \"a*\" ]] || echo literal; [[ 'a*' == \"a*\" ]] && echo eq"),
    ("variable pattern", "p='a*'; [[ abc == $p ]] && echo glob; [[ abc == \"$p\" ]] || echo lit"),
    ("single equals", "[[ abc = a* ]] && echo t"),
    ("empty operands", "[[ '' ]] || echo empty-false; [[ x ]] && echo word-true"),
    ("unset variable", "unset u; [[ $u ]] || echo f; [[ -z $u ]] && echo z"),
    ("string less than", "[[ a < b ]] && echo lt; [[ B < a ]] && echo upper-first"),
    ("numeric strings compare as strings", "[[ 10 < 9 ]] && echo string-order"),
    ("arithmetic operands", "x=4; [[ x -eq 4 ]] && echo t; [[ 1+1 -eq 2 ]] && echo t2"),
    ("-eq with a bad expression", "[[ a+ -eq 1 ]]; echo status=$?"),
    ("newlines inside", "[[ a == a &&\n b == b ]] && echo t"),
    ("regex match", "[[ abc123 =~ ^([a-z]+)([0-9]+)$ ]] && echo \"${BASH_REMATCH[0]} ${BASH_REMATCH[1]} ${BASH_REMATCH[2]}\""),
    ("regex no match clears", "[[ ab =~ (a) ]]; [[ xy =~ (a) ]]; echo \"${#BASH_REMATCH[@]}\""),
    ("regex quoted is literal", "[[ a.c =~ \"a.c\" ]] && echo t; [[ abc =~ \"a.c\" ]] || echo lit"),
    ("regex from a variable", "re='^a.c$'; [[ abc =~ $re ]] && echo t"),
    ("regex partial quoting", "[[ 'a.c' =~ a\".\"c ]] && echo t; [[ abc =~ a\".\"c ]] || echo lit-dot"),
    ("regex with spaces", "[[ 'a b' =~ a\\ b ]] && echo t; re='a b'; [[ 'a b' =~ $re ]] && echo t2"),
    ("regex invalid", "[[ a =~ ( ]]; echo status=$?"),
    ("regex anchors and classes", "[[ x9 =~ ^[[:alpha:]][[:digit:]]$ ]] && echo t"),
    ("regex alternation", "[[ dog =~ ^(cat|dog)$ ]] && echo \"${BASH_REMATCH[1]}\""),
    ("regex optional group", "[[ ac =~ a(b)?c ]] && echo \"[${BASH_REMATCH[1]}] ${#BASH_REMATCH[@]}\""),
    ("regex backreference", "[[ aa =~ (a)\\1 ]] && echo t; echo status=$?"),
    ("regex bracket with paren", "[[ '(' =~ [(] ]] && echo t"),
    ("regex unicode", "[[ é =~ ^.$ ]] && echo one-char"),
    ("regex BASH_REMATCH readonly", "[[ a =~ a ]]; BASH_REMATCH=x; echo status=$? ${BASH_REMATCH[0]}"),
    ("regex nocasematch", "shopt -s nocasematch; [[ ABC =~ ^abc$ ]] && echo ci"),
    ("regex quoted sets BASH_REMATCH", "[[ abc =~ \"b\" ]]; echo $? \"${BASH_REMATCH[0]}\" ${#BASH_REMATCH[@]}; [[ abc =~ '' ]]; echo $? ${#BASH_REMATCH[@]}"),
    ("regex quoted then unquoted", "[[ abc =~ 'a'.c ]]; echo $? \"${BASH_REMATCH[0]}\"; [[ abc =~ \"a\"(b)\"c\" ]]; echo $? \"${BASH_REMATCH[1]}\"; [[ abc =~ \"b\"* ]]; echo $? \"[${BASH_REMATCH[0]}]\""),
    ("regex quoted nocasematch", "shopt -s nocasematch; [[ ABC =~ \"b\" ]]; echo $? \"${BASH_REMATCH[0]}\""),
    ("regex xtrace shows the pattern", "set -x; [[ abc =~ \"a.c\" ]]; x=b; [[ abc =~ $x ]]; [[ abc =~ 'a'.c ]]"),
    ("-v on arrays", "a=(x); declare -A m=([k]=v); [[ -v a ]] && echo a; [[ -v a[0] ]] && echo a0; [[ -v m[k] ]] && echo mk; [[ -v m ]] || echo m-no-zero"),
    ("-v positional", "set -- a; [[ -v 1 ]] && echo one; [[ -v 2 ]] || echo no-two"),
    ("file tests with dev paths", "[[ -e /dev/null ]] && echo e; [[ -c /dev/null ]] && echo c; [[ -f /dev/null ]] || echo not-f"),
    ("-ef same file", "echo x > /tmp/a1; [[ /tmp/a1 -ef /tmp/../tmp/a1 ]] && echo same"),
    ("-nt missing file", "echo x > /tmp/a2; [[ /tmp/a2 -nt /tmp/missing ]] && echo newer; [[ /tmp/missing -ot /tmp/a2 ]] && echo older"),
    ("syntax error", "[[ a == ]]"),
    ("bad unary", "[[ -Q x ]]"),
    ("as a command argument", "echo [[ a ]]"),
    ("status in a pipeline", "[[ a == b ]] | true; echo ${PIPESTATUS[0]}"),
    ("with set -x", "set -x; [[ a == a && -n x ]]"),
]:
    _add("cond " + label, script, ["compound.cond"])

# --- getopts ----------------------------------------------------------------------------------

GETOPTS_SPECS = ["ab:c", ":ab:c", "a", ":a", "ab::", "x:y:"]
GETOPTS_ARGS = [
    "-a -b val -c", "-ab val", "-abval", "-c -a", "-z", "-b", "-a -- -b x", "-a file -c", "", "-", "--",
    "-a-b", "-b -a", "-aaa", "-x 1 -y 2", "-y",
]
for spec, args in itertools.product(GETOPTS_SPECS, GETOPTS_ARGS):
    if (len(spec) + len(args)) % 3:
        continue
    _add("getopts '" + spec + "' " + (args or "no arguments"),
         "set -- " + args + "; while getopts '" + spec + "' o; do echo \"o=$o OPTARG=${OPTARG-unset} OPTIND=$OPTIND\"; done; echo \"end $? OPTIND=$OPTIND rest=$*\"",
         ["builtin.getopts"])
for label, script in [
    ("explicit arguments", "while getopts 'a:' o -a one extra; do echo \"$o $OPTARG\"; done; echo $OPTIND"),
    ("reset OPTIND", "set -- -a; getopts a o; echo $o $OPTIND; OPTIND=1; getopts a o; echo $o $OPTIND"),
    ("in a function", "f() { local OPTIND o; while getopts 'v' o; do echo \"flag $o\"; done; }; f -v; f -v"),
    ("OPTERR zero", "OPTERR=0; set -- -z; getopts a o; echo \"$o ${OPTARG-unset}\""),
    ("invalid variable name", "set -- -a; getopts a 1bad; echo status=$?"),
    ("no spec", "getopts; echo status=$?"),
    ("digits as options", "set -- -1 -2; while getopts 12 o; do echo $o; done"),
    ("colon option", "set -- -:; getopts ':a' o; echo \"$o ${OPTARG-}\""),
    ("question mark option", "set -- '-?'; getopts 'a' o; echo \"$o ${OPTARG-unset}\""),
]:
    _add("getopts " + label, script, ["builtin.getopts"])

# --- declare, readonly, unset, export -----------------------------------------------------------

ATTRS = ["-i", "-l", "-u", "-r", "-x", "-a", "-A", "-n", "-t", "-il", "-ux", "-ai", "-Al", "-ri"]
VALUES = [("none", None), ("text", "MiXed"), ("number expression", "3+4"), ("compound", "(1 2)"),
          ("assoc compound", "([k]=v)")]
for attr in ATTRS:
    for vname, value in VALUES:
        if (len(attr) + len(vname)) % 2:
            continue
        assign = "v" if value is None else "v=" + value
        _add("declare " + attr + " " + vname,
             "declare " + attr + " " + assign + "; echo \"status=$?\"; declare -p v; v=Next; declare -p v",
             ["builtin.declare", "param.attributes"])
for label, script in [
    ("declare -p of several kinds", "a=1; b=(x 'y z'); declare -A c=([k]='v w'); declare -i d=3; declare -p a b c d"),
    ("declare -p quoting", "v=$'it\\'s \"q\" $x \\\\ \\t'; declare -p v"),
    ("declare -p unset", "declare -p nosuch; echo status=$?"),
    ("declare -p declared without value", "declare d; declare -p d; declare -a e; declare -p e"),
    ("declare -p after unset element", "a=(1 2 3); unset 'a[1]'; declare -p a"),
    ("declare +x", "export e=1; declare +x e; declare -p e"),
    ("declare -r then assignment", "declare -r r=1; r=2; echo status=$?; declare -p r"),
    ("readonly listing", "readonly r1=1; readonly -p | while read -r l; do [[ $l == *r1* ]] && echo \"$l\"; done"),
    ("readonly array", "readonly -a ra=(1 2); ra[0]=x; echo status=$?; declare -p ra"),
    ("readonly unset", "readonly r=1; unset r; echo status=$?"),
    ("readonly without value", "readonly rv; rv=1; echo status=$?; declare -p rv"),
    ("unset -v", "v=1; unset -v v; echo \"[${v-unset}]\""),
    ("unset several", "a=1 b=2; unset a b nosuch; echo \"[${a-u}${b-u}] $?\""),
    ("unset an invalid name", "unset 1x; echo status=$?"),
    ("unset a special parameter", "unset 1; echo status=$?; unset '?'; echo status=$?"),
    ("unset IFS default", "IFS=:; unset IFS; v='a b'; set -- $v; echo $#"),
    ("unset PATH is fine", "unset PATH; echo still; echo status=$?"),
    ("export listing", "export ex1=v; export -p | while read -r l; do [[ $l == *ex1* ]] && echo \"$l\"; done"),
    ("export without value", "export ev; declare -p ev; bash -c 'echo \"[${ev-unset}]\"'"),
    ("export an invalid name", "export 'a-b=1'; echo status=$?"),
    ("export -n", "export en=1; export -n en; bash -c 'echo \"[${en-unset}]\"'"),
    ("typeset -p", "typeset -i t=4; typeset -p t"),
    ("declare conflicting options", "declare -a -A v; echo status=$?"),
    ("declare -u then -l", "declare -u v=abc; declare -l v; v=XyZ; echo $v"),
    ("declare -l -u together", "declare -lu v=MiX; echo $v"),
    ("declare -t", "declare -t tv=1; declare -p tv"),
    ("declare with += on integer", "declare -i n=1; declare n+=4; echo $n"),
    ("declare += on array", "a=(1); declare a+=(2); echo ${a[@]}"),
    ("declare -g at top level", "declare -g gv=1; echo $gv"),
    ("declare -x reaches child", "declare -x dx=1; bash -c 'echo ${dx-unset}'"),
    ("declare in a subshell", "( declare sd=1 ); echo \"[${sd-unset}]\""),
    ("declare invalid option", "declare -Z v; echo status=$?"),
    ("declare name with subscript and value", "declare 'a[2]=x'; declare -p a"),
    ("declare assignment splitting", "v='1 2'; declare -a arr=($v); declare -p arr"),
]:
    _add("declare " + label, script, ["builtin.declare"])

NESTING = (
    "bash-tool's limit (README, Limits: Nesting): function calls and `$( )` share Wasmtime's 512 KiB "
    "native stack, so recursion ends with the nesting error (about 75 levels through an `if`, 150 "
    "bare, 50 through `$( )`); bash has no such limit"
)
EXPECTED = {
    "gram func recursion depth 200": (
        1, b"", b"bash: line 1: d: maximum function nesting level exceeded (150): deeper nesting is "
        b"unsupported in bash-tool\n", NESTING,
    ),
}
