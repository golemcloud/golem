"""Grammar sweep: arithmetic expansion, (( )), let, declare -i and arithmetic contexts.

Expressions come from a small grammar driven by a fixed-seed arithmetic PRNG. Each generated
expression is kept only if its shape (operators and structure, ignoring which literal or variable
fills a leaf) is new, so the cases differ in construct rather than in literals.
"""

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

    def pick(self, seq):
        return seq[self.below(len(seq))]


CASES = []
_NAMES = []


def _add(name, script, tags=()):
    name = "gram arith " + name
    if name in _NAMES:
        raise ValueError("duplicate case name " + name)
    _NAMES.append(name)
    CASES.append((name, script, list(tags)))


SETUP = "x=5 y=-3 s='x+1' r=x c=0 t=0; a=(10 20 30); unset z; "
LEAVES = [
    ("N", "7"), ("N", "0"), ("N", "1"), ("HEX", "0x1f"), ("OCT", "010"), ("BASE", "2#101"),
    ("BASE", "36#z"), ("V", "x"), ("V", "y"), ("UNSET", "z"), ("EXPR", "s"), ("CHAIN", "r"),
    ("ELEM", "a[1]"), ("ELEM", "a[x-4]"), ("DOLLAR", "$x"), ("NEG", "-2"),
]
SAFE_RIGHT = ["1", "2", "3", "7"]
BINARY = ["*", "/", "%", "+", "-", "<<", ">>", "<", "<=", ">", ">=", "==", "!=", "&", "^", "|", "&&", "||", "**"]
UNARY = ["-", "+", "!", "~"]
ASSIGN = ["=", "+=", "-=", "*=", "/=", "%=", "<<=", ">>=", "&=", "^=", "|="]


def _gen(rng, depth):
    """Return (shape, text) for a random expression."""
    if depth == 0 or rng.below(4) == 0:
        return rng.pick(LEAVES)
    kind = rng.below(9)
    if kind <= 3:
        op = rng.pick(BINARY)
        ls, lt = _gen(rng, depth - 1)
        if op in ("/", "%", "<<", ">>", "**"):
            rs, rt = "N", rng.pick(SAFE_RIGHT)
        else:
            rs, rt = _gen(rng, depth - 1)
        spaced = rng.below(2)
        text = lt + (" " + op + " " if spaced else op) + rt
        return "(" + ls + op + rs + ")", text
    if kind == 4:
        op = rng.pick(UNARY)
        s, t = _gen(rng, depth - 1)
        return op + "U(" + s + ")", op + "(" + t + ")"
    if kind == 5:
        cs, ct = _gen(rng, depth - 1)
        a, at = _gen(rng, depth - 1)
        b, bt = _gen(rng, depth - 1)
        return "(" + cs + "?" + a + ":" + b + ")", "(" + ct + " ? " + at + " : " + bt + ")"
    if kind == 6:
        op = rng.pick(ASSIGN)
        target = rng.pick(["t", "c", "a[2]"])
        if op in ("/=", "%=", "<<=", ">>="):
            vs, vt = "N", rng.pick(SAFE_RIGHT)
        else:
            vs, vt = _gen(rng, depth - 1)
        return "(A" + target[0] + op + vs + ")", "(" + target + " " + op + " " + vt + ")"
    if kind == 7:
        form = rng.pick(["c++", "c--", "++c", "--c", "t++", "++a[0]", "a[1]--"])
        return "I" + form.replace("c", "V").replace("t", "V"), form
    s1, t1 = _gen(rng, depth - 1)
    s2, t2 = _gen(rng, depth - 1)
    return "(" + s1 + "," + s2 + ")", "(" + t1 + ", " + t2 + ")"


CONTEXTS = [
    ("expansion", lambda e: "echo $((" + e + "))"),
    ("command", lambda e: "((" + e + ")); echo \"status=$?\""),
    ("let", lambda e: "let '" + e.replace("'", "") + "'; echo \"status=$?\""),
    ("declare -i", lambda e: "declare -i v; v='" + e + "'; echo \"$v\""),
    ("subscript", lambda e: "b=(); b[" + e + "]=1 2>/dev/null; echo \"status=$? ${!b[@]}\""),
    ("legacy", lambda e: "echo $[" + e + "]"),
]

