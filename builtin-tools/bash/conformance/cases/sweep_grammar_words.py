"""Grammar sweep: quoting, word splitting and IFS, `read`, brace and tilde expansion, and command
substitution, generated from fixed tables (see sweep_grammar_param.py for the scheme)."""
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
    name = "gram words " + name
    if name in _NAMES:
        raise ValueError("duplicate case name " + name)
    _NAMES.append(name)
    CASES.append((name, script, list(tags)))


P = "p() { printf '%s' \"$#\"; printf ' <%s>' \"$@\"; echo; }"

# --- $'...' escapes -------------------------------------------------------------------------

ANSI = [
    ("alert", "\\a"), ("backspace", "\\b"), ("escape e", "\\e"), ("escape E", "\\E"), ("form feed", "\\f"),
    ("newline", "\\n"), ("return", "\\r"), ("tab", "\\t"), ("vertical tab", "\\v"), ("backslash", "\\\\"),
    ("single quote", "\\'"), ("double quote", '\\"'), ("question", "\\?"), ("octal one digit", "\\7"),
    ("octal three digits", "\\101"), ("octal four digits", "\\1011"), ("octal overflow", "\\777"),
    ("hex two", "\\x41"), ("hex one", "\\x4"), ("hex then letter", "\\x41g"), ("hex none", "\\xZ"),
    ("hex three", "\\x414"), ("unicode four", "\\u00e9"), ("unicode short", "\\u41"),
    ("unicode smiley", "\\u263A"), ("unicode eight", "\\U0001F600"), ("unicode surrogate", "\\ud800"),
    ("control A", "\\cA"), ("control lower", "\\ca"), ("control question", "\\c?"), ("control at", "\\c@"),
    ("control bracket", "\\c["), ("control backslash", "\\c\\\\"), ("unknown escape", "\\z"),
    ("trailing backslash", "x\\\\"), ("nul", "a\\0b"), ("hex nul", "a\\x00b"), ("dollar", "\\$"),
    ("literal dollar brace", "${x}"), ("backquote", "`"),
]
for label, esc in ANSI:
    _add("ansi-c " + label,
         "x=$'<" + esc + ">'; echo ${#x}; printf '%q\\n' \"$x\"",
         ["syntax.quote.ansi-c"])

# ANSI-C quoting in unusual places.
for label, script in [
    ("as a case pattern", "case $'a\\tb' in $'a\\tb') echo tab-match;; *) echo none;; esac"),
    ("in a here-document is literal", "cat <<EOF\n$'a\\tb'\nEOF"),
    ("inside double quotes is literal", "echo \"$'a\\tb'\""),
    ("in an array", "a=($'x\\ty' $'z'); echo ${#a[0]} ${#a[@]}"),
    ("in a default word", "unset u; x=${u:-$'a\\tb'}; echo ${#x}"),
    ("in a double quoted default word", "unset u; x=\"${u:-$'a\\tb'}\"; echo \"$x\""),
    ("as a redirect target", "cd /tmp; echo hi > $'f\\x41'; cat fA"),
    ("as an IFS", "IFS=$'\\t'; v=$'a\\tb c'; set -- $v; echo $#"),
    ("in arithmetic", "echo $(( $'1' + 2 ))"),
    ("adjacent to other quotes", "echo $'a'\"b\"'c'd$'e'"),
    ("empty", "x=$''; echo \"[${x}]\" ${#x}"),
    ("in a [[ pattern", "[[ $'a\\nb' == a?b ]] && echo q-match"),
    ("in a function name call", "f() { echo called; }; $'f'"),
    ("as an assignment name", "$'x'=1 2>&1; echo status=$?"),
    ("locale string", "echo $\"plain\" $\"with $HOSTNAME_UNSET\""),
]:
    _add("ansi-c " + label, script, ["syntax.quote.ansi-c"])

# --- Backslashes and double quotes ----------------------------------------------------------

