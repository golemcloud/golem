"""Edge inputs: invalid UTF-8 and binary bytes, made with printf and $'...' inside the script, through
pipes, read, mapfile, command substitution, parameter expansion, the text filters and file names.

Generated from fixed tables with plain loops, so every Python 3 produces the same scripts.
"""

TIER = "sweep"

# Byte shapes as printf/$'...' escapes; each input has a second, plain line so sorters have work.
INPUTS = [
    ("lone ff", "a\\xffb\\nzz\\n"),
    ("truncated", "ab\\xc3\\nzz\\n"),
    ("surrogate", "a\\xed\\xa0\\x80b\\nzz\\n"),
    ("latin1", "caf\\xe9 ol\\xe9\\nzz\\n"),
    ("nul", "a\\000b\\nzz\\n"),
    ("crlf", "a b\\r\\nzz\\r\\n"),
    ("controls", "a\\001\\033[1mb\\177\\nzz\\n"),
]

HEX = " | od -An -tx1"
# (label, script) with {f} the printf format.
PIPE_OPS = [
    ("cat", "printf '{f}' | cat" + HEX),
    ("cat -v", "printf '{f}' | cat -v"),
    ("cat -A", "printf '{f}' | cat -A"),
    ("head -c", "printf '{f}' | head -c 3" + HEX),
    ("head -n", "printf '{f}' | head -n 1" + HEX),
    ("tail -c", "printf '{f}' | tail -c 5" + HEX),
    ("tail -n", "printf '{f}' | tail -n 1" + HEX),
    ("cut -b", "printf '{f}' | cut -b 2-3" + HEX),
    ("cut -c", "printf '{f}' | cut -c 2-3" + HEX),
    ("cut -f", "printf '{f}' | cut -d ' ' -f 2" + HEX),
    ("tr -d", "printf '{f}' | tr -d '\\377\\000'" + HEX),
    ("tr high range", "printf '{f}' | tr '\\200-\\377' '?'"),
    ("tr -c", "printf '{f}' | tr -c 'a-z\\n' '.'"),
    ("tr upper", "printf '{f}' | tr a-z A-Z" + HEX),
    ("tr -s", "printf '{f}' | tr -s 'z'" + HEX),
    ("sort", "printf '{f}' | sort" + HEX),
    ("sort -u", "printf '{f}' | sort -u" + HEX),
    ("uniq -c", "printf '{f}' | uniq -c" + HEX),
    ("wc -c", "printf '{f}' | wc -c"),
    ("wc -m", "printf '{f}' | wc -m"),
    ("wc -l", "printf '{f}' | wc -l"),
    ("wc -w", "printf '{f}' | wc -w"),
    ("wc -L", "printf '{f}' | wc -L"),
    ("grep -c dot", "printf '{f}' | grep -c ."),
    ("grep a", "printf '{f}' | grep a; echo status=$?"),
    ("grep -a", "printf '{f}' | grep -a a" + HEX),
    ("grep -c a", "printf '{f}' | grep -c a"),
    ("grep -v", "printf '{f}' | grep -v zz; echo status=$?"),
    ("grep -o", "printf '{f}' | grep -o '[a-z]'; echo status=$?"),
    ("grep -q", "printf '{f}' | grep -q a; echo status=$?"),
    ("grep -l", "printf '{f}' | grep -l a; echo status=$?"),
    ("grep -n", "printf '{f}' | grep -n z; echo status=$?"),
    ("sed any char", "printf '{f}' | sed 's/./X/g'" + HEX),
    ("sed -n l", "printf '{f}' | sed -n l"),
    ("sed s", "printf '{f}' | sed 's/a/A/'" + HEX),
    ("sed line count", "printf '{f}' | sed -n '$='"),
    ("sed y", "printf '{f}' | sed 'y/abz/ABZ/'" + HEX),
    ("rev", "printf '{f}' | rev" + HEX),
    ("tac", "printf '{f}' | tac" + HEX),
    ("fold", "printf '{f}' | fold -w 2" + HEX),
    ("nl", "printf '{f}' | nl" + HEX),
    ("paste -s", "printf '{f}' | paste -s -d ," + HEX),
    ("od -c", "printf '{f}' | od -c"),
    ("base64", "printf '{f}' | base64"),
    ("base64 round trip", "printf '{f}' | base64 | base64 -d" + HEX),
    ("cksum", "printf '{f}' | cksum"),
    ("expand", "printf '{f}' | expand" + HEX),
    ("file", "printf '{f}' | file -"),
    ("jq -R", "printf '{f}' | jq -R ." + HEX),
    ("jq -Rs length", "printf '{f}' | jq -Rs length"),
    ("tee", "printf '{f}' | tee /tmp/t >/dev/null; od -An -tx1 /tmp/t"),
    ("xargs", "printf '{f}' | xargs echo" + HEX),
    ("split", "cd /tmp && printf '{f}' | split -l 1 && cat xa*" + HEX),
    ("read -r", "printf '{f}' | {{ IFS= read -r l; printf %s \"$l\"" + HEX + "; }}"),
    ("read length", "printf '{f}' | {{ IFS= read -r l; echo \"${{#l}}\"; }}"),
    ("read loop", "printf '{f}' | while IFS= read -r l; do printf '%s|' \"$l\"; done" + HEX),
    ("read -a", "printf '{f}' | {{ read -r -a w; echo \"${{#w[@]}}\"; printf %s \"${{w[0]}}\"" + HEX + "; }}"),
    ("mapfile", "printf '{f}' | {{ mapfile -t a; echo \"${{#a[@]}}\"; printf %s \"${{a[0]}}\"" + HEX + "; }}"),
    ("command substitution", "x=$(printf '{f}'); printf %s \"$x\"" + HEX),
    ("command substitution length", "x=$(printf '{f}'); echo \"${{#x}}\""),
    ("file substitution", "printf '{f}' > /tmp/f; x=$(</tmp/f); printf %s \"$x\"" + HEX),
    ("ansi-c variable", "x=$'{f}'; printf %s \"$x\"" + HEX),
    ("ansi-c length", "x=$'{f}'; echo \"${{#x}}\""),
    ("ansi-c substring", "x=$'{f}'; printf %s \"${{x:1:2}}\"" + HEX),
    ("ansi-c replace", "x=$'{f}'; printf %s \"${{x//z/Z}}\"" + HEX),
    ("ansi-c upper", "x=$'{f}'; printf %s \"${{x^^}}\"" + HEX),
    ("ansi-c printf q", "x=$'{f}'; printf '%q\\n' \"$x\""),
    ("ansi-c transform Q", "x=$'{f}'; echo \"${{x@Q}}\""),
    ("ansi-c declare -p", "x=$'{f}'; declare -p x"),
    ("ansi-c echo", "x=$'{f}'; echo \"$x\"" + HEX),
    ("ansi-c herestring", "x=$'{f}'; od -An -tx1 <<< \"$x\""),
    ("ansi-c array", "a=($'{f}'); echo \"${{#a[@]}}\"; printf '%s|' \"${{a[@]}}\"" + HEX),
    ("ansi-c word split", "x=$'{f}'; set -- $x; echo $#"),
    ("ansi-c pattern", "x=$'{f}'; case $x in a*) echo a;; *) echo other;; esac"),
    ("ansi-c to file", "x=$'{f}'; printf '%s' \"$x\" > /tmp/f; wc -c < /tmp/f"),
    ("echo -e", "echo -e '{f}'" + HEX),
    ("printf %b", "printf '%b' '{f}'" + HEX),
    ("file operand", "printf '{f}' > /tmp/f; file /tmp/f"),
    ("stat size", "printf '{f}' > /tmp/f; stat -c %s /tmp/f"),
    ("diff", "printf '{f}' > /tmp/a; printf 'zz\\n' > /tmp/b; diff /tmp/a /tmp/b; echo status=$?"),
    ("diff -a", "printf '{f}' > /tmp/a; printf 'zz\\n' > /tmp/b; diff -a /tmp/a /tmp/b" + HEX),
    ("cmp", "printf '{f}' > /tmp/a; printf 'zz\\n' > /tmp/b; cmp /tmp/a /tmp/b; echo status=$?"),
    ("comm", "printf '{f}' > /tmp/a; comm -12 /tmp/a /tmp/a" + HEX),
    ("grep file", "printf '{f}' > /tmp/f; grep z /tmp/f; echo status=$?"),
    ("sed -i", "printf '{f}' > /tmp/f; sed -i 's/z/Q/' /tmp/f; od -An -tx1 /tmp/f"),
]