rng = _Rng(0x5eed2026)
seen = []
count = 0
attempts = 0
while count < 120 and attempts < 20000:
    attempts += 1
    shape, text = _gen(rng, 1 + rng.below(4))
    if shape in seen or len(text) > 90 or shape in [leaf[0] for leaf in LEAVES]:
        continue
    seen.append(shape)
    cname, ctx = CONTEXTS[count % len(CONTEXTS)]
    count += 1
    _add("generated " + str(count) + " " + cname + ": " + text,
         SETUP + ctx(text) + "; echo \"x=$x c=$c t=$t a=${a[*]}\"",
         ["expansion.arithmetic"])

# --- Fixed cases: literals, bases and errors --------------------------------------------------

for label, expr in [
    ("decimal", "42"), ("leading zeros octal", "0017"), ("octal eight", "08"), ("octal nine", "019"),
    ("hex upper", "0XFF"), ("hex empty", "0x"), ("hex bad digit", "0xg"), ("base two", "2#1010"),
    ("base two bad digit", "2#12"), ("base 16", "16#ff"), ("base 36", "36#Z"), ("base 64 at", "64#@"),
    ("base 64 underscore", "64#_"), ("base 37 upper", "37#A"), ("base 37 lower", "37#a"),
    ("base one", "1#0"), ("base 65", "65#1"), ("base zero", "0#1"), ("base ten negative", "10#-8"),
    ("base ten leading zeros", "10#0010"), ("base no digits", "10#"), ("base with variable", "b#11"),
    ("float", "1.5"), ("exponent notation", "1e3"), ("quoted number", "\"5\""), ("spaces", "' 5 '"),
    ("max int", "9223372036854775807"), ("min int", "-9223372036854775808"),
    ("overflow literal", "9223372036854775808"), ("huge literal", "99999999999999999999"),
    ("add overflow", "9223372036854775807 + 1"), ("multiply overflow", "4611686018427387904 * 4"),
    ("min divided by minus one", "-9223372036854775808 / -1"),
    ("min modulo minus one", "-9223372036854775808 % -1"),
    ("divide by zero", "1 / 0"), ("modulo by zero", "5 % 0"), ("divide assign by zero", "t /= 0"),
    ("negative exponent", "2 ** -1"), ("zero to zero", "0 ** 0"), ("power precedence", "-2 ** 2"),
    ("power right assoc", "2 ** 3 ** 2"), ("negative shift", "1 << -1"), ("large shift", "1 << 64"),
    ("shift 63", "1 << 63"), ("right shift negative", "-16 >> 2"), ("modulo negative", "-7 % 3"),
    ("division truncates", "-7 / 2"), ("unary chain", "- - -3"), ("not not", "!!7"),
    ("bitwise not", "~0"), ("comma", "1, 2, 3"), ("nested ternary", "1 ? 0 ? 2 : 3 : 4"),
    ("ternary assignment", "1 ? t = 4 : (c = 5)"), ("ternary short circuit", "0 ? c++ : t++"),
    ("and short circuit", "0 && c++"), ("or short circuit", "1 || c++"),
    ("assignment to literal", "1 = 2"), ("assignment to expression", "(t) = 2"),
    ("double increment", "c++ + ++c"), ("increment literal", "5++"), ("increment expression", "++(c)"),
    ("empty", ""), ("only spaces", "   "), ("unbalanced open", "(1 + 2"), ("unbalanced close", "1 + 2)"),
    ("trailing operator", "1 +"), ("leading operator", "* 2"), ("two operands", "1 2"),
    ("unknown operator", "1 ** * 2"), ("string variable", "w"), ("recursive variable", "rec"),
    ("variable with spaces", "sp"), ("dollar in name", "$t$t"), ("array without index", "a"),
    ("array negative index", "a[-1]"), ("array bad negative index", "a[-9]"),
    ("array star", "a[*]"), ("assoc element", "m[k]"), ("assoc string key", "m[k]+m[j]"),
    ("positional", "$1 + $2"), ("dollar hash", "$#"), ("parameter expansion", "${#a[@]} * 2"),
    ("command substitution", "$(echo 3) + 1"), ("nested arithmetic", "$((1+1)) * 3"),
    ("comparison chain", "1 < 2 < 3"), ("equality of strings", "q == q2"),
    ("bitwise precedence", "1 | 2 ^ 3 & 4"), ("logical precedence", "0 || 1 && 0"),
    ("assign chain", "t = c = 3"), ("compound assign chain", "t += c += 2"),
    ("post decrement in condition", "c-- ? 1 : 2"), ("unset variable arithmetic", "undefined + 1"),
    ("readonly assignment", "ro = 2"), ("integer attribute string", "iv + 1"),
    ("newline inside", "1 +\n2"), ("tab inside", "1\t+\t2"), ("hash comment", "1 # 2"),
    ("character constant", "'a'"), ("backslash", "1 \\+ 2"), ("unicode digit", "١"),
]:
    _add("fixed " + label,
         "x=5 t=0 c=0 w=hello rec=rec sp='1 2' q=1 q2=1 iv=abc; readonly ro=1; declare -A m=([k]=3 [j]=4); "
         "a=(10 20 30); set -- 4 5; echo \"[$((" + expr + "))]\"; echo \"status=$? t=$t c=$c\"",
         ["expansion.arithmetic"])
    if len(label) % 3 == 0:
        _add("fixed command " + label,
             "x=5 t=0 c=0 w=hello rec=rec sp='1 2' q=1 q2=1 iv=abc; readonly ro=1; declare -A m=([k]=3 [j]=4); "
             "a=(10 20 30); set -- 4 5; (( " + expr + " )); echo \"status=$? t=$t c=$c\"",
             ["compound.arith"])