BACKSLASH_WORDS = [
    ("unquoted letters", "\\a\\b"), ("unquoted backslash", "\\\\"), ("unquoted dollar", "\\$x"),
    ("unquoted double quote", '\\"'), ("unquoted single quote", "\\'"), ("unquoted space", "a\\ b"),
    ("unquoted glob", "\\*"), ("unquoted newline", "a\\\nb"), ("unquoted tab", "a\\\tb"),
    ("double quoted letters", '"\\a\\b"'), ("double quoted backslash", '"\\\\"'),
    ("double quoted dollar", '"\\$x"'), ("double quoted backquote", '"\\`"'),
    ("double quoted quote", '"\\""'), ("double quoted newline", '"a\\\nb"'),
    ("double quoted single quote", '"\\\'"'), ("single quoted backslash", "'\\\\'"),
    ("single quoted backslash n", "'a\\nb'"), ("mixed", "a\\'b\"c\\\"d\"'e\\f'"),
    ("backslash before brace", "\\{a,b}"), ("backslash tilde", "\\~"), ("backslash hash", "\\#x"),
    ("backslash semicolon", "a\\;b"), ("backslash pipe", "a\\|b"), ("backslash ampersand", "a\\&b"),
    ("backslash parenthesis", "\\(x\\)"), ("backslash less", "\\<x"), ("backslash equals", "x\\=y"),
    ("double quoted exclamation", '"a!b"'), ("double quoted history like", '"!!"'),
]
for label, word in BACKSLASH_WORDS:
    _add("backslash " + label, "cd /tmp; " + P + "; x=X; p " + word, ["syntax.quote.escape"])
    _add("backslash in assignment " + label, "cd /tmp; " + P + "; x=X; v=" + word + "; p \"$v\"",
         ["syntax.quote.escape"])

# Quote removal and concatenation.
for label, words in [
    ("empty quotes are words", "'' \"\" $'' a''b"),
    ("adjacent quote kinds", "'a'\"b\"$'c'd\\e"),
    ("quoted empty variable", "\"$e\" $e \"${e}\"x"),
    ("dollar at in the middle of a word", "x\"$@\"y"),
    ("dollar star in the middle of a word", "x\"$*\"y"),
    ("array at in the middle of a word", "x\"${a[@]}\"y"),
    ("array at unquoted in a word", "x${a[@]}y"),
    ("quoted and unquoted halves", "\"$v\"$v"),
    ("single quotes inside double quotes", "\"'$v'\""),
    ("double quotes inside single quotes", "'\"$v\"'"),
    ("dollar before quotes", "\"$\"'$' $"),
    ("dollar at end of a quoted word", "\"a$\""),
    ("braces with quotes inside", "\"${v:-'x'}\" ${v:-\"x\"}"),
    ("quoted pattern chars", "\"*\" '?' \\[a]"),
    ("unquoted empty at", "$@"),
    ("quoted at with empty elements", "\"${a[@]}\""),
    ("quoted star with custom IFS", "\"${a[*]}\""),
]:
    for setup in ["set -- 'one two' ''; a=('a b' '' c); v='x  y'; e=",
                  "set --; a=(); unset v; e=; IFS=:"]:
        _add("quote removal " + label + (" empty" if "set --;" in setup else " filled"),
             "cd /tmp; " + P + "; " + setup + "; p " + words,
             ["syntax.quote.double", "syntax.quote.single"])

# --- Word splitting and IFS ------------------------------------------------------------------

