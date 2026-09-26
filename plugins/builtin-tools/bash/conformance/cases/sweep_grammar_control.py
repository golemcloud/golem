"""Grammar sweep: lists and statuses, if, case, loops with break/continue, subshells and groups,
errexit/nounset/pipefail, exit, eval, source, aliases and xtrace (see sweep_grammar_param.py)."""
import itertools

TIER = "sweep"


class _Rng:
    """A 64-bit linear congruential generator with a fixed seed."""

    def __init__(self, seed):
        self.state = seed & 0xFFFFFFFFFFFFFFFF

    def next(self):
        self.state = (self.state * 6364136223846793005 + 1442695040888963407) & 0xFFFFFFFFFFFFFFFF
        return self.state >> 33

    def below(self, n):
        return self.next() % n

    def shuffled(self, seq):
        items = list(seq)
        for i in range(len(items) - 1, 0, -1):
            j = self.below(i + 1)
            items[i], items[j] = items[j], items[i]
        return items


def _pairwise(dims, seed):
    """Index tuples over `dims` covering every pair of values of two dimensions at least once."""
    rng = _Rng(seed)
    combos = rng.shuffled(itertools.product(*[range(len(d)) for d in dims]))
    covered = []
    chosen = []
    for combo in combos:
        pairs = []
        for a in range(len(combo)):
            for b in range(a + 1, len(combo)):
                pairs.append((a, combo[a], b, combo[b]))
        new = [p for p in pairs if p not in covered]
        if new:
            covered.extend(new)
            chosen.append(combo)
    chosen.sort()
    return [tuple(dims[k][i] for k, i in enumerate(c)) for c in chosen]


CASES = []
_NAMES = []


def _add(name, script, tags=()):
    name = "gram control " + name
    if name in _NAMES:
        raise ValueError("duplicate case name " + name)
    _NAMES.append(name)
    CASES.append((name, script, list(tags)))


# --- Lists: &&, ||, ;, | and ! ---------------------------------------------------------------

UNITS = ["true", "false", "(exit 3)", "! false", "echo o"]
LIST_OPS = [" && ", " || ", "; ", " | "]
for c1, op1, c2, op2, c3 in _pairwise([UNITS, LIST_OPS, UNITS, LIST_OPS, UNITS], 41):
    script = c1 + op1 + c2 + op2 + c3
    _add("list " + script, script + "; echo \"status=$? pipe=${PIPESTATUS[*]}\"", ["compound.list"])
for units in [("false", "true"), ("true", "false"), ("(exit 2)", "(exit 3)")]:
    for neg in ("", "! "):
        for fail in ("", "set -o pipefail; "):
            _add("pipeline status " + fail + neg + " | ".join(units),
                 fail + neg + units[0] + " | " + units[1] + "; echo \"status=$? pipe=${PIPESTATUS[*]}\"",
                 ["compound.pipeline", "option.pipefail"])

# --- if / elif / else -------------------------------------------------------------------------

CONDS = ["true", "false", "[[ -n x ]]", "(( 0 ))", "! true", "false || true", "true && false",
         "{ false; true; }", "(exit 7)", "cond_fn", "false | true", "[ ]"]
SHAPES = [
    ("if", "if C1; then echo then; fi"),
    ("if else", "if C1; then echo then; else echo else; fi"),
    ("if elif", "if C1; then echo then; elif C2; then echo elif; fi"),
    ("if elif else", "if C1; then echo then; elif C2; then echo elif; else echo else; fi"),
    ("nested", "if C1; then if C2; then echo inner; fi; else echo outer-else; fi"),
    ("newlines", "if C1\nthen\n  echo then\nelif C2\nthen\n  echo elif\nfi"),
]
IF_CASES = [(shape, c1, "") for shape in SHAPES[:2] for c1 in CONDS]
IF_CASES += _pairwise([SHAPES[2:], CONDS, ["true", "false", "cond_fn", "(exit 7)"]], 43)
for (sname, shape), c1, c2 in IF_CASES:
    body = shape.replace("C1", c1).replace("C2", c2)
    _add("if " + sname + ": " + c1 + (" / " + c2 if c2 else ""),
         "cond_fn() { return 1; }; " + body + "\necho \"status=$?\"", ["compound.if"])
