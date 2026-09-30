"""Grammar sweep: parameter expansion operators, generated over subjects, words and contexts.

Every script is built from fixed tables with plain loops and a fixed-seed arithmetic PRNG, so the
cases (and the script hashes their goldens are keyed by) are the same on every Python 3. Where a
full product would be large, `_pairwise` keeps the combinations that cover every pair of values
from two dimensions at least once.
"""
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

    def pick(self, seq):
        return seq[self.below(len(seq))]

    def shuffled(self, seq):
        items = list(seq)
        for i in range(len(items) - 1, 0, -1):
            j = self.below(i + 1)
            items[i], items[j] = items[j], items[i]
        return items


def _pairwise(dims, seed, valid=None):
    """Index tuples over `dims` covering every pair of values of two dimensions at least once."""
    rng = _Rng(seed)
    combos = rng.shuffled(itertools.product(*[range(len(d)) for d in dims]))
    if valid is not None:
        combos = [c for c in combos if valid(*[dims[k][i] for k, i in enumerate(c)])]
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
    name = "gram param " + name
    if name in _NAMES:
        raise ValueError("duplicate case name " + name)
    _NAMES.append(name)
    CASES.append((name, script, list(tags)))


# `p` prints its argument count and each argument in brackets, so zero words, one empty word and
# splitting are all visible.
P = "p() { printf '%s' \"$#\"; printf ' <%s>' \"$@\"; echo; }"

# --- ${x-w} ${x:-w} ${x+w} ${x:+w} ${x=w} ${x:=w} ${x?w} ${x:?w} -----------------------------

SUBJECTS = [
    # label, setup, reference, how the reference prints afterwards
    ("unset", "unset v", "v"),
    ("empty", "v=", "v"),
    ("set", "v=abc", "v"),
    ("spaced", "v=' a  b '", "v"),
    ("glob value", "cd /tmp; touch g1 g2; v='g*'", "v"),
    ("pos1 set", "set -- one 'two words'", "1"),
    ("pos1 empty", "set -- '' x", "1"),
    ("pos3 unset", "set -- one 'two words'", "3"),
    ("at two", "set -- one 'two words'", "@"),
    ("at none", "set --", "@"),
    ("star none", "set --", "*"),
    ("elem set", "a=(x 'y z' '')", "a[1]"),
    ("elem unset", "a=(x 'y z' '')", "a[7]"),
    ("array at", "a=(x 'y z' '')", "a[@]"),
    ("array at empty", "a=()", "a[@]"),
    ("array star", "a=(x 'y z')", "a[*]"),
    ("assoc key", "declare -A m=([k]=v [e]=)", "m[k]"),
    ("assoc missing", "declare -A m=([k]=v [e]=)", "m[z]"),
    ("assoc at", "declare -A m=([k]=v)", "m[@]"),
    ("special hash", "set -- a b", "#"),
    ("special question", "false", "?"),
]
OPS = ["-", ":-", "+", ":+", "=", ":=", "?", ":?"]
WORDS = [
    ("literal", "W"),
    ("double quoted", '"d  q"'),
    ("variable", "$w"),
    ("side effect", "$((n+=1))"),
]
CONTEXTS = ["bare", "quoted", "assign", "heredoc"]
OPNAMES = {"-": "dash", ":-": "colon-dash", "+": "plus", ":+": "colon-plus", "=": "equals",
           ":=": "colon-equals", "?": "question", ":?": "colon-question"}


def _default_script(subject, op, word, context):
    label, setup, ref = subject
    expr = "${" + ref + op + word[1] + "}"
    pre = P + "; w='p  q'; n=0; " + setup + "; "
    if context == "bare":
        body = "p " + expr
    elif context == "quoted":
        body = 'p "' + expr + '"'
    elif context == "assign":
        body = "r=" + expr + "; p \"$r\""
    else:
        body = "cat <<EOF\n[" + expr + "]\nEOF"
    sep = "\n" if context == "heredoc" else "; "
    after = sep + "echo \"n=$n\""
    if op in ("=", ":="):
        shown = '"${' + ref + '}"' if ref not in ("@", "*") else '"$' + ref + '"'
        after += "; p " + shown
    if op in ("?", ":?"):
        return pre + "( " + body + sep + ")" + sep + "echo \"status=$?\"" + after
    return pre + body + after