IFS_VALUES = [
    ("unset", "unset IFS"), ("empty", "IFS="), ("default", "IFS=$' \\t\\n'"), ("colon", "IFS=:"),
    ("space colon", "IFS=' :'"), ("colon comma", "IFS=:,"), ("newline", "IFS=$'\\n'"),
    ("letters", "IFS=ab"), ("star", "IFS='*'"), ("space", "IFS=' '"), ("multibyte", "IFS=é"),
]
SPLIT_VALUES = [
    ("colon list", "'a:b:c'"), ("colon edges", "':a::b:'"), ("spaces", "'  a  b  '"),
    ("space colon mix", "' : a : b : '"),
    ("newlines", "$'a\\nb c\\n'"), ("letters", "'xaybz'"), ("star", "'a*b'"), ("empty", "''"),
    ("multibyte", "'aébéc'"),
    ("only delimiters", "':::'"),
]
SPLIT_CONTEXTS = [
    ("set", "set -- $v; p \"$@\""),
    ("array", "arr=($v); p \"${arr[@]}\""),
    ("for", "for w in $v; do printf '<%s>' \"$w\"; done; echo"),
    ("star join", "set -- $v; p \"$*\""),
    ("read three", "read -r x y z <<< \"$v\"; p \"$x\" \"$y\" \"$z\""),
    ("read array", "read -ra arr <<< \"$v\"; p \"${arr[@]}\""),
]
for (iname, iset), (vname, value), (cname, ctx) in _pairwise([IFS_VALUES, SPLIT_VALUES, SPLIT_CONTEXTS], 23):
    _add("split " + cname + ": IFS " + iname + ", " + vname,
         "cd /tmp; " + P + "; v=" + value + "; " + iset + "; " + ctx,
         ["expansion.word-splitting"])
# The plainest context, for a spread of pairs.
for iname, iset in IFS_VALUES[::3]:
    for vname, value in SPLIT_VALUES[1::3]:
        _add("split table: IFS " + iname + ", " + vname,
             "cd /tmp; " + P + "; v=" + value + "; " + iset + "; p $v",
             ["expansion.word-splitting"])

# IFS scoping and special cases.
for label, script in [
    ("temporary IFS on read", "v='a:b'; IFS=: read -r x y <<< \"$v\"; echo \"$x|$y\"; set -- $v; echo $#"),
    ("temporary IFS on a function", "f() { set -- $1; echo $#; }; IFS=: f 'a:b:c'; f 'a:b:c'"),
    ("local IFS", "f() { local IFS=,; set -- $1; echo $#; }; f 'a,b'; v='a,b'; set -- $v; echo $#"),
    ("IFS in a subshell", "( IFS=:; v='a:b'; set -- $v; echo $# ); v='a:b'; set -- $v; echo $#"),
    ("unset IFS inside a function", "f() { unset IFS; v='a b'; set -- $v; echo $#; }; IFS=:; f; v='a b'; set -- $v; echo $#"),
    ("IFS applies to dollar star", "set -- a b c; IFS=; echo \"$*\"; IFS=-+; echo \"$*\"; unset IFS; echo \"$*\""),
    ("IFS applies to array star", "a=(a b c); IFS=/; echo \"${a[*]}\" \"${a[*]:1}\" \"${!a[*]}\""),
    ("IFS and dollar at unquoted", "set -- 'a b' c; IFS=:; p() { echo $#; }; p $@; p $*; p \"$@\""),
    ("IFS whitespace collapse", "v=$'a \\t\\n b'; set -- $v; echo $#"),
    ("IFS non-whitespace keeps empties", "IFS=,; v='a,,b,'; set -- $v; echo $#; printf '<%s>' \"$@\"; echo"),
    ("IFS trailing whitespace and delimiter", "IFS=' ,'; v='a , b , '; set -- $v; echo $#; printf '<%s>' \"$@\"; echo"),
    ("IFS leading delimiter", "IFS=,; v=',a'; set -- $v; echo $#; printf '<%s>' \"$@\"; echo"),
    ("splitting of arithmetic result", "IFS=1; set -- $((121)); echo $#; printf '<%s>' \"$@\"; echo"),
    ("splitting of tilde result", "IFS=/; HOME=/a/b; set -- ~; echo $#"),
    ("no splitting in assignments", "v='a b'; w=$v; echo \"$w\"; declare x=$v; echo \"$x\"; local_like=$(echo 'c  d'); echo \"$local_like\""),
    ("no splitting in [[", "v='a  b'; [[ $v == 'a  b' ]] && echo kept"),
    ("no splitting in case word", "v='a  b'; case $v in 'a  b') echo kept;; esac"),
    ("splitting in declare arguments", "v='x=1 y=2'; declare $v; echo \"$x $y\""),
    ("splitting in export arguments", "v='p=1 q'; export $v; echo \"${p-} ${q-unset}\""),
    ("splitting in local arguments", "f() { v='m=1 n=2'; local $v; echo \"$m $n\"; }; f"),
    ("here-string does not split", "v='a  b'; cat <<< $v"),
    ("redirect word does not split", "cd /tmp; v='f  g'; echo x > \"$v\"; cat 'f  g'"),
    ("IFS read with backslash", "IFS=: read x y <<< 'a\\:b:c'; echo \"$x|$y\"; IFS=: read -r x y <<< 'a\\:b:c'; echo \"$x|$y\""),
    ("IFS with glob characters", "cd /tmp; touch ab; IFS='*'; v='a*b'; set -- $v; echo $# \"$1\" \"$2\""),
    ("splitting result is globbed", "cd /tmp; touch s1 s2; v='s* x'; set -- $v; echo $#"),
    ("noglob stops globbing after splitting", "cd /tmp; touch s1 s2; set -f; v='s* x'; set -- $v; echo $# \"$1\""),
    ("empty IFS keeps dollar at separate", "set -- 'a b' c; IFS=; for w in $@; do echo \"[$w]\"; done"),
    ("empty IFS joins dollar star", "set -- 'a b' c; IFS=; for w in $*; do echo \"[$w]\"; done"),
    ("empty IFS joins quoted dollar star", "set -- 'a b' c; IFS=; for w in \"$*\"; do echo \"[$w]\"; done"),
    ("unset IFS joins with space", "set -- a b; unset IFS; echo \"$*\"; a=(x y); echo \"${a[*]}\""),
    ("IFS first character joins", "set -- a b; IFS=' :'; echo \"$*\"; IFS=': '; echo \"$*\""),
    ("IFS multibyte first character joins", "set -- a b; IFS=é; echo \"$*\""),
]:
    _add("ifs " + label, P + "; " + script, ["expansion.word-splitting"])