CASES = []
for op, template in PIPE_OPS:
    for shape, fmt in INPUTS:
        CASES.append((f"edge bytes: {op} {shape}", template.replace("{f}", fmt).replace("{{", "{").replace("}}", "}")))

# NUL-separated data and the -z / -0 options.
NUL = "printf 'b\\000a\\000c\\000'"
NUL_NOEND = "printf 'b\\000a\\000c'"
NUL_OPS = [
    ("sort -z", "sort -z"),
    ("sort -z -r", "sort -rz"),
    ("head -z", "head -z -n 2"),
    ("tail -z", "tail -z -n 1"),
    ("uniq -z", "sort -z | uniq -z -c"),
    ("grep -z", "grep -z a"),
    ("grep -z -c", "grep -zc ."),
    ("sed -z", "sed -z 's/^/>/'"),
    ("cut -z", "cut -z -c1"),
    ("xargs -0", "xargs -0 echo"),
    ("xargs -0 -n1", "xargs -0 -n 1 echo"),
    ("tr to newline", "tr '\\0' '\\n'"),
    ("read -d empty", "while IFS= read -r -d '' x; do echo \"[$x]\"; done"),
    ("mapfile -d empty", "{ mapfile -t -d '' a; echo \"${#a[@]} ${a[2]}\"; }"),
    ("paste -z", "paste -z -s -d ,"),
    ("nl -z", "nl -z"),
    ("wc -l", "wc -l"),
    ("tac -s", "tac -s ''"),
    ("jq -R", "jq -R ."),
    ("od", "od -An -c"),
]
for op, command in NUL_OPS:
    CASES.append((f"edge bytes nul: {op}", f"{NUL} | {command}" + HEX))
    CASES.append((f"edge bytes nul: {op} unterminated", f"{NUL_NOEND} | {command}" + HEX))