for subject, op, word, context in _pairwise([SUBJECTS, OPS, WORDS, CONTEXTS], 20260924):
    _add(
        "default " + OPNAMES[op] + ": " + subject[0] + ", " + word[0] + " word, " + context,
        _default_script(subject, op, word, context),
        ["expansion.parameter.default"] if op in ("-", ":-", "=", ":=") else
        ["expansion.parameter.alternative"] if op in ("+", ":+") else ["expansion.parameter.error"],
    )

# The error operators end a non-interactive shell; outside a subshell nothing after them runs.
for label, setup, ref in [("unset", "unset v", "v"), ("empty", "v=", "v"), ("pos1", "set --", "1"),
                          ("array elem", "a=()", "a[0]"), ("at none", "set --", "@")]:
    for op in ("?", ":?"):
        for message in ("", " custom text", "'quoted  msg'"):
            _add(
                "error " + OPNAMES[op] + " ends the shell: " + label + ", message " + (message.strip() or "none"),
                setup + "; echo before; echo \"${" + ref + op + message + "}\"; echo after",
                ["expansion.parameter.error"],
            )

# --- ${x#p} ${x##p} ${x%p} ${x%%p} ----------------------------------------------------------

TRIM_OPS = [("#", "hash"), ("##", "double hash"), ("%", "percent"), ("%%", "double percent")]
TRIM_PATTERNS = [
    ("literal", "a"), ("star", "*"), ("question", "?"), ("star suffix", "*b"), ("star prefix", "b*"),
    ("bracket", "[ab]"), ("negated bracket", "[!a]*"), ("caret bracket", "[^c]*"),
    ("class", "[[:alpha:]]"), ("digit class", "*[[:digit:]]"),
    ("quoted star", '"*"'), ("escaped star", "\\*"), ("variable pattern", "$pat"),
    ("quoted variable pattern", '"$pat"'), ("empty", ""),
    ("inner star", "a*c"), ("single quoted", "'a'*"),
]
TRIM_SUBJECTS = [
    ("literal stars", "v='a*b*c'", "v"),
    ("array at", "a=(abc bca cab)", "a[@]"),
    ("array star", "a=(abc bca cab)", "a[*]"),
    ("positional", "set -- abc bca 'c ab'", "@"),
    ("assoc elem", "declare -A m=([k]=abcab)", "m[k]"),
    ("empty", "v=", "v"),
    ("unicode", "v=éaéb", "v"),
]
for subject, op, pattern, quoted in _pairwise(
        [TRIM_SUBJECTS, TRIM_OPS, TRIM_PATTERNS[::2], ["quoted", "bare"]], 7):
    expr = "${" + subject[2] + op[0] + pattern[1] + "}"
    use = '"' + expr + '"' if quoted == "quoted" else expr
    _add(
        "trim " + op[1] + ": " + subject[0] + ", " + pattern[0] + ", " + quoted,
        "cd /tmp; " + P + "; pat='*'; " + subject[1] + "; p " + use,
        ["expansion.parameter.trim"],
    )
# Every trim operator against every pattern on a plain string.
for op in TRIM_OPS:
    for pattern in TRIM_PATTERNS:
        script = "cd /tmp; pat='*'; v=abcabc1; echo \"[${v" + op[0] + pattern[1] + "}]\""
        _add("trim table " + op[1] + ": " + pattern[0], script, ["expansion.parameter.trim"])

# --- ${x/p/r} ${x//p/r} ${x/#p/r} ${x/%p/r} ---------------------------------------------------