for label, script in [
    ("status when no branch runs", "if false; then :; fi; echo $?"),
    ("status is the body's", "if true; then (exit 4); fi; echo $?"),
    ("status of a failed elif chain", "if false; then :; elif false; then :; fi; echo $?"),
    ("condition output", "if echo cond; then echo body; fi"),
    ("redirect on if", "if true; then echo in; fi > /tmp/o; cat /tmp/o"),
    ("if in a pipeline", "if true; then echo piped; fi | cat"),
    ("if as a function body", "f() if [[ $1 ]]; then echo arg; else echo none; fi; f x; f"),
    ("if with a background condition", "if true & wait; then echo bg; fi"),
    ("if with a list condition", "if false; true; then echo last-counts; fi"),
    ("if with a negated group", "if ! { true; }; then echo t; else echo e; fi"),
]:
    _add("if " + label, script, ["compound.if"])

# --- case -------------------------------------------------------------------------------------

TERMS = [";;", ";&", ";;&"]
CASE_PATTERNS = ["a*", "*b", "ab|cd", "?", "[ab]*", "*", "'ab'", "\"$pv\"", "$pv", "@(x)"]
CASE_WORDS = ["ab", "cd", "x", "", "a b", "*"]
for t1, t2, t3, word in _pairwise([TERMS, TERMS, TERMS, CASE_WORDS], 47):
    script = ("pv='a*'; case '" + word + "' in\n  a*) echo one " + t1 + "\n  *b|x) echo two " + t2
              + "\n  '') echo empty " + t3 + "\n  *) echo default ;;\nesac; echo \"status=$?\"")
    _add("case terminators " + t1 + " " + t2 + " " + t3 + ": word '" + word + "'", script,
         ["compound.case", "compound.case.fallthrough"])
for pattern in CASE_PATTERNS:
    if pattern == "@(x)":
        continue
    for word in CASE_WORDS:
        if (len(pattern) + len(word)) % 2:
            continue
        _add("case pattern " + pattern + ": word '" + word + "'",
             "pv='a*'; case '" + word + "' in " + pattern + ") echo matched;; *) echo no;; esac",
             ["compound.case"])
for label, script in [
    ("no match status", "case x in y) echo y;; esac; echo $?"),
    ("status is the body's", "case x in x) (exit 3);; esac; echo $?"),
    ("empty body", "case x in x) ;; esac; echo $?"),
    ("leading parenthesis", "case x in (x) echo paren;; esac"),
    ("last clause without terminator", "case x in x) echo last\nesac"),
    ("newlines everywhere", "case x\nin\nx)\necho nl\n;;\nesac"),
    ("pattern from command substitution", "case abc in $(echo 'a*')) echo sub;; esac"),
    ("pattern with escaped star", "case '*' in \\*) echo star;; esac; case x in \\*) echo no;; *) echo other;; esac"),
    ("word with glob characters is literal", "cd /tmp; : > f1; case f1 in f*) echo g;; esac; case 'f*' in f1) echo no;; *) echo lit;; esac"),
    ("word expansion once", "n=0; case $((n+=1)) in 1) echo one;; esac; echo $n"),
    ("pattern expanded lazily", "case a in a) echo a;; $(echo side >&2)) echo no;; esac"),
    ("fallthrough to the end", "case a in a) echo a ;& b) echo b ;& esac"),
    ("test next skips non-matches", "case ab in a*) echo 1 ;;& x*) echo 2 ;;& *b) echo 3 ;;& esac"),
    ("in a pipeline", "echo k | case $(cat) in k) echo key;; esac"),
    ("redirect on case", "case x in x) echo r;; esac > /tmp/c; cat /tmp/c"),
    ("keyword words as patterns", "case if in if) echo kw;; esac; case esac in esac) echo es;; esac"),
    ("pattern list with spaces", "case b in a | b ) echo spaced;; esac"),
    ("nocasematch", "shopt -s nocasematch; case ABC in abc) echo ci;; esac"),
    ("character classes", "case 5 in [[:digit:]]) echo digit;; esac; case ' ' in [[:space:]]) echo space;; esac"),
    ("negated bracket", "case z in [!a-y]) echo not;; esac"),
    ("bracket with dash first", "case - in [-a]) echo dash;; esac"),
    ("empty pattern", "case '' in '') echo empty;; esac"),
    ("unset word", "unset u; case $u in '') echo unset-empty;; esac"),
    ("dollar at word", "set -- a b; case $@ in 'a b') echo joined;; esac"),
    ("array word", "arr=(p q); case ${arr[@]} in 'p q') echo arr;; esac"),
    ("esac as a word", "echo esac; case x in x) echo esac;; esac"),
    ("case inside a loop with break", "for i in 1 2 3; do case $i in 2) break;; esac; echo $i; done"),
    ("case inside a function with return", "f() { case $1 in r) return 5;; esac; echo no-return; }; f r; echo $?; f s"),
]:
    _add("case " + label, script, ["compound.case"])