# An arithmetic error in an expansion ends the command; outside a subshell, the script.
for label, expr in [("divide by zero", "1/0"), ("syntax", "1 +"), ("bad base", "2#3"),
                    ("recursion", "rec"), ("bad subscript", "a[-9]"), ("assign to literal", "1=2")]:
    _add("error ends the shell: " + label,
         "rec=rec; a=(1); echo before; echo $((" + expr + ")); echo \"after $?\"\necho next line",
         ["expansion.arithmetic", "error"])
    _add("error in a command: " + label,
         "rec=rec; a=(1); echo before; ((" + expr + ")); echo \"after $?\"\necho next line",
         ["compound.arith", "error"])
    _add("error in let: " + label,
         "rec=rec; a=(1); echo before; let '" + expr + "'; echo \"after $?\"\necho next line",
         ["builtin.let", "error"])
    _add("error in a for header: " + label,
         "rec=rec; a=(1); echo before; for ((i=0; " + expr + "; i++)); do echo body; break; done; echo \"after $?\"\necho next line",
         ["compound.for-arith", "error"])
    _add("error in declare -i: " + label,
         "rec=rec; a=(1); declare -i v; echo before; v='" + expr + "'; echo \"after $? v=$v\"\necho next line",
         ["param.attributes", "error"])

# --- let, (( )), declare -i and other arithmetic contexts -------------------------------------

