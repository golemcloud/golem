"""Edge inputs: very large but bounded data generated inside the script (seq, yes, printf loops,
brace expansion), around the 64 KiB pipe buffer and well past it, with small checked output
(wc, tail, cksum, sha256sum).

Generated from fixed tables with plain loops, so every Python 3 produces the same scripts.
"""

TIER = "sweep"

CASES = []

# A large numeric stream through each filter; the output is reduced to a checksum or a count.
SOURCES = [
    ("seq 100k", "seq 100000"),
    ("long lines", "seq 2000 | paste -s -d ' ' | sed 'p;p;p;p'"),
]
FILTERS = [
    ("cat", "cat | cksum"),
    ("sort", "sort | cksum"),
    ("sort -n", "sort -n | tail -n 2"),
    ("sort -rn", "sort -rn | head -n 2"),
    ("sort -u", "sort -u | wc -l"),
    ("sort -t -k", "sort -t 0 -k 2 | cksum"),
    ("uniq -c", "cut -c1 | sort | uniq -c"),
    ("tac", "tac | head -n 2"),
    ("tail -n", "tail -n 3"),
    ("tail -c", "tail -c 20"),
    ("head -n", "head -n 3 | cut -c1-40"),
    ("wc", "wc"),
    ("md5sum", "md5sum"),
    ("sha256sum", "sha256sum"),
    ("sha512sum", "sha512sum | cut -c1-32"),
    ("b2sum", "b2sum | cut -c1-32"),
    ("grep -c", "grep -c 7"),
    ("grep -v", "grep -v 1 | wc -l"),
    ("grep -o", "grep -o 99 | wc -l"),
    ("grep -E", "grep -E -c '^(12|34)+'"),
    ("grep -F", "grep -F -c 555"),
    ("sed s///g", "sed 's/1/X/g' | cksum"),
    ("sed -n $p", "sed -n '$p' | cut -c1-40"),
    ("sed range", "sed -n '500,510p' | cksum"),
    ("sed hold", "sed -n 'H;${x;s/\\n/,/g;p}' | wc -c"),
    ("tr", "tr 0-9 a-j | cksum"),
    ("tr -d", "tr -d '\\n' | wc -c"),
    ("cut -c", "cut -c 2-4 | cksum"),
    ("cut -f", "cut -d ' ' -f 3 | cksum"),
    ("rev", "rev | cksum"),
    ("fold", "fold -w 7 | cksum"),
    ("nl", "nl | tail -n 1 | cut -c1-30"),
    ("paste -s", "paste -s -d + | wc -c"),
    ("od", "od -An -tx1 | tail -n 1"),
    ("base64", "base64 | cksum"),
    ("base64 round trip", "base64 | base64 -d | cksum"),
    ("base32 round trip", "base32 | base32 -d | cksum"),
    ("xargs", "xargs | wc -c"),
    ("xargs -n", "xargs -n 1000 echo | wc -l"),
    ("jq -R", "jq -R 'length' | sort -n | tail -n 1"),
    ("jq -s", "jq -R . | jq -s 'length'"),
    ("expand", "sed 's/ /\\t/' | expand | cksum"),
    ("fmt", "fmt -w 60 | cksum"),
    ("tee", "tee /tmp/copy | wc -c; cksum < /tmp/copy"),
    ("while read", "{ n=0; while read -r l; do n=$((n+1)); done; echo $n; }"),
    ("mapfile", "{ mapfile -t a; echo ${#a[@]}; }"),
    ("command substitution", "{ x=$(cat); echo ${#x}; }"),
    ("read -d empty", "{ IFS= read -r -d '' x; echo ${#x}; }"),
    ("split", "{ cd /tmp && split -l 999 && ls x* | wc -l && cat x* | cksum; }"),
    ("file", "file -"),
]
for source, producer in SOURCES:
    for label, consumer in FILTERS:
        CASES.append((f"edge large: {label} {source}", f"{producer} | {consumer}"))