REPL_OPS = [("/", "first"), ("//", "all"), ("/#", "prefix"), ("/%", "suffix")]
REPL_PATTERNS = [
    ("literal", "a"), ("star", "*"), ("question", "?"), ("bracket", "[ab]"), ("class", "[[:upper:]]"),
    ("quoted star", '"*"'), ("escaped slash", "\\/"), ("variable", "$pat"),
    ("quoted variable", '"$pat"'), ("empty", ""), ("multi", "ab"), ("inner star", "a*b"),
    ("space", "' '"),
]
REPLACEMENTS = [
    ("literal", "X"), ("deleted", None), ("empty", ""), ("ampersand", "&"),
    ("quoted ampersand", '"&"'), ("variable", "$r"),
    ("quoted variable", '"$r"'), ("slash", "a/b"), ("spaces", "'x  y'"),
]
REPL_SUBJECTS = [
    ("scalar", "v='ab/Ab ab'", "v"),
    ("array at", "a=(ab 'b a' Ab)", "a[@]"),
    ("array star", "a=(ab 'b a' Ab)", "a[*]"),
    ("positional", "set -- ab 'b a' Ab", "@"),
    ("assoc at", "declare -A m=([k]='ab ab')", "m[@]"),
    ("empty", "v=", "v"),
    ("unset", "unset v", "v"),
]
for subject, op, pattern, repl, quoted in _pairwise(
        [REPL_SUBJECTS, REPL_OPS, REPL_PATTERNS[::2], REPLACEMENTS[::2] + [REPLACEMENTS[3]], ["quoted", "bare"]], 11):
    if repl[1] is None:
        expr = "${" + subject[2] + op[0] + pattern[1] + "}"
    else:
        expr = "${" + subject[2] + op[0] + pattern[1] + "/" + repl[1] + "}"
    use = '"' + expr + '"' if quoted == "quoted" else expr
    _add(
        "replace " + op[1] + ": " + subject[0] + ", " + pattern[0] + " pattern, " + repl[0]
        + " replacement, " + quoted,
        "cd /tmp; " + P + "; pat='?'; r='<&>'; " + subject[1] + "; p " + use,
        ["expansion.parameter.replace"],
    )
# patsub_replacement on and off, and & in each quoting.
for setting in ("-s", "-u"):
    for repl, rname in [("&", "bare"), ("\\&", "escaped"), ("'&'", "single"), ('"&"', "double"),
                        ("[&&]", "doubled"), ("\\\\&", "escaped backslash"), ("$r", "variable")]:
        _add(
            "replace patsub_replacement " + setting + ": " + rname + " ampersand",
            "shopt " + setting + " patsub_replacement; r='<&>'; v=abcb; echo \"${v//b/" + repl + "}\"; echo ${v/b/" + repl + "}",
            ["expansion.parameter.replace"],
        )

# --- ${x:offset} ${x:offset:length} --------------------------------------------------------

OFFSETS = [("zero", "0"), ("two", "2"), ("negative spaced", " -2"), ("negative parens", "(-2)"),
           ("past end", "10"), ("before start", " -10"), ("arithmetic", "i+1"), ("variable", "$i"),
           ("empty", ""), ("negative one", " -1")]
LENGTHS = [("none", None), ("zero", "0"), ("two", "2"), ("negative", "-1"),
           ("past start", "-10"), ("large", "100"), ("arithmetic", "i*2"), ("empty", "")]
SUB_SUBJECTS = [
    ("unicode", "v=héllo", "v"),
    ("array at", "a=(a b c d e f)", "a[@]"),
    ("array star", "a=(a b c d e f)", "a[*]"),
    ("sparse array", "a=([1]=a [5]=b [9]=c)", "a[@]"),
    ("positional", "set -- a b c d e f", "@"),
    ("positional star", "set -- a b c d e f", "*"),
    ("empty", "v=", "v"),
    ("unset", "unset v", "v"),
    ("assoc elem", "declare -A m=([k]=abcdef)", "m[k]"),
]
for subject, off, length in _pairwise([SUB_SUBJECTS, OFFSETS[::2], LENGTHS[1::2]], 13):
    expr = "${" + subject[2] + ":" + off[1] + ("" if length[1] is None else ":" + length[1]) + "}"
    _add(
        "substring: " + subject[0] + ", offset " + off[0] + ", length " + length[0],
        P + "; i=1; " + subject[1] + "; p \"" + expr + "\"; echo \"status=$?\"",
        ["expansion.parameter.substring"],
    )