for label, script in [
    ("let several arguments", "let a=1 b=a+1 'c = b * 3'; echo $a $b $c"),
    ("let status of zero", "let 0; echo $?; let 1; echo $?; let 'x=0'; echo $?"),
    ("let no arguments", "let; echo status=$?"),
    ("let with spaces unquoted", "let x = 1; echo status=$?"),
    ("double parentheses status", "((0)); echo $?; ((-1)); echo $?; ((2-2)); echo $?"),
    ("double parentheses assignment result", "((x = 0)); echo $? $x; ((x = 7)); echo $? $x"),
    ("declare -i on assignment", "declare -i n; n=3+4; echo $n; n+=2; echo $n; n=n*2; echo $n"),
    ("declare -i with string", "declare -i n; n=hello; echo $n; hello=9; n=hello; echo $n"),
    ("declare -i append", "declare -i n=5; n+=5; echo $n; declare +i n; n+=5; echo $n"),
    ("declare -i array", "declare -ai arr=(1+1 2*3); arr+=(4-1); echo ${arr[@]}"),
    ("declare -i assoc", "declare -Ai m=([a]=1+1); m[b]=3*3; echo ${m[a]} ${m[b]}"),
    ("local -i", "f() { local -i n=2+2; echo $n; }; f"),
    ("integer and lowercase", "declare -il n=2+3; echo $n"),
    ("arithmetic for loop", "for ((i=0; i<3; i++)); do printf '%s ' $i; done; echo"),
    ("arithmetic for empty parts", "i=0; for ((;;)); do ((i++ >= 2)) && break; done; echo $i"),
    ("arithmetic for with comma", "for ((i=0, j=10; i<3; i++, j--)); do printf '%s:%s ' $i $j; done; echo"),
    ("arithmetic for status", "for ((i=0; i<0; i++)); do :; done; echo $?"),
    ("arithmetic for body status", "for ((i=0; i<2; i++)); do false; done; echo $?"),
    ("arithmetic for with spaces", "for (( i = 0 ; i < 2 ; i++ )) ; do echo $i; done"),
    ("arithmetic for with braces body", "for ((i=0;i<2;i++)) { echo b$i; }"),
    ("substring offsets", "s=abcdef; i=1; echo ${s:i+1:i*2} ${s:(-i-1)}"),
    ("array index arithmetic", "a=(a b c d); i=1; echo ${a[i+1]} ${a[i*3]} ${a[-i]}"),
    ("assoc index is not arithmetic", "declare -A m; m[1+1]=x; echo ${!m[@]}"),
    ("[[ -eq evaluates expressions", "x=3; [[ x+1 -eq 4 ]] && echo yes; [[ 2*3 -gt 5 ]] && echo gt"),
    ("[ -eq rejects expressions", "[ 1+1 -eq 2 ]; echo status=$?"),
    ("[[ -eq with unset", "unset u; [[ $u -eq 0 ]] && echo zero"),
    ("test -lt with spaces", "[ ' 3 ' -lt 4 ]; echo status=$?"),
    ("dollar bracket", "echo $[2*3] $[x=4] $x"),
    ("arithmetic in here-document", "cat <<EOF\n$((6*7))\nEOF"),
    ("arithmetic quoted", "echo \"$((1+2))\" '$((1+2))'"),
    ("arithmetic with quotes inside", "echo $(( \"1\" + '2' ))"),
    ("arithmetic word splitting", "IFS=0; echo $((100))"),
    ("increment unset", "unset n; ((n++)); echo $n; ((m--)); echo $m"),
    ("increment string variable", "v=abc; ((v++)); echo $v"),
    ("increment through reference", "v=w; w=1; ((v++)); echo $v $w"),
    ("assignment through reference", "v=w; ((v=5)); echo $v ${w-unset}"),
    ("nameref arithmetic", "n=1; declare -n r=n; ((r+=4)); echo $n"),
    ("readonly arithmetic assignment", "readonly k=1; ((k=2)); echo status=$? k=$k"),
    ("arithmetic in case", "case $((1+1)) in 2) echo two;; esac"),
    ("arithmetic in while", "i=0; while ((i<3)); do ((i++)); done; echo $i"),
    ("arithmetic in until", "i=5; until ((i==0)); do ((i--)); done; echo $i"),
    ("arithmetic in if", "if ((1 > 2)); then echo gt; elif ((2 > 1)); then echo lt; fi"),
    ("arithmetic negation pipeline", "! ((0)); echo $?"),
    ("arithmetic with set -e", "set -e; ((0)) || true; echo survived; ((0)); echo not-reached"),
    ("arithmetic with set -u", "set -u; echo $((undefined + 1)); echo after"),
    ("arithmetic with set -u in let", "set -u; let 'k = undefined2'; echo after $?"),
    ("arithmetic with set -u in command", "set -u; (( undefined3 )); echo after $?"),
    ("arithmetic assign to positional", "set -- 1; ((1 = 2)); echo status=$?"),
    ("arithmetic assign to special", "(( $ = 2 )); echo status=$?"),
    ("arithmetic long expression", "echo $(( 1+2+3+4+5+6+7+8+9+10+11+12+13+14+15+16+17+18+19+20 ))"),
    ("arithmetic deep parentheses", "echo $(( ((((((((1+1)))))))) ))"),
    ("arithmetic side effects order", "i=1; echo $(( i++ * 10 + i ))"),
    ("arithmetic in brace sequence is literal", "echo {1..$((2))}"),
    ("arithmetic variable holding hex", "h=0x10; echo $((h + 1))"),
    ("arithmetic variable holding octal", "o=010; echo $((o + 1))"),
    ("arithmetic variable holding base", "b=2#11; echo $((b + 1))"),
    ("arithmetic variable holding negative", "n=-5; echo $((-n)) $((n*n))"),
    ("arithmetic variable holding spaces", "n=' 4 '; echo $((n + 1))"),
    ("arithmetic variable holding empty", "n=; echo $((n + 1))"),
    ("arithmetic variable holding assignment", "n='q=3'; echo $((n)) $q"),
    ("arithmetic variable holding comma", "n='1,2'; echo $((n))"),
    ("arithmetic nested variable depth", "a1=a2 a2=a3 a3=a4 a4=7; echo $((a1))"),
    ("arithmetic array element holding expression", "arr=('2*3'); echo $((arr[0] + 1))"),
    ("arithmetic subscript with side effect", "i=0; arr[i++]=a; arr[i++]=b; echo ${#arr[@]} $i"),
    ("arithmetic with BASH_REMATCH", "[[ 42 =~ ([0-9]+) ]]; echo $((BASH_REMATCH[1] + 1))"),
    ("arithmetic string compare", "echo $(( abc == abd ))"),
    ("arithmetic in printf argument", "printf '%d\\n' $((3*3)) '5' 0x10 010"),
]:
    _add(label, script, ["expansion.arithmetic"])