# The 64 KiB pipe buffer: exactly at, one below and one past it, and far past it.
for size in (65535, 65536, 65537, 131072, 1048577):
    CASES += [
        (f"edge large pipe: yes head -c {size}", f"yes | head -c {size} | wc -c"),
        (f"edge large pipe: cat chain {size}", f"yes abcdefg | head -c {size} | cat | cat | cat | cksum"),
        (f"edge large pipe: tee chain {size}", f"yes | head -c {size} | tee /tmp/t | wc -c; wc -c < /tmp/t"),
        (f"edge large pipe: command substitution {size}", f"x=$(yes | head -c {size}); echo ${{#x}}".replace("{{", "{").replace("}}", "}")),
        (f"edge large pipe: file round trip {size}", f"yes | head -c {size} > /tmp/f; wc -c < /tmp/f; cat /tmp/f | cksum"),
    ]

# One very long line, with no newline anywhere, through line-oriented filters.
LONG = "yes x | head -n 300000 | tr -d '\\n'"
for label, consumer in [
    ("wc", "wc"), ("sed", "sed 's/x/y/g' | cksum"), ("grep -c", "grep -c x"),
    ("cut -c", "cut -c 299990-300005"), ("sort", "sort | wc -c"), ("tail -c", "tail -c 5"),
    ("rev", "rev | wc -c"), ("fold", "fold -w 1000 | wc -l"), ("read", "{ read -r l; echo $? ${#l}; }"),
    ("uniq", "uniq | wc -c"), ("tr", "tr x y | tail -c 3"), ("head -n", "head -n 1 | wc -c"),
    ("jq -R", "jq -R length"), ("od", "od -An -c | tail -n 2"), ("nl", "nl | cut -c1-12"),
]:
    CASES.append((f"edge large long line: {label}", f"{LONG} | {consumer}"))