# --- Loops, break and continue ---------------------------------------------------------------

OUTER = [
    ("for", "for i in 1 2 3; do", "done"),
    ("while", "i=0; while (( i++ < 3 )); do", "done"),
    ("until", "i=0; until (( i++ >= 3 )); do", "done"),
    ("arith for", "for (( i=1; i<=3; i++ )); do", "done"),
]
INNER = [
    ("for", "for j in a b c; do", "done"),
    ("while", "j=0; while (( j++ < 3 )); do", "done"),
    ("select", "select j in a b c; do", "done <<< $'1\\n2\\n3'"),
]
JUMPS = ["break", "break 2", "continue", "continue 2", "break 5", "continue 9", "break 0", "continue -1",
         "break x", "return", "exit 4"]
WHERE = ["first", "second"]
for outer, inner, jump, where in _pairwise([OUTER, INNER, JUMPS, WHERE], 53):
    cond = "[[ $j == " + ("a" if where == "first" else "b") + " || $j == " + ("1" if where == "first" else "2") + " ]] && " + jump
    script = (outer[1] + " " + inner[1] + " " + cond + "; printf '%s%s ' \"$i\" \"$j\"; " + inner[2]
              + "; echo \"|$i\"; " + outer[2] + "; echo \"end status=$?\"")
    _add("loop " + outer[0] + " around " + inner[0] + ": " + jump + " at " + where, script,
         ["compound.for", "compound.while", "builtin.break", "builtin.continue"])
for label, script in [
    ("break outside a loop", "break; echo status=$?"),
    ("continue outside a loop", "continue; echo status=$?"),
    ("break in a function called from a loop", "f() { break; }; for i in 1 2; do f; echo $i; done; echo end"),
    ("continue in a function called from a loop", "f() { continue; }; for i in 1 2; do f; echo $i; done; echo end"),
    ("break in a subshell inside a loop", "for i in 1 2; do (break); echo $i; done"),
    ("break in a pipeline inside a loop", "for i in 1 2; do echo x | break; echo $i; done"),
    ("break inside eval", "for i in 1 2; do eval break; echo $i; done; echo end"),
    ("loop status with no iterations", "false; for i in; do :; done; echo $?"),
    ("while status with false body", "i=0; while (( i++ < 2 )); do false; done; echo $?"),
    ("while condition list", "i=0; while echo c$i; (( i++ < 1 )); do echo b; done"),
    ("until status", "until true; do :; done; echo $?"),
    ("for without in uses positionals", "set -- p 'q r'; for a; do echo \"[$a]\"; done"),
    ("for without in inside a function", "f() { for a; do echo \"$a\"; done; }; f x y"),
    ("for with a semicolon before do", "for a in 1 2; do echo $a; done; for b; do echo never; done"),
    ("for with newline before in", "for a\nin x y\ndo echo $a\ndone"),
    ("for with braces body", "for a in 1 2; { echo b$a; }"),
    ("for loop variable after the loop", "for a in 1 2 3; do :; done; echo $a"),
    ("for over empty expansion", "e=; for a in $e; do echo never; done; echo after"),
    ("for over quoted empty", "for a in \"\"; do echo \"[$a]\"; done"),
    ("for variable is not local", "f() { for v in in-f; do :; done; }; v=out; f; echo $v"),
    ("for with a readonly variable", "readonly ro=1; for ro in 2; do echo $ro; done; echo status=$?"),
    ("for with an invalid name", "for 1x in a; do echo $1x; done; echo status=$?"),
    ("while read with here-document", "while read -r l; do echo \"<$l>\"; done <<EOF\nl1\n  l2\nEOF"),
    ("loop output redirected", "for i in 1 2; do echo $i; done > /tmp/l; cat /tmp/l"),
    ("loop in a pipeline runs in a subshell", "n=0; echo x | while read; do n=1; done; echo $n"),
    ("loop input redirected keeps variables", "n=0; while read -r; do n=$((n+1)); done < <(printf 'a\\nb\\n'); echo $n"),
    ("nested loop depth counting", "for a in 1 2; do for b in 1 2; do for c in 1 2; do [[ $c == 2 ]] && continue 3; echo $a$b$c; done; done; done"),
    ("select prints a menu", "PS3='pick: '; select c in one two; do echo \"got [$c] reply [$REPLY]\"; break; done <<< 2"),
    ("select with an invalid reply", "select c in one; do echo \"[$c] [$REPLY]\"; break; done <<< 9"),
    ("select at end of input", "select c in one; do echo in; done < /dev/null; echo status=$?"),
    ("select without in", "set -- x y; select c; do echo \"$c\"; break; done <<< 1"),
    ("break with a large count", "for a in 1; do for b in 2; do break 1000; done; echo not; done; echo out"),
    ("infinite loop with break", "n=0; while :; do (( ++n == 5 )) && break; done; echo $n"),
    ("until with continue", "n=0; until (( n >= 4 )); do (( n++ )); (( n % 2 )) && continue; echo $n; done"),
]:
    _add("loop " + label, script, ["compound.for", "compound.while"])