# File names with bytes that are not UTF-8, and names with control characters.
NAME_OPS = [
    ("touch invalid name", "touch \"$(printf '/tmp/n\\377')\"; echo status=$?; ls /tmp | od -An -c"),
    ("redirect invalid name", "echo x > $'/tmp/n\\xff'; echo status=$?; ls /tmp | od -An -c"),
    ("mkdir invalid name", "mkdir $'/tmp/d\\xc3'; echo status=$?; ls /tmp | od -An -c"),
    ("glob invalid name", "cd /tmp && : > $'n\\xff' && for f in *; do printf %s \"$f\" | od -An -tx1; done"),
    ("find invalid name", "cd /tmp && : > $'n\\xff'; find . -name 'n*' | od -An -c"),
    ("ls -b invalid name", "cd /tmp && : > $'n\\xff'; ls -b"),
    ("ls quoted invalid name", "cd /tmp && : > $'n\\xff'; ls --quoting-style=shell-escape"),
    ("rm invalid name", "cd /tmp && : > $'n\\xff'; rm $'n\\xff'; echo status=$?; ls | wc -l"),
    ("newline name", "cd /tmp && : > $'a\\nb'; ls | od -An -c; ls -b"),
    ("newline name glob", "cd /tmp && : > $'a\\nb'; for f in *; do printf '[%s]\\n' \"$f\"; done"),
    ("newline name find", "cd /tmp && : > $'a\\nb'; find . -name 'a*' -print0 | od -An -c"),
    ("tab name", "cd /tmp && : > $'a\\tb'; ls; ls -b"),
    ("escape name", "cd /tmp && : > $'a\\033b'; ls | od -An -c; ls -b"),
    ("backslash name", "cd /tmp && : > 'a\\b'; ls; ls --quoting-style=c"),
    ("dash name", "cd /tmp && : > ./-n; ls; cat -- -n; rm -- -n; ls | wc -l"),
    ("space-only name", "cd /tmp && : > ' '; ls | od -An -c"),
    ("quote names", "cd /tmp && : > \"it's\" && : > 'say \"hi\"'; ls; ls -Q"),
    ("glob chars in name", "cd /tmp && : > '[a]' && : > '*'; ls; printf '%s\\n' \\[*"),
    ("xargs -0 invalid name", "cd /tmp && : > $'n\\xff'; printf 'n\\377\\000' | xargs -0 ls"),
    ("basename invalid", "basename $'/tmp/a\\xffb/c\\xfe' | od -An -tx1"),
    ("dirname invalid", "dirname $'/tmp/a\\xffb/c' | od -An -tx1"),
    ("cd invalid name", "mkdir $'/tmp/d\\xff' && cd $'/tmp/d\\xff' && pwd | od -An -tx1"),
]
for op, script in NAME_OPS:
    CASES.append((f"edge bytes names: {op}", script))