# --- read -------------------------------------------------------------------------------------

READ_INPUTS = [
    ("plain", "'a b c'"), ("extra fields", "'a b c d e'"), ("fewer fields", "'a'"),
    ("leading spaces", "'   a b'"), ("trailing spaces", "'a b   '"), ("backslash", "'a\\\\ b c'"),
    ("line continuation", "$'a \\\\\\nb c'"), ("tabs", "$'a\\tb\\tc'"), ("empty", "''"),
    ("colons", "'a:b:c'"), ("no newline", "NONL"),
]
READ_FORMS = [
    ("two vars", "read x y; p \"$x\" \"$y\""),
    ("raw two vars", "read -r x y; p \"$x\" \"$y\""),
    ("reply", "read; p \"$REPLY\""),
    ("raw reply", "read -r; p \"$REPLY\""),
    ("array", "read -a arr; p \"${arr[@]}\""),
    ("colon IFS", "IFS=: read -r x y; p \"$x\" \"$y\""),
    ("empty IFS", "IFS= read -r x; p \"$x\""),
    ("delimiter space", "read -r -d ' ' x; p \"$x\""),
    ("count two", "read -r -n 2 x; p \"$x\""),
    ("exact count", "read -r -N 3 x; p \"$x\""),
    ("count with vars", "read -r -n 4 x y; p \"$x\" \"$y\""),
]
for (iname, inp), (fname, form) in [(READ_INPUTS[i], READ_FORMS[(i * 3 + k) % len(READ_FORMS)])
                                     for i in range(len(READ_INPUTS)) for k in range(5)]:
    if inp == "NONL":
        feed = "printf 'a b' | { " + form + "; echo \"status=$?\"; }"
    else:
        feed = "{ " + form + "; echo \"status=$?\"; } <<< " + inp
    _add("read " + fname + ": " + iname, P + "; " + feed, ["builtin.read"])