# --- Groups and subshells -------------------------------------------------------------------

for label, script in [
    ("group shares variables", "x=1; { x=2; }; echo $x"),
    ("subshell isolates variables", "x=1; (x=2); echo $x"),
    ("subshell isolates cd", "cd /tmp; (cd /); pwd"),
    ("subshell isolates functions", "(f() { :; }); declare -F f || echo none"),
    ("subshell isolates options", "(set -u); echo ${undef-ok}; (shopt -s nullglob); shopt -q nullglob || echo off"),
    ("subshell isolates traps", "trap 'echo main' EXIT; (trap - EXIT); echo body"),
    ("exit in a subshell", "(exit 6); echo $?; (exit); echo $?"),
    ("exit in a group ends the shell", "{ echo in; exit 3; }; echo not-reached"),
    ("group status", "{ false; }; echo $?; { true; false; true; }; echo $?"),
    ("group needs a separator", "{ echo a; }; { echo b\n}"),
    ("group as a pipeline stage", "{ echo a; echo b; } | while read l; do echo \"<$l>\"; done"),
    ("group redirect", "{ echo o; echo e >&2; } > /tmp/g 2>&1; cat /tmp/g"),
    ("nested subshells", "( ( ( echo deep; exit 2 ); echo $? ) )"),
    ("subshell pid differs", "[[ $BASHPID == $$ ]] && echo same-main; ( [[ $BASHPID != $$ ]] && echo differs )"),
    ("BASH_SUBSHELL levels", "echo $BASH_SUBSHELL; ( echo $BASH_SUBSHELL; ( echo $BASH_SUBSHELL ) ); { echo $BASH_SUBSHELL; }"),
    ("BASH_SUBSHELL in a pipeline", "echo $BASH_SUBSHELL | cat; true | echo $BASH_SUBSHELL"),
    ("subshell without space", "(echo a);(echo b)"),
    ("double parenthesis subshell", "( (echo spaced) )"),
    ("subshell with function body", "f() ( x=in; echo $x ); x=out; f; echo $x"),
    ("group with a trailing newline", "{\necho nl\n}"),
    ("background group", "{ echo bg; } & wait; echo done"),
    ("subshell exit status in a condition", "if (exit 0); then echo ok; fi"),
    ("variables after a pipeline", "x=0; x=1 | true; echo $x; echo p | x=2; echo $x"),
    ("lastpipe keeps the last stage", "shopt -s lastpipe; x=0; echo 9 | read x; echo $x"),
    ("lastpipe with a group", "shopt -s lastpipe; echo v | { read y; }; echo ${y-unset}"),
]:
    _add("group " + label, script, ["compound.group", "compound.subshell"])

# --- errexit ----------------------------------------------------------------------------------