# Patterns and arguments that carry invalid bytes.
ARG_OPS = [
    ("grep invalid pattern", "printf 'a\\377b\\nzz\\n' | grep -c $'\\xff'"),
    ("grep -a invalid pattern", "printf 'a\\377b\\nzz\\n' | grep -a $'\\xff'" + HEX),
    ("grep -F invalid pattern", "printf 'a\\377b\\nzz\\n' | grep -aF $'\\xff'" + HEX),
    ("sed invalid pattern", "printf 'a\\377b\\n' | sed $'s/\\xff/X/'"),
    ("sed invalid replacement", "printf 'ab\\n' | sed $'s/a/\\xff/'" + HEX),
    ("tr invalid set", "printf 'a\\377b\\n' | tr $'\\xff' X"),
    ("cut -d invalid", "printf 'a\\377b\\n' | cut -d $'\\xff' -f 2"),
    ("sort -t invalid", "printf 'b\\377a\\na\\377b\\n' | sort -t $'\\xff' -k 2" + HEX),
    ("IFS invalid", "IFS=$'\\xff'; x=$'a\\xffb'; set -- $x; echo $#"),
    ("case invalid pattern", "x=$'a\\xffb'; case $x in a$'\\xff'b) echo match;; *) echo no;; esac"),
    ("cond invalid pattern", "x=$'a\\xffb'; [[ $x == a?b ]] && echo one || echo other"),
    ("cond regex invalid", "x=$'a\\xffb'; [[ $x =~ ^a.b$ ]] && echo one || echo other"),
    ("cond regex invalid before the match", "x=$'\\xffab'; [[ $x =~ ab ]]; echo $? \"${BASH_REMATCH[0]}\""),
    ("cond regex invalid after the match", "x=$'ab\\xff'; [[ $x =~ a ]]; echo $?; x=$'abc\\xff'; [[ $x =~ a ]]; echo $? \"${BASH_REMATCH[0]}\""),
    ("cond regex invalid in the pattern", "x=$'a\\xffb'; p=$'^a\\xffb$'; [[ $x =~ $p ]]; echo $?; [[ $x =~ \"a\"$'\\xff'\"b\" ]]; echo $?"),
    ("cond regex invalid with a back-reference", "x=$'\\xffaa'; [[ $x =~ (a)\\1 ]]; echo $? \"${BASH_REMATCH[0]}\""),
    ("trim invalid", "x=$'a\\xffb'; printf %s \"${x#a?}\"" + HEX),
    ("printf arg invalid", "printf '%s\\n' $'\\xff\\xfe'" + HEX),
    ("printf %c invalid", "printf '%c' $'\\xff'" + HEX),
    ("printf char code invalid", "printf '%d\\n' \"'\"$'\\xff'"),
    ("echo arg invalid", "echo $'\\xc3'" + HEX),
    ("expr length invalid", "expr length $'a\\xffb'"),
    ("test -n invalid", "[ -n $'\\xff' ] && echo nonempty"),
    ("test = invalid", "[ $'\\xff' = $'\\xff' ] && echo equal"),
    ("export invalid", "export V=$'\\xff'; sh -c 'printf %s \"$V\"'" + HEX),
    ("env invalid", "V=$'\\xfe' env | grep -a '^V='" + HEX),
    ("sh -c invalid", "sh -c 'printf %s \"$1\"' sh $'\\xff'" + HEX),
    ("function arg invalid", "f() { printf %s \"$1\"; }; f $'\\xff'" + HEX),
    ("array element invalid", "a=(x $'\\xff'); printf %s \"${a[1]}\"" + HEX),
    ("assoc key invalid", "declare -A m; m[$'\\xff']=1; printf %s \"${!m[@]}\"" + HEX),
    ("heredoc invalid expansion", "x=$'\\xff'; cat <<EOF" + HEX + "\n$x\nEOF"),
    ("jq arg invalid", "jq -n --arg v $'\\xff' '$v'" + HEX),
    ("jq invalid json string", "printf '\"a\\377b\"\\n' | jq ." + HEX),
    ("seq separator invalid", "seq -s $'\\xff' 3" + HEX),
    ("nl separator invalid", "printf 'a\\n' | nl -s $'\\xff'" + HEX),
    ("paste delimiter invalid", "printf '1\\n2\\n' | paste -s -d $'\\xff'" + HEX),
    ("join -t invalid", "printf 'k\\377a\\n' > /tmp/a; join -t $'\\xff' /tmp/a /tmp/a" + HEX),
    ("xargs -d invalid", "printf 'a\\377b' | xargs -d $'\\xff' echo"),
    ("read -d invalid", "IFS= read -r -d $'\\xff' x < <(printf 'ab\\377cd'); printf %s \"$x\"" + HEX),
    ("mapfile -d invalid", "mapfile -t -d $'\\xff' a < <(printf 'a\\377b\\377'); echo \"${#a[@]}\""),
    ("printf %q control", "printf '%q\\n' $'a\\x01\\x7f\\tb'"),
    ("transform Q control", "x=$'a\\x01\\x7f\\tb'; echo \"${x@Q}\""),
    ("transform E", "x='a\\x41\\u00e9\\xff'; printf %s \"${x@E}\"" + HEX),
    ("xtrace control", "set -x; : $'a\\x01b\\xff'"),
    ("declare -p control", "x=$'\\x01\\n\\xff'; declare -p x"),
    ("dollar-quote nul", "x=$'a\\000b'; echo \"${#x}\""),
    ("dollar-quote octal high", "x=$'\\377\\376'; printf %s \"$x\"" + HEX),
    ("dollar-quote control-x", "x=$'\\cA\\c?'; printf %s \"$x\"" + HEX),
    ("command substitution nul warning", "x=$(printf 'a\\000b'); echo \"${#x}\""),
    ("command substitution trailing crlf", "x=$(printf 'a\\r\\n\\n'); printf %s \"$x\"" + HEX),
    ("read crlf", "IFS= read -r l < <(printf 'a\\r\\n'); echo \"${#l}\""),
    ("mapfile crlf", "mapfile -t a < <(printf 'a\\r\\nb\\r\\n'); printf %s \"${a[0]}\"" + HEX),
]
for op, script in ARG_OPS:
    CASES.append((f"edge bytes args: {op}", script))