# Every offset against every length on a plain string, each alone.
for off in OFFSETS[:6]:
    for length in LENGTHS[:5]:
        expr = "${v:" + off[1] + ("" if length[1] is None else ":" + length[1]) + "}"
        _add(
            "substring table: offset " + off[0] + ", length " + length[0],
            "i=1; v=abcdef; echo \"[" + expr + "]\"; echo \"status=$?\"",
            ["expansion.parameter.substring"],
        )

# --- ${x^} ${x^^} ${x,} ${x,,} ${x~} ${x~~} --------------------------------------------------

CASE_OPS = [("^", "upper first"), ("^^", "upper all"), (",", "lower first"), (",,", "lower all"),
            ("~", "toggle first"), ("~~", "toggle all")]
CASE_PATTERNS = [("none", ""), ("letter", "l"), ("bracket", "[aeiou]"), ("question", "?"),
                 ("star", "*"), ("class", "[[:upper:]]"), ("capital", "H"), ("quoted", "'o'")]
CASE_SUBJECTS = [
    ("upper", "v='HELLO wORLD'", "v"),
    ("unicode", "v='élan ÀÉ ß'", "v"),
    ("array at", "a=(hello World oLd)", "a[@]"),
    ("positional", "set -- hello World oLd", "@"),
    ("empty", "v=", "v"),
    ("assoc at", "declare -A m=([k]=hello)", "m[@]"),
]
for subject, op, pattern in _pairwise([CASE_SUBJECTS, CASE_OPS, CASE_PATTERNS[:4]], 17):
    _add(
        "case " + op[1] + ": " + subject[0] + ", pattern " + pattern[0],
        P + "; " + subject[1] + "; p \"${" + subject[2] + op[0] + pattern[1] + "}\"",
        ["expansion.parameter.case"],
    )
for op in CASE_OPS:
    for pattern in CASE_PATTERNS[:6]:
        _add(
            "case table " + op[1] + ": pattern " + pattern[0],
            "v='hello World lol'; echo \"${v" + op[0] + pattern[1] + "}\"",
            ["expansion.parameter.case"],
        )

# --- ${x@op} --------------------------------------------------------------------------------

TRANSFORMS = ["Q", "E", "A", "a", "K", "k", "u", "U", "L"]
TRANSFORM_SUBJECTS = [
    ("plain", "v=plain", "v"),
    ("quotes and spaces", "v=\"it's a \\\"test\\\"\"", "v"),
    ("control characters", "v=$'tab\\there\\nnl'", "v"),
    ("unicode", "v='héllo wörld'", "v"),
    ("empty", "v=", "v"),
    ("unset", "unset v", "v"),
    ("integer", "declare -i v=42", "v"),
    ("lowercase attribute", "declare -l v=MiXeD", "v"),
    ("array whole", "a=(one 'two three' '')", "a"),
    ("array at", "a=(one 'two three' '')", "a[@]"),
    ("array star", "a=(one 'two three')", "a[*]"),
    ("array element", "a=(one 'two three')", "a[1]"),
    ("assoc whole", "declare -A m=([key]='v 1')", "m"),
    ("assoc at", "declare -A m=([key]='v 1')", "m[@]"),
    ("assoc element", "declare -A m=([key]='v 1')", "m[key]"),
    ("positional at", "set -- one 'two three'", "@"),
    ("positional star", "set -- one 'two three'", "*"),
    ("nameref", "t=target; declare -n v=t", "v"),
]
for number, subject in enumerate(TRANSFORM_SUBJECTS):
    for op in TRANSFORMS:
        if op in ("u", "U", "L", "k", "E") and number % 3 or op == "K" and number % 2:
            continue
        _add(
            "transform @" + op + ": " + subject[0],
            P + "; " + subject[1] + "; p \"${" + subject[2] + "@" + op + "}\"; echo \"status=$?\"",
            ["expansion.parameter.transform"],
        )