for label, script in [
    ("simple failure exits", "set -e; echo a; false; echo b"),
    ("failure in an and list does not exit", "set -e; false && true; echo survived"),
    ("failure at the end of an and list exits", "set -e; true && false; echo not"),
    ("failure in an or list does not exit", "set -e; false || true; echo survived"),
    ("negated failure does not exit", "set -e; ! true; echo survived"),
    ("if condition does not exit", "set -e; if false; then :; fi; echo survived"),
    ("while condition does not exit", "set -e; while false; do :; done; echo survived"),
    ("until condition does not exit", "set -e; until true; do :; done; echo survived"),
    ("function in a condition ignores errexit", "set -e; f() { false; echo inside; }; if f; then echo t; fi; f && echo and"),
    ("function failing exits", "set -e; f() { false; echo inside; }; f; echo not"),
    ("subshell failure exits", "set -e; (false); echo not"),
    ("subshell inner failure", "set -e; (false; echo in-sub); echo not"),
    ("command substitution in assignment exits", "set -e; x=$(false); echo not"),
    ("command substitution in an argument does not exit", "set -e; echo \"[$(false)]\"; echo survived"),
    ("local with failing substitution does not exit", "set -e; f() { local x=$(false); echo in-f; }; f; echo survived"),
    ("pipeline last stage fails", "set -e; true | false; echo not"),
    ("pipeline first stage fails", "set -e; false | true; echo survived"),
    ("pipeline with pipefail", "set -eo pipefail; false | true; echo not"),
    ("arithmetic zero exits", "set -e; (( 0 )); echo not"),
    ("arithmetic in and list", "set -e; (( 0 )) && echo t; echo survived"),
    ("let zero exits", "set -e; let 0; echo not"),
    ("test failure exits", "set -e; [[ -z x ]]; echo not"),
    ("group failure exits", "set -e; { false; }; echo not"),
    ("group in an or list", "set -e; { false; echo in-group; } || echo or; echo survived"),
    ("brace group failure mid list", "set -e; { false; true; }; echo survived"),
    ("errexit in a subshell of an or list", "set -e; (false; echo in-sub) || echo or"),
    ("set +e turns it off", "set -e; set +e; false; echo survived"),
    ("errexit set inside a function", "f() { set -e; false; echo not; }; f; echo not2"),
    ("errexit with return status", "set -e; f() { return 3; }; f; echo not"),
    ("errexit and eval", "set -e; eval false; echo not"),
    ("errexit and eval in a condition", "set -e; if eval false; then :; fi; echo survived"),
    ("errexit exit status", "set -e; (exit 7); echo not"),
    ("errexit with ERR trap", "set -e; trap 'echo err $?' ERR; false; echo not"),
    ("errexit in a for loop", "set -e; for i in 1 2; do echo $i; false; done; echo not"),
    ("errexit in case body", "set -e; case x in x) false;; esac; echo not"),
    ("errexit with a failed redirect", "set -e; echo x > /tmp/no/such; echo not"),
    ("errexit with a failed builtin", "set -e; cd /nonexistent; echo not"),
    ("errexit with command not found", "set -e; no_such_command_zz; echo not"),
    ("errexit in background job", "set -e; { false; echo bg-continues; } & wait; echo waited"),
    ("errexit with a negated pipeline", "set -e; ! false | false; echo survived"),
    ("errexit and source", "printf 'false\\necho sourced-after\\n' > /tmp/s.sh; set -e; . /tmp/s.sh; echo not"),
    ("inherit_errexit in substitution", "set -e; shopt -s inherit_errexit; echo \"[$(false; echo in)]\"; echo survived"),
    ("errexit in a condition's subshell", "set -e; if (false; echo in-cond); then echo t; fi"),
    ("errexit shows in dash flags", "set -e; [[ $- == *e* ]] && echo has-e; set +e; [[ $- == *e* ]] || echo no-e"),
    ("set -o errexit", "set -o errexit; false; echo not"),
]:
    _add("errexit " + label, script, ["option.errexit"])

# --- nounset ----------------------------------------------------------------------------------