for label, script in [
    ("returns false at end of input", "read x < /dev/null; echo \"status=$? [$x]\""),
    ("partial last line", "printf 'a\\nb' | { while read -r l; do echo \"[$l]\"; done; echo \"last=[$l]\"; }"),
    ("partial last line idiom", "printf 'a\\nb' | while read -r l || [[ -n $l ]]; do echo \"[$l]\"; done"),
    ("from a file descriptor", "exec 3<<< 'fd line'; read -r -u 3 x; echo \"$x\"; exec 3<&-"),
    ("delimiter empty reads to nul", "printf 'a\\0b\\0' | { read -r -d '' x; read -r -d '' y; echo \"$x|$y\"; }"),
    ("invalid variable name", "read 1x <<< 'a'; echo status=$?"),
    ("invalid option", "read -Z x <<< 'a'; echo status=$?"),
    ("missing option argument", "read -d; echo status=$?"),
    ("into array element", "read 'a[2]' <<< 'elem'; echo \"${a[2]}\""),
    ("into readonly variable", "readonly r=1; read r <<< 'x'; echo status=$? r=$r"),
    ("prompt without a terminal", "read -p 'prompt> ' x <<< 'in'; echo \"[$x]\""),
    ("silent flag without a terminal", "read -s x <<< 'secret'; echo \"[$x]\""),
    ("n zero", "read -n 0 x <<< 'abc'; echo \"status=$? [$x]\""),
    ("n with backslash", "read -n 3 x <<< 'a\\bc'; echo \"[$x]\""),
    ("N ignores delimiter", "read -N 4 -d b x <<< 'abcdef'; echo \"[$x]\""),
    ("multiple lines one read", "{ read a; read b; } <<< $'1\\n2\\n3'; echo \"$a $b\""),
    ("read in a pipeline does not persist", "echo piped | read x; echo \"[${x-unset}]\""),
    ("read with lastpipe persists", "shopt -s lastpipe; echo piped | read x; echo \"[${x-unset}]\""),
    ("unicode count", "read -n 2 x <<< 'éèa'; echo \"[$x]\""),
    ("read -a with IFS comma", "IFS=, read -ra arr <<< 'a,,b,'; echo ${#arr[@]}; printf '<%s>' \"${arr[@]}\"; echo"),
    ("assoc target refused", "declare -A m; read -a m <<< 'a b'; echo status=$?"),
]:
    _add("read " + label, P + "; " + script, ["builtin.read"])

# --- mapfile / readarray --------------------------------------------------------------------

MAPFILE_OPTS = [
    ("plain", ""), ("trim", "-t"), ("count", "-n 2"), ("skip", "-s 1"), ("origin", "-O 5"),
    ("delimiter", "-d ,"), ("trim delimiter", "-t -d ,"), ("skip and count", "-s 1 -n 1 -t"),
    ("nul delimiter", "-d ''"), ("count zero", "-n 0 -t"),
]
MAPFILE_INPUTS = [
    ("lines", "$'a\\nb\\nc\\n'"), ("commas", "'a,b,,c'"), ("no trailing newline", "$'a\\nb'"),
    ("empty", "''"), ("blank lines", "$'\\n\\nx\\n'"),
]
for (oname, opt), (iname, inp) in [(MAPFILE_OPTS[i], MAPFILE_INPUTS[(i + k) % len(MAPFILE_INPUTS)])
                                   for i in range(len(MAPFILE_OPTS)) for k in range(3)]:
    for builtin in ("mapfile", "readarray"):
        if builtin == "readarray" and (len(oname) + len(iname)) % 4:
            continue
        _add(builtin + " " + oname + ": " + iname,
             "arr=(keep0 keep1); printf '%s' " + inp + " | { " + builtin + " " + opt + " arr; echo \"status=$? n=${#arr[@]}\"; declare -p arr; }",
             ["builtin.mapfile"] if builtin == "mapfile" else ["builtin.readarray"])