for op in TRANSFORMS:
    _add("transform @" + op + ": unquoted splitting", P + "; v='a  b'; a=('x y' z); p ${v@" + op + "} ${a[@]@" + op + "}",
         ["expansion.parameter.transform"])
for bad in ["Z", "QQ", "", "q", "1"]:
    _add("transform bad operator " + (bad or "empty"),
         "v=x; echo before; echo \"${v@" + bad + "}\"; echo \"after status=$?\"",
         ["expansion.parameter.transform", "error"])

# --- ${!name} ${!prefix*} ${!prefix@} ${!a[@]} ---------------------------------------------

INDIRECT = [
    ("scalar", "t=value; r=t"),
    ("unset target", "unset t; r=t"),
    ("empty name", "r="),
    ("array element", "a=(x 'y z'); r='a[1]'"),
    ("array at", "a=(x 'y z'); r='a[@]'"),
    ("array star", "a=(x 'y z'); r='a[*]'"),
    ("positional", "set -- p1 p2; r=2"),
    ("positional at", "set -- p1 'p 2'; r=@"),
    ("hash", "set -- p1 p2; r=#"),
    ("invalid name", "r='a b'"),
    ("nameref target", "t=deep; declare -n n=t; r=n"),
    ("assoc element", "declare -A m=([k]=mv); r='m[k]'"),
    ("chained", "t2=end; t=t2; r=t"),
    ("digits", "r=12"),
    ("bad subscript", "r='a['"),
]
IND_FORMS = [
    ("plain", "${!r}"), ("default", "${!r:-def}"), ("alternative", "${!r:+alt}"),
    ("trim", "${!r#?}"), ("quote", "${!r@Q}"),
]
for (label, setup), (fname, form) in _pairwise([INDIRECT, IND_FORMS], 19)[::2]:
    _add("indirect " + fname + ": " + label,
         P + "; " + setup + "; p \"" + form + "\"; echo \"status=$?\"",
         ["param.indirect"])
for label, setup in INDIRECT:
    _add("indirect bare: " + label, P + "; " + setup + "; p ${!r}; echo \"status=$?\"", ["param.indirect"])

PREFIX_SETUPS = [
    ("scalars", "pre_a=1 pre_b=2 prf=3"),
    ("with array", "pre_a=1; pre_arr=(x y); declare -A pre_m=([k]=v)"),
    ("none match", "other=1"),
    ("unset declared", "declare pre_x; pre_y=1"),
    ("underscore prefix", "_pre=1 _pre2=2"),
]
for label, setup in PREFIX_SETUPS:
    for form in ["${!pre*}", "\"${!pre*}\"", "\"${!pre@}\""]:
        for ifs in ("", "IFS=-; "):
            _add("prefix names " + form + (" with IFS" if ifs else "") + ": " + label,
                 P + "; " + setup + "; " + ifs + "p " + form,
                 ["expansion.parameter.names"])

KEY_SETUPS = [
    ("dense", "a=(x y z)"),
    ("sparse", "a=([3]=x [10]=y [7]=z)"),
    ("empty", "a=()"),
    ("scalar", "a=scalar"),
    ("unset", "unset a"),
    ("assoc one key", "declare -A a=(['k 1']=v)"),
    ("after unset element", "a=(x y z); unset 'a[1]'"),
    ("negative assign", "a=(x y z); a[-1]=Z"),
]
for label, setup in KEY_SETUPS:
    for form in ["\"${!a[@]}\"", "${!a[*]}", "\"${!a[*]}\""]:
        _add("array keys " + form + ": " + label, P + "; " + setup + "; IFS=,; p " + form,
             ["expansion.parameter.names"])

# --- ${#x} ----------------------------------------------------------------------------------