# Shell-level volume: words, arrays, strings, loops and recursion.
SHELL = [
    ("brace expansion count", "echo {1..100000} | wc -w"),
    ("brace expansion nested", "echo {a..z}{a..z}{0..9} | wc -c"),
    ("printf many arguments", "printf '%s\\n' {1..100000} | tail -n 1"),
    ("set many positional", "set -- $(seq 50000); echo $# ${50000}"),
    ("shift many", "set -- $(seq 20000); shift 19999; echo $# $1"),
    ("array many", "a=($(seq 50000)); echo ${#a[@]} ${a[49999]}"),
    ("array append loop", "a=(); for ((i=0;i<20000;i++)); do a+=($i); done; echo ${#a[@]} ${a[-1]}"),
    ("array sparse", "a=(); a[1000000]=x; a[5]=y; echo ${#a[@]} ${!a[@]}"),
    ("assoc many", "declare -A m; for ((i=0;i<10000;i++)); do m[k$i]=$i; done; echo ${#m[@]} ${m[k9999]}"),
    ("string concat loop", "s=; for ((i=0;i<20000;i++)); do s+=x; done; echo ${#s}"),
    # Bash's replacement is quadratic: 200,000 characters took it over 15 seconds.
    ("string replace large", "s=$(printf '%*s' 50000 ''); s=${s// /ab}; echo ${#s}"),
    ("string substring large", "s=$(seq 100000 | tr -d '\\n'); echo ${s:488885:10}"),
    ("string trim large", "s=$(seq 3000 | tr '\\n' /); t=${s##*/1999}; echo ${#t}"),
    ("string length large", "s=$(yes abc | head -n 100000); echo ${#s}"),
    ("string upper large", "s=$(yes abc | head -n 20000 | tr -d '\\n'); u=${s^^}; echo ${u:0:6} ${#u}"),
    ("word split large", "s=$(seq 30000); set -- $s; echo $#"),
    ("IFS split large", "s=$(seq 30000 | tr '\\n' ,); IFS=,; set -- $s; echo $#"),
    ("arith loop", "n=0; for ((i=1;i<=100000;i++)); do ((n+=i)); done; echo $n"),
    ("while loop", "i=0; while ((i<50000)); do ((i++)); done; echo $i"),
    ("until loop", "i=0; until [ $i -ge 20000 ]; do i=$((i+1)); done; echo $i"),
    ("for in large list", "n=0; for x in $(seq 30000); do n=$((n+1)); done; echo $n"),
    ("case in loop", "n=0; for ((i=0;i<20000;i++)); do case $((i%3)) in 0) ((n++));; esac; done; echo $n"),
    ("function calls", "f() { :; }; for ((i=0;i<20000;i++)); do f; done; echo done"),
    ("recursion depth", "f() { if (( $1 > 0 )); then f $(( $1 - 1 )); else echo bottom; fi; }; f 1000"),
    ("recursion return", "f() { (( $1 == 0 )) && return 0; f $(( $1 - 1 )); return $(( $? + 0 )); }; f 800; echo $?"),
    ("recursion with locals", "f() { local d=$1; (( d == 0 )) && { echo 0; return; }; echo $(( $(f $((d-1))) + 1 )); }; f 60"),
    ("nested subshells", "x=$( ( ( ( ( ( ( ( ( ( echo deep ) ) ) ) ) ) ) ) ) ); echo $x"),
    ("nested command substitution", "echo $(echo $(echo $(echo $(echo $(echo $(echo $(echo nested)))))))"),
    ("long pipeline", "echo x" + " | cat" * 60),
    ("long pipeline status", "set -o pipefail; true" + " | true" * 40 + " | false; echo $? ${#PIPESTATUS[@]}"),
    ("many redirections", "for i in $(seq 200); do echo $i >> /tmp/f; done; wc -l < /tmp/f"),
    ("many heredoc lines", "cat <<EOF | wc -l\n$(seq 30000)\nEOF"),
    ("here string large", "x=$(seq 50000); wc -l <<< \"$x\""),
    ("eval large", "s=; for ((i=0;i<2000;i++)); do s+=\"x$i=$i;\"; done; eval \"$s\"; echo $x1999"),
    ("declare many variables", "for ((i=0;i<5000;i++)); do declare v$i=$i; done; echo $v4999; compgen -v v | wc -l"),
    ("many functions", "for ((i=0;i<2000;i++)); do eval \"f$i() { echo $i; }\"; done; f1999; declare -F | wc -l"),
    ("arith big numbers", "echo $(( 2**62 )) $(( 2**63 )) $(( 2**64 )) $(( -2**63 ))"),
    ("arith overflow multiply", "echo $(( 9223372036854775807 + 1 )) $(( 3037000500 * 3037000500 ))"),
    ("arith min divide", "echo $(( -9223372036854775808 / -1 ))"),
    ("arith min modulo", "echo $(( -9223372036854775808 % -1 ))"),
    ("arith large literal", "echo $(( 99999999999999999999 ))"),
    ("arith base 36", "echo $(( 36#zzzzzzzzzzzz ))"),
    ("printf large integer", "printf '%d\\n' 9223372036854775807; printf '%d\\n' 9223372036854775808; echo $?"),
    ("printf negative overflow", "printf '%d\\n' -9223372036854775809; echo $?"),
    ("printf unsigned", "printf '%u %x %o\\n' -1 -1 -1"),
    ("printf large width", "printf '%5000s' x | wc -c"),
    ("printf large precision", "printf '%.3000d' 7 | wc -c"),
    ("printf float large", "printf '%.2f\\n' 1e300 | wc -c"),
    ("test large integer", "[ 9223372036854775807 -gt 9223372036854775806 ] && echo gt"),
    ("test overflowing integer", "[ 99999999999999999999 -gt 1 ]; echo $?"),
    ("seq large range", "seq 999999990 1000000000 | tail -n 2"),
    ("seq large step count", "seq 0 7 700000 | wc -l"),
    ("seq float", "seq 0 0.1 1000 | tail -n 1"),
    ("seq big integers", "seq 18446744073709551614 18446744073709551616"),
    ("seq -w large", "seq -w 99998 100000"),
    ("factor large", "factor 9223372036854775807 18446744073709551615"),
    ("factor 128-bit", "factor 340282366920938463463374607431768211455"),
    ("expr large", "expr 9223372036854775807 + 0; expr 9223372036854775807 + 1; echo $?"),
    ("expr huge", "expr 99999999999999999999 + 1; echo $?"),
    ("sort -n huge numbers", "printf '%s\\n' 1e3 99999999999999999999 -99999999999999999999 1 | sort -n"),
    ("sort -g", "printf '%s\\n' 1e3 2e2 inf -inf nan 5 | sort -g"),
    ("sort -h", "printf '%s\\n' 1G 2K 3M 512 1T | sort -h"),
    ("numfmt large", "numfmt --to=iec 9223372036854775807 1073741824"),
    ("numfmt from", "numfmt --from=iec 8E 1K"),
    ("head -c large count", "printf abc | head -c 99999999999999999999 | wc -c"),
    ("tail -n large count", "seq 3 | tail -n 99999999999999999999"),
    ("cut large position", "echo abc | cut -c 99999999999999999999-"),
    ("fold large width", "echo abc | fold -w 99999999999999999999"),
    ("many files", "cd /tmp && for i in $(seq 1000); do : > f$i; done; ls | wc -l; ls | sort -V | tail -n 1"),
    ("many files glob", "cd /tmp && for i in $(seq 1000); do : > f$i; done; set -- *; echo $#"),
    ("many files find", "cd /tmp && for i in $(seq 1000); do : > f$i; done; find . -type f | wc -l"),
    ("many files rm", "cd /tmp && for i in $(seq 1000); do : > f$i; done; rm f*; ls | wc -l"),
    ("many files xargs", "cd /tmp && seq 500 | xargs touch; ls | wc -l"),
    ("many files grep -r", "mkdir /tmp/g && cd /tmp/g && for i in $(seq 300); do echo $i > f$i; done; grep -rl 7 . | wc -l"),
    ("deep directories", "cd /tmp && mkdir -p $(printf 'd/%.0s' $(seq 100)) && find . -type d | wc -l"),
    ("deep cd", "cd /tmp && mkdir -p $(printf 'd/%.0s' $(seq 60)) && cd $(printf 'd/%.0s' $(seq 60)) && pwd | wc -c"),
    ("deep rm -r", "cd /tmp && mkdir -p $(printf 'd/%.0s' $(seq 80)) && rm -r d && ls | wc -l"),
    ("long file name", "cd /tmp && n=$(printf 'x%.0s' $(seq 255)); : > $n && ls | wc -c"),
    ("large file wc", "seq 200000 > /tmp/f; wc /tmp/f; cksum /tmp/f"),
    ("large file cp", "seq 200000 > /tmp/f; cp /tmp/f /tmp/g; cmp /tmp/f /tmp/g && echo same"),
    ("large file diff", "seq 50000 > /tmp/a; seq 50000 | sed '25000s/.*/X/' > /tmp/b; diff /tmp/a /tmp/b"),
    ("large file diff -q", "seq 50000 > /tmp/a; seq 50001 > /tmp/b; diff -q /tmp/a /tmp/b; echo $?"),
    ("large file cmp", "seq 50000 > /tmp/a; seq 50000 | sed '40000s/0/o/' > /tmp/b; cmp /tmp/a /tmp/b"),
    ("large file comm", "seq 30000 | sort > /tmp/a; seq 2 2 40000 | sort > /tmp/b; comm -12 /tmp/a /tmp/b | wc -l"),
    ("large file join", "seq 20000 | sed 's/$/ a/' | sort > /tmp/a; seq 20000 | sed 's/$/ b/' | sort > /tmp/b; join /tmp/a /tmp/b | wc -l"),
    ("large file paste", "seq 50000 > /tmp/a; paste /tmp/a /tmp/a | tail -n 1"),
    ("large file sed -i", "seq 100000 > /tmp/f; sed -i 's/0/o/g' /tmp/f; cksum /tmp/f"),
    ("large file truncate", "seq 100000 > /tmp/f; truncate -s 1000 /tmp/f; wc -c /tmp/f"),
    ("large file split -b", "cd /tmp && seq 100000 > f && split -b 65536 f p && ls p* | wc -l && cat p* | cmp - f && echo same"),
    ("large file csplit", "cd /tmp && seq 1000 > f && csplit -s f 250 500 750 && wc -l xx*"),
    ("large file tail -c +", "seq 100000 > /tmp/f; tail -c +588890 /tmp/f"),
    ("large file head -c -", "seq 100000 > /tmp/f; head -c -588880 /tmp/f | wc -c"),
    ("large file od -N", "seq 100000 > /tmp/f; od -An -c -j 588880 -N 8 /tmp/f"),
    ("large file grep -m", "seq 100000 > /tmp/f; grep -m 3 -n 9999 /tmp/f"),
    ("large file sort -o in place", "seq 100000 > /tmp/f; sort -r -o /tmp/f /tmp/f; head -n 1 /tmp/f"),
    ("large file uniq", "seq 100000 | sed 's/.$//' > /tmp/f; uniq /tmp/f | wc -l"),
    ("large file jq", "seq 20000 | jq -s 'add'"),
    ("large json object", "seq 5000 | jq -R '{(.): 1}' | jq -s 'add | length'"),
    ("large json nested", "printf '%.0s[' $(seq 500) > /tmp/j; printf '%.0s]' $(seq 500) >> /tmp/j; jq -c 'flatten' /tmp/j"),
    ("large json string", "yes a | head -n 100000 | tr -d '\\n' | jq -R 'length'"),
    ("large xargs input", "seq 100000 | xargs -n 5000 echo | wc -l"),
    ("large xargs -I", "seq 2000 | xargs -I{} echo x{} | tail -n 1"),
    ("large find -exec", "mkdir /tmp/e && cd /tmp/e && seq 200 | xargs touch && find . -type f -exec echo {} + | wc -w"),
    ("large tsort", "seq 2000 | sed 'N;s/\\n/ /' | tsort | wc -l"),
    ("large shuf -n0", "seq 100000 | shuf -n 0 | wc -l"),
    ("large tr classes", "seq 100000 | tr '[:digit:]' '[:alpha:]' | sort -u | wc -l"),
    ("large background output", "{ seq 100000 > /tmp/bg; } & wait; wc -l < /tmp/bg"),
    ("large process substitution", "wc -l < <(seq 100000); cat <(seq 50000) <(seq 50000) | wc -l"),
    ("large process substitution output", "seq 100000 > >(wc -l)"),
    ("large yes lines", "yes 'a long line of text' | head -n 200000 | uniq -c"),
    ("large printf repeat", "printf 'ab%.0s' $(seq 50000) | wc -c"),
]
for label, script in SHELL:
    CASES.append((f"edge large: {label}", script))

NESTING = (
    "bash-tool's limit (README, Limits: Nesting): function calls and `$( )` share Wasmtime's 512 KiB "
    "native stack, so recursion ends with the nesting error (about 75 levels through an `if`, 150 "
    "bare, about 50 through `$( )`); bash has no such limit"
)
EXPECTED = {
    "edge large: recursion depth": (
        1, b"", b"bash: line 1: f: maximum function nesting level exceeded (75): deeper nesting is "
        b"unsupported in bash-tool\n", NESTING,
    ),
    "edge large: recursion return": (
        1, b"", b"bash: line 1: f: maximum function nesting level exceeded (150): deeper nesting is "
        b"unsupported in bash-tool\n", NESTING,
    ),
    "edge large: recursion with locals": (
        0, b"49\n", b"bash: line 1: f: maximum function nesting level exceeded (49): deeper nesting "
        b"is unsupported in bash-tool\n", NESTING,
    ),
}