for label, script in [
    ("callback", "mapfile -t -C 'echo cb' -c 2 arr <<< $'a\\nb\\nc\\nd'; echo ${#arr[@]}"),
    ("callback quantum one", "f() { echo \"at $1: $2\"; }; mapfile -t -C f -c 1 arr <<< $'x\\ny'"),
    ("default MAPFILE", "mapfile <<< $'p\\nq'; echo ${#MAPFILE[@]} \"${MAPFILE[1]}\""),
    ("from a file descriptor", "exec 4<<< $'u\\nv'; mapfile -t -u 4 arr; echo \"${arr[*]}\""),
    ("invalid array name", "mapfile 1bad <<< x; echo status=$?"),
    ("negative count", "mapfile -n -1 arr <<< x; echo status=$?"),
    ("into an associative array", "declare -A m; mapfile m <<< x; echo status=$?"),
    ("process substitution", "mapfile -t arr < <(printf '%s\\n' one two); echo \"${arr[1]}\""),
    ("keeps existing elements beyond", "arr=(a b c d); mapfile -t -O 1 arr <<< 'X'; declare -p arr"),
    ("delimiter longer than one character", "mapfile -t -d ab arr <<< 'xaybzac'; declare -p arr"),
]:
    _add("mapfile " + label, script, ["builtin.mapfile"])

# --- Brace expansion ------------------------------------------------------------------------

BRACES = [
    "{a,b}", "{a,,b}", "{,a}", "{a,}", "{a}", "{}", "x{a,b}y", "{a,b}{c,d}", "{a,{b,c}}", "{{a,b},c}",
    "{1..5}", "{5..1}", "{1..10..3}", "{10..1..3}", "{10..1..-3}", "{1..10..-3}", "{01..10}", "{1..010}",
    "{-3..3}", "{-03..3}", "{3..-3..2}", "{a..e}", "{e..a}", "{a..z..5}", "{A..c}", "{Z..a}", "{1..a}",
    "{1..}", "{..1}", "{1..3..0}", "{a..e..-2}", "\"{a,b}\"", "\\{a,b}", "{a\\,b,c}", "{a,b}.{1..2}",
    "a{b..d}e", "{1..3}{a..b}", "{0..2}{0..1}", "{a,b}{}", "{$v,x}", "$v{a,b}", "${v}{a,b}", "{a,b}$v",
    "{'a,b',c}", "{\"x y\",z}", "{a..c}{1,2}", "{9..11}", "{-1..-5..2}", "{x..x}", "{1..1}", "{aa..cc}",
    "{a..b..c}", "{1.5..3}", "{a,b}}", "{{a,b}", "a{,}b", "{,}", "{ a,b }", "{a, b}", "{~,x}",
    "{1..3}\\ x", "{@,#}", "{a..e..0}", "{0x1..0x3}", "{+1..3}", "{1..+3}", "{00..3}", "{-00..2}",
    "{9223372036854775806..9223372036854775807}", "{a,b}[0-9]", "{*,?}",
]
for brace in BRACES:
    _add("brace " + brace, "cd /tmp; " + P + "; v=V; p " + brace, ["expansion.brace"])
for label, script in [
    ("in an array assignment", "a=({a,b}{1,2}); echo ${#a[@]} ${a[3]}"),
    ("not in a scalar assignment", "x={a,b}; echo \"$x\""),
    ("in a for loop", "for i in {1..3}{x,y}; do printf '%s ' $i; done; echo"),
    ("not in [[", "[[ a == {a,b} ]] && echo yes || echo no"),
    ("in case words", "case {a,b} in '{a,b}') echo literal;; *) echo other;; esac"),
    ("in a redirect target is ambiguous", "cd /tmp; echo x > {a,b}.txt; echo status=$?; echo *"),
    ("braceexpand off", "set +B; echo {a,b} {1..2}; set -B; echo {a,b}"),
    ("with command substitution", "echo {$(echo a),b}"),
    ("sequence from variables is literal", "n=3; echo {1..$n}"),
    ("eval makes it work", "n=3; eval echo {1..$n}"),
    ("generated words then globbing", "cd /tmp; touch b1 b2; echo {b,c}*"),
    ("with tilde", "HOME=/h; echo {~,~/x}"),
    ("large sequence count", "set -- {1..1000}; echo $# ${1000}"),
    ("nested sequence", "echo {a,{1..3},b}"),
    ("with quoted comma", "echo {a',b',c}"),
    ("in function arguments", "f() { echo $#; }; f {x,y,z}"),
    ("in a here-document is literal", "cat <<EOF\n{a,b}\nEOF"),
    ("in double quotes in a word", "echo x\"{a,b}\"y {a,b}\"q\""),
    ("with parameter expansion inside", "v=1; echo {${v},2}"),
    ("after a dollar", "echo ${a,b} 2>/dev/null; echo \\${a,b}; echo \\$\\{a,b\\}"),
]:
    _add("brace " + label, script, ["expansion.brace"])