for label, setup, form in [
    ("unicode", "v=héllo", "${#v}"), ("empty", "v=", "${#v}"), ("unset", "unset v", "${#v}"),
    ("array count", "a=(a bb '')", "${#a[@]}"), ("array star count", "a=(a bb '')", "${#a[*]}"),
    ("array element", "a=(a bb '')", "${#a[1]}"), ("array bare", "a=(abc d)", "${#a}"),
    ("sparse count", "a=([5]=x [9]=y)", "${#a[@]}"), ("missing element", "a=(x)", "${#a[4]}"),
    ("negative element", "a=(x yy)", "${#a[-1]}"), ("positional count", "set -- a b c", "${#@}"),
    ("positional star", "set -- a b c", "${#*}"), ("positional one", "set -- abc", "${#1}"),
    ("hash of hash", "set -- a b", "${##}"), ("status", "false", "${#?}"),
    ("assoc count", "declare -A m=([a]=1 [b]=22)", "${#m[@]}"), ("assoc element", "declare -A m=([a]=1 [b]=22)", "${#m[b]}"),
    ("integer", "declare -i v=12345", "${#v}"), ("newline", "v=$'a\\nb'", "${#v}"),
    ("emoji", "v='a😀b'", "${#v}"), ("invalid utf8", "v=$'\\xff\\xfe'", "${#v}"),
    # `$$` differs from run to run, so its length is checked against the length of its value.
    ("dollar", "p=$$", "$(( ${#$} == ${#p} ))"), ("dash", "set -u", "${#-}"), ("zero", "", "${#0}"),
]:
    _add("length: " + label, (setup + "; " if setup else "") + "echo \"" + form + "\"",
         ["expansion.parameter.length"])

# --- Bad substitutions and malformed operators ------------------------------------------------

for label, expr in [
    ("colon only", "${v:}"), ("double colon", "${v::}"), ("bare bang", "${!}x"), ("unclosed subscript", "${a[}"),
    ("triple caret", "${v^^^}"), ("space in name", "${v w}"), ("digit name", "${1a}"),
    ("empty braces", "${}"), ("hash bang", "${#!v}"), ("plus colon", "${v+:x}"),
    ("dash only", "${-}"), ("at subscript on scalar", "${v[@]}"), ("slash only", "${v/}"),
    ("percent hash", "${v%#}"), ("comma caret", "${v,^}"), ("bang at", "${!@}"),
    ("bang star", "${!*}"), ("nested name", "${${v}}"), ("brackets only", "${[0]}"),
    ("length of substring", "${#v:1}"), ("length default", "${#v:-x}"),
]:
    _add("bad substitution: " + label,
         "v=abcd; a=(x); set -- p q; echo before; echo \"" + expr + "\"; echo \"after status=$?\"",
         ["error"])
    if len(label) % 3 == 0:
        _add("bad substitution in a subshell: " + label,
             "v=abcd; a=(x); ( echo \"" + expr + "\" ); echo \"status=$?\"", ["error"])

# --- Nesting operators ------------------------------------------------------------------------

NEST = [
    "${v:-${w:-${x:-deep}}}", "${v:+[${w}]}", "${v#${w}}", "${v/${w}/${x}}", "${v:${#w}:1}",
    "${v:-$(echo sub)}", "${v:-`echo back`}", "${v:-$((1+2))}", "${v:-\"${w}\"}", "${v:-'${w}'}",
    "${v:-\\${w}}", "${v//${w:0:1}/-}", "${v%\"${w#?}\"}", "${#v}${#w}", "${v:+${w:+both}}",
    "${v:-a${w}b}", "${v:-${w}${w}}", "${a[${#a[@]}-1]}", "${a[i+1]:-none}", "${v:-~}",
    "${v:-{a,b}}", "${v:-*}", "${v:-\"*\"}", "${v:-a\\ b}", "${v:-$'t\\tt'}",
]
for index, expr in enumerate(NEST):
    for state, setup in [[("unset", "unset v"), ("set", "v=abcabc")][index % 2]]:
        _add("nested " + str(index + 1) + ": v " + state + ", " + expr,
             "cd /tmp; " + P + "; " + setup + "; w=bc; unset x; i=0; a=(q r s); p " + expr + "; p \"" + expr + "\"",
             ["expansion.parameter.default"])