for label, script in [
    ("unset variable", "set -u; echo $undefined; echo not"),
    ("unset variable in a subshell", "set -u; ( echo $undefined ); echo status=$?"),
    ("default operator allowed", "set -u; echo ${undefined-d} ${undefined:-e}"),
    ("alternative operator allowed", "set -u; echo \"[${undefined+x}]\""),
    ("length of unset", "set -u; echo ${#undefined}; echo not"),
    ("unset positional", "set -u; set -- a; echo $1; echo $2; echo not"),
    ("dollar at with no arguments", "set -u; set --; echo \"[$@]\" \"[$*]\" $#"),
    ("empty array at", "set -u; a=(); echo \"[${a[@]}]\"; echo ok"),
    ("unset array element", "set -u; a=(x); echo ${a[3]}; echo not"),
    ("unset assoc element", "set -u; declare -A m; echo ${m[k]}; echo not"),
    ("declared without value", "set -u; declare d; echo $d; echo not"),
    ("empty variable is fine", "set -u; e=; echo \"[$e]\""),
    ("arithmetic unset", "set -u; echo $((undefined + 1)); echo not"),
    ("arithmetic bare name", "set -u; (( undefined )); echo not"),
    ("[[ -v is fine", "set -u; [[ -v undefined ]] || echo unset"),
    ("indirect to unset", "set -u; r=undefined; echo ${!r}; echo not"),
    ("unset special dollar bang", "set -u; echo $!; echo not"),
    ("in a function", "set -u; f() { echo $1; }; f; echo not"),
    ("in a here-document", "set -u; cat <<EOF\n$undefined\nEOF\necho not"),
    ("set +u", "set -u; set +u; echo \"[$undefined]\""),
    ("unset array whole", "set -u; unset a; echo ${a[@]}; echo not"),
    ("unset in a substring", "set -u; echo ${undefined:1}; echo not"),
    ("unset in a trim", "set -u; echo ${undefined#x}; echo not"),
    ("unset nameref target", "set -u; declare -n r=undefined; echo $r; echo not"),
]:
    _add("nounset " + label, script, ["option.nounset"])

# --- exit -------------------------------------------------------------------------------------

for label, script in [
    ("no argument keeps last status", "false; exit"),
    ("explicit status", "exit 42"),
    ("status modulo 256", "exit 300"),
    ("negative status", "exit -1"),
    ("non-numeric status", "exit abc; echo after"),
    ("too many arguments", "exit 1 2; echo after"),
    ("status with plus sign", "exit +5"),
    ("status with spaces", "exit ' 7 '"),
    ("huge status", "exit 99999999999999999999"),
    ("exit in a function", "f() { exit 9; }; f; echo not"),
    ("exit in a pipeline stage", "exit 3 | true; echo status=$?"),
    ("exit in a command substitution", "x=$(exit 4); echo status=$?"),
    ("exit after output without newline", "printf partial; exit 2"),
    ("exit inside a loop", "for i in 1 2; do exit $i; done"),
    ("exit in a trap", "trap 'exit 8' EXIT; exit 1"),
    ("exit status of last command", "(exit 5)"),
    ("logout is not allowed", "logout; echo status=$?"),
    ("exit with double dash", "exit -- 3"),
]:
    _add("exit " + label, script, ["builtin.exit"])

# --- eval ------------------------------------------------------------------------------------

for label, script in [
    ("assignments", "eval 'a=1; b=2'; echo $a$b"),
    ("joins its arguments", "eval echo one '\"two  three\"'"),
    ("status", "eval false; echo $?; eval; echo $?; eval ''; echo $?"),
    ("syntax error message", "eval 'echo (' ; echo status=$?"),
    ("defines a function", "eval 'f() { echo from-eval; }'; f"),
    ("nested eval", "eval 'eval \"echo nested\"'"),
    ("indirect assignment", "n=target; eval \"$n=value\"; echo $target"),
    ("with a here-document", "eval 'cat <<EOF\nhd\nEOF'"),
    ("multiple lines", "eval $'echo l1\\necho l2'"),
    ("return in eval inside a function", "f() { eval 'return 3'; echo not; }; f; echo $?"),
    ("exit in eval", "eval 'exit 5'; echo not"),
    ("eval of a double quoted expansion", "v='echo \"a  b\"'; eval \"$v\"; eval $v"),
    ("eval with LINENO", "eval 'echo $LINENO'\necho $LINENO"),
    ("eval of an alias definition", "shopt -s expand_aliases; eval 'alias e1=\"echo aliased\"'\ne1"),
    ("eval and set -e in a condition", "set -e; eval 'false; echo inside' || echo or"),
    ("eval with options", "eval -- echo dd; eval -x 2>&1; echo status=$?"),
]:
    _add("eval " + label, script, ["builtin.eval"])

# --- source -----------------------------------------------------------------------------------