# --- Tilde expansion --------------------------------------------------------------------------

TILDE = [
    "~", "~/x", "~+", "~+/x", "~-", "~-/x", "x~", "~nonexistentuser", "\"~\"", "'~'", "\\~",
    "a:~/b", "~:~", "--opt=~", "~/", "~//x", "~+0", "~1",
]
HOME_DEPENDENT = ["~", "~/x", "a:~/b", "~:~", "--opt=~", "~/", "~//x"]
for home in [("unset", "unset HOME"), ("set", "HOME=/h/me"), ("empty", "HOME="), ("slash", "HOME=/")]:
    for word in TILDE:
        if word not in HOME_DEPENDENT and home[0] != "unset":
            continue
        _add("tilde " + word + ": HOME " + home[0],
             "cd /tmp; OLDPWD=/old; " + home[1] + "; " + P + "; p " + word,
             ["expansion.tilde"])
for label, script in [
    ("in an assignment after a colon", "HOME=/h; x=a:~/b:~; echo \"$x\""),
    ("in an assignment argument", "HOME=/h; declare x=~/y; echo \"$x\"; export z=a:~; echo \"$z\""),
    ("in a default word", "HOME=/h; unset u; echo ${u:-~} \"${u:-~}\""),
    ("in an array element", "HOME=/h; a=(~ x~ ~/z); echo \"${a[@]}\""),
    ("in a case pattern", "HOME=/h; case /h in ~) echo matched;; *) echo no;; esac"),
    ("in [[", "HOME=/h; [[ ~ == /h ]] && echo equal"),
    ("in a redirect target", "HOME=/tmp; echo t > ~/tf; cat /tmp/tf"),
    ("after cd", "cd /tmp; cd /; echo ~- ~+"),
    ("directory stack", "cd /tmp; pushd / >/dev/null; echo ~1 ~+1 ~-1 ~0; popd >/dev/null"),
    ("in a here-string", "HOME=/h; cat <<< ~"),
    ("in a here-document is literal", "HOME=/h; cat <<EOF\n~\nEOF"),
    ("in a for list", "HOME=/h; for d in ~ ~/a; do echo $d; done"),
    ("HOME with trailing slash", "HOME=/h/; echo ~/x ~"),
    ("tilde in a function argument", "HOME=/h; f() { echo \"$1\"; }; f ~/p"),
    ("HOME changes mid script", "HOME=/one; a=~; HOME=/two; echo $a ~"),
]:
    _add("tilde " + label, script, ["expansion.tilde"])

# --- Command substitution --------------------------------------------------------------------