NOT_UTF8_NAME = (
    "WASI names files with Unicode strings, so a file name holding bytes that are not UTF-8 "
    "cannot exist: creating one fails with EILSEQ (Illegal byte sequence) instead of creating "
    "a file with a different name (README, deliberate limits)"
)
EXPECTED = {
    "edge bytes names: ls -b invalid name": (
        0, b"", b"bash: line 1: n\xff: Illegal byte sequence\n", NOT_UTF8_NAME,
    ),
    "edge bytes names: ls quoted invalid name": (
        0, b"", b"bash: line 1: n\xff: Illegal byte sequence\n", NOT_UTF8_NAME,
    ),
    "edge bytes names: mkdir invalid name": (
        0, b"status=1\n",
        b"mkdir: cannot create directory \xe2\x80\x98/tmp/d\xef\xbf\xbd\xe2\x80\x99: Illegal byte sequence\n",
        NOT_UTF8_NAME,
    ),
    "edge bytes names: rm invalid name": (
        0, b"status=1\n0\n",
        b"bash: line 1: n\xff: Illegal byte sequence\nrm: cannot remove \"n\\xFF\": Illegal byte sequence\n",
        NOT_UTF8_NAME,
    ),
    "edge bytes names: touch invalid name": (
        0, b"status=1\n", b"touch: setting times of \"/tmp/n\\xFF\": Illegal byte sequence\n",
        NOT_UTF8_NAME,
    ),
    # Divergences that are decisions, not bugs.
    'edge bytes names: cd invalid name': (
        1, b'', b'mkdir: cannot create directory \xe2\x80\x98/tmp/d\xef\xbf\xbd\xe2\x80\x99: Illegal byte sequence\n',
        'fixture: WASI file names are Unicode strings, so a name that is not valid UTF-8 (the byte 0xff) cannot be created; mkdir fails with EILSEQ and its message shows the byte as U+FFFD',
    ),
    "edge bytes names: glob invalid name": (
        1, b"", b"bash: line 1: n\xff: Illegal byte sequence\n",
        "WASI names files with Unicode strings, so a file name holding bytes that are not UTF-8 "
        "cannot exist: creating one fails with EILSEQ (Illegal byte sequence) instead of creating a "
        "file with a different name (README: file names)",
    ),
    'edge bytes names: redirect invalid name': (
        0, b'status=1\n', b'bash: line 1: /tmp/n\xff: Illegal byte sequence\n', NOT_UTF8_NAME,
    ),
    'edge bytes names: find invalid name': (
        0, b'', b'bash: line 1: n\xff: Illegal byte sequence\n', NOT_UTF8_NAME,
    ),
    'edge bytes names: xargs -0 invalid name': (
        123, b'',
        b'bash: line 1: n\xff: Illegal byte sequence\nls: cannot access "n\\xFF": Illegal byte sequence\n',
        NOT_UTF8_NAME,
    ),
}