for label, body, call in [
    ("sets variables", "sv=1", ". /tmp/s.sh; echo $sv"),
    ("receives arguments", "echo \"args: $# $*\"", ". /tmp/s.sh a 'b c'"),
    ("keeps outer arguments", "echo \"args: $*\"", "set -- outer; . /tmp/s.sh; echo $*"),
    ("return ends the file", "echo before; return 4; echo not", ". /tmp/s.sh; echo status=$?"),
    ("return without value", "false; return", "source /tmp/s.sh; echo status=$?"),
    ("exit ends the shell", "exit 6", ". /tmp/s.sh; echo not"),
    ("syntax error", "if then", ". /tmp/s.sh; echo status=$?"),
    ("defines functions", "sf() { echo sf; }", ". /tmp/s.sh; sf"),
    ("BASH_SOURCE and FUNCNAME", "echo \"${BASH_SOURCE[0]} ${FUNCNAME[0]-none}\"", ". /tmp/s.sh"),
    ("LINENO inside", "echo l$LINENO\necho l$LINENO", ". /tmp/s.sh"),
    ("nested source", "echo inner", "printf '. /tmp/s.sh\\necho outer\\n' > /tmp/o.sh; . /tmp/o.sh"),
    ("in a function sees locals", "echo \"$lv\"", "f() { local lv=local; . /tmp/s.sh; }; f"),
    ("local in a sourced file at top level", "local x=1", ". /tmp/s.sh; echo status=$?"),
    ("empty file", "", ". /tmp/s.sh; echo status=$?"),
    ("RETURN trap", "echo body", "trap 'echo ret' RETURN; . /tmp/s.sh"),
    ("relative path", "echo rel", "cd /tmp; . ./s.sh"),
    ("in a pipeline", "sv=piped", ". /tmp/s.sh | cat; echo \"[${sv-}]\""),
    ("with redirected input", "read -r line; echo \"[$line]\"", ". /tmp/s.sh <<< input"),
]:
    _add("source " + label, "printf '%s\\n' '" + body.replace("'", "'\\''") + "' > /tmp/s.sh; " + call, ["builtin.source"])
for label, script in [
    ("missing file", ". /tmp/missing.sh; echo status=$?"),
    ("missing argument", ".; echo status=$?"),
    ("a directory", ". /tmp; echo status=$?"),
]:
    _add("source " + label, script, ["builtin.source"])

# --- Aliases (off by default in a non-interactive shell) --------------------------------------

for label, script in [
    ("not expanded by default", "alias hi='echo hi'\nhi"),
    ("expanded after expand_aliases on an earlier line", "shopt -s expand_aliases\nalias hi='echo hi'\nhi"),
    ("same line definition is not used", "shopt -s expand_aliases\nalias hi='echo hi'; hi"),
    ("trailing space expands the next word", "shopt -s expand_aliases\nalias run='echo run '\nalias w='word'\nrun w"),
    ("alias listing", "alias a1='echo x' a2=y\nalias; alias a1"),
    ("unalias", "shopt -s expand_aliases\nalias u1='echo u'\nunalias u1\nu1 2>/dev/null; echo status=$?"),
    ("unalias -a", "alias p1=x p2=y; unalias -a; alias; echo status=$?"),
    ("unknown alias", "alias nosuch; echo status=$?; unalias nosuch; echo status=$?"),
    ("alias in a function body defined later", "shopt -s expand_aliases\nf() { al; }\nalias al='echo late'\nf 2>/dev/null; echo status=$?"),
    ("alias used inside a function defined after it", "shopt -s expand_aliases\nalias al='echo early'\nf() { al; }\nf"),
    ("alias with a keyword", "shopt -s expand_aliases\nalias maybe='if true; then echo kw; fi'\nmaybe"),
    ("recursive alias stops", "shopt -s expand_aliases\nalias ls2='ls2x'\nalias ls2x='echo stop'\nls2"),
    ("alias not in quotes", "shopt -s expand_aliases\nalias q='echo q'\n'q' 2>/dev/null; echo status=$?"),
    ("alias -p", "alias z='echo it'\\''s'; alias -p"),
    ("invalid alias name", "alias 'a b=c'; echo status=$?"),
]:
    _add("alias " + label, script, ["syntax.alias"])

# --- xtrace -----------------------------------------------------------------------------------