for label, script in [
    ("trailing newlines removed", "x=$(printf 'a\\n\\n'); echo \"[$x]\""),
    ("inner newlines kept", "x=$(printf 'a\\n\\nb'); echo \"[$x]\""),
    ("only newlines", "x=$(printf '\\n\\n'); echo \"[$x]\" ${#x}"),
    ("carriage return kept", "x=$(printf 'a\\r\\n'); echo ${#x}"),
    ("nul byte dropped with a warning", "x=$(printf 'a\\0b'); echo \"[$x]\" ${#x}"),
    ("nested three deep", "echo $(echo $(echo $(echo deep)))"),
    ("nested quotes", "echo \"$(echo \"$(echo 'a  b')\")\""),
    ("backquotes nested", "echo `echo \\`echo in\\``"),
    ("backquote backslash dollar", "v=V; echo `echo \\$v` `echo $v`"),
    ("backquote double backslash", "echo `echo a\\\\\\\\b`"),
    ("backquote in double quotes", "echo \"`echo 'x  y'`\""),
    ("case inside substitution", "echo $(case a in a) echo matched;; esac)"),
    ("case with paren inside substitution", "echo $(case a in (a) echo p;; esac)"),
    ("comment inside substitution", "echo $(echo a # comment )\n)"),
    ("here-document inside substitution", "x=$(cat <<EOF\nhd $((1+1))\nEOF\n); echo \"$x\""),
    ("subshell inside substitution", "echo $( (echo sub) )"),
    ("arithmetic lookalike", "echo $( (echo a); echo b )"),
    ("status of assignment", "x=$(exit 3); echo $?"),
    ("status of assignment with two substitutions", "x=$(exit 3)$(exit 4); echo $?"),
    ("status of local assignment", "f() { local x=$(exit 3); echo $?; }; f"),
    ("status of export assignment", "export x=$(exit 3); echo $?"),
    ("status of declare assignment", "declare x=$(exit 3); echo $?"),
    ("status of readonly assignment", "readonly x=$(exit 3); echo $?"),
    ("status of a command with substitution", "echo $(exit 3); echo $?"),
    ("status of empty command", "$(exit 5); echo $?"),
    ("substitution runs in a subshell", "x=1; : $(x=2); echo $x; $(cd /tmp); pwd"),
    ("exit inside substitution", "x=$(echo a; exit 2; echo b); echo \"$x $?\""),
    ("substitution output as command", "$(echo echo) hello"),
    ("file read shorthand", "cd /tmp; printf 'one\\ntwo\\n' > f; echo \"$(< f)\""),
    ("file read shorthand with spaces", "cd /tmp; printf 'z' > f; echo \"$( < f )\""),
    ("file read shorthand missing", "echo \"$(< /tmp/missing)\"; echo status=$?"),
    ("stderr is not captured", "x=$(echo out; echo err >&2); echo \"[$x]\""),
    ("stderr captured with redirect", "x=$(echo err >&2 2>&1); echo \"[$x]\"; y=$( { echo e >&2; } 2>&1 ); echo \"[$y]\""),
    ("in a here-document", "cat <<EOF\n$(echo sub) \\$(echo no)\nEOF"),
    ("in arithmetic", "echo $(( $(echo 3) * 2 ))"),
    ("unbalanced parenthesis in quotes", "echo $(echo ')')"),
    ("unbalanced parenthesis escaped", "echo $(echo \\))"),
    ("empty substitution", "echo \"[$()]\" [$( )]"),
    ("substitution with only a comment", "echo \"[$(# nothing\n)]\""),
    ("pipeline inside", "echo $(echo a b | while read x y; do echo $y$x; done)"),
    ("function defined inside does not escape", "x=$(f() { echo in; }; f); echo $x; type f >/dev/null 2>&1 || echo no-f"),
    ("large output", "x=$(for i in {1..2000}; do echo line$i; done); echo ${#x}"),
    ("BASH_SUBSHELL", "echo $BASH_SUBSHELL $(echo $BASH_SUBSHELL $(echo $BASH_SUBSHELL))"),
    ("trap reset inside", "trap 'echo trapped' EXIT; x=$(trap -p EXIT); echo \"[$x]\"; trap - EXIT"),
    ("set -e inherited only with inherit_errexit", "set -e; x=$(false; echo after); echo \"[$x]\"; shopt -s inherit_errexit; y=$(false; echo after2); echo \"[$y]\""),
    ("substitution in a case pattern", "case ab in $(echo a)*) echo pat;; esac"),
    ("substitution as a for list", "for w in $(echo 'x y'); do echo \"<$w>\"; done"),
    ("quoted substitution as a for list", "for w in \"$(echo 'x y')\"; do echo \"<$w>\"; done"),
    ("backquote with dollar parenthesis", "echo `echo $(echo mix)`"),
    ("dollar parenthesis with backquote", "echo $(echo `echo mix2`)"),
]:
    _add("command substitution " + label, script, ["expansion.command"])

# Bash 5.3's ${ cmd; } and ${| cmd; } forms.
for label, script in [
    ("funsub", "x=${ echo in-current; }; echo \"[$x]\""),
    ("funsub shares variables", "y=1; x=${ y=2; echo z; }; echo \"$x $y\""),
    ("valsub", "x=${| REPLY=val; }; echo \"[$x]\""),
]:
    _add("command substitution " + label, script, ["expansion.command"])