for label, script in [
    ("simple command", "set -x; echo a b"),
    ("quoted arguments", "set -x; echo 'a b' \"c\" '' $'t\\tt'"),
    ("assignment", "set -x; v=1; w='x y'"),
    ("assignment and command", "set -x; v=1 true"),
    ("arithmetic command", "set -x; (( 1 + 2 ))"),
    ("conditional command", "set -x; [[ a == b ]]"),
    ("for loop", "set -x; for i in 1 2; do :; done"),
    ("arithmetic for loop", "set -x; for ((i=0; i<1; i++)); do :; done"),
    ("case", "set -x; case x in x) : ;; esac"),
    ("if", "set -x; if true; then :; fi"),
    ("while", "set -x; i=0; while (( i < 1 )); do (( i++ )); done"),
    ("function call", "f() { echo in; }; set -x; f arg"),
    ("subshell depth", "set -x; ( echo sub; ( echo deeper ) )"),
    ("command substitution", "set -x; x=$(echo inner)"),
    ("pipeline", "(set -x; echo p | cat) 2>/tmp/trace; sort /tmp/trace"),  # the stages trace in either order
    ("redirection is not shown", "set -x; echo r > /tmp/x; cat < /tmp/x"),
    ("here-document", "set -x; cat <<EOF\nhd\nEOF"),
    ("custom PS4", "PS4='>> '; set -x; echo ps4"),
    ("PS4 with expansion", "PS4='+${LINENO}: '; set -x; echo l\necho m"),
    ("set +x is traced", "set -x; echo on; set +x; echo off"),
    ("special characters", "set -x; echo '*' '$v' 'a\"b' \"it's\""),
    ("newline argument", "set -x; echo $'a\\nb'"),
    ("array assignment", "set -x; a=(1 'two words')"),
    ("declare", "set -x; declare -i n=5"),
    ("export", "set -x; export E=1"),
    ("local in a function", "f() { local l=1; }; set -x; f"),
    ("eval", "set -x; eval 'echo e'"),
    ("xtrace with BASH_XTRACEFD", "exec 5>/tmp/trace; BASH_XTRACEFD=5; set -x; echo traced; set +x; cat /tmp/trace"),
    ("xtrace via set -o", "set -o xtrace; echo o"),
    ("empty command substitution", "set -x; $(true)"),
    ("unicode argument", "set -x; echo héllo"),
    ("backslash argument", "set -x; echo 'a\\b'"),
    ("assignment with tilde", "set -x; HOME=/h; v=~/x"),
    ("brace group", "set -x; { echo g; }"),
    ("negation", "set -x; ! false"),
    ("trap handler", "trap 'echo handler' EXIT; set -x; echo body"),
]:
    _add("xtrace " + label, script, ["option.xtrace"])

# --- Other options ----------------------------------------------------------------------------

for label, script in [
    ("noexec skips commands", "echo before; set -n; echo not"),
    ("noexec still checks syntax at parse", "set -n\necho not"),
    ("verbose prints input", "set -v; echo v"),
    ("allexport exports", "set -a; av=1; bash -c 'echo ${av-unset}'"),
    ("dash flags", "echo $-; set -f; echo $-; set +f -u; echo $-"),
    ("set -o listing of one option", "set -o noglob; set -o | while read -r n v; do [[ $n == noglob ]] && echo \"$n $v\"; done; set +o | while read -r l; do [[ $l == *\\ noglob ]] && echo \"$l\"; done"),
    ("shopt -p of one option", "shopt -p extglob; shopt -s extglob; shopt -p extglob"),
    ("shopt -q status", "shopt -q nullglob; echo $?; shopt -s nullglob; shopt -q nullglob; echo $?"),
    ("set unknown option", "set -o nosuchopt; echo status=$?"),
    ("set -- clears positionals", "set -- a b; set --; echo $#"),
    ("set - turns off x and v", "set -x; set -; echo quiet"),
    ("set with only a double dash", "set -- ; echo $#"),
    ("shopt -o", "shopt -o -s nounset; echo $-; shopt -o -u nounset; echo $-"),
    ("posix mode flag", "set -o posix; [[ :$SHELLOPTS: == *:posix:* ]] && echo posix"),
    ("SHELLOPTS reflects set -o", "set -o noglob; [[ :$SHELLOPTS: == *:noglob:* ]] && echo has"),
    ("BASHOPTS reflects shopt", "shopt -s nullglob; [[ :$BASHOPTS: == *:nullglob:* ]] && echo has"),
    ("time with empty TIMEFORMAT", "TIMEFORMAT=; time true; echo status=$?"),
    ("time of a failing pipeline", "TIMEFORMAT=; time false | true; echo $?; time ! true; echo $?"),
]:
    _add("option " + label, script)
