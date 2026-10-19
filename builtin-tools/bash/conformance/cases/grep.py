"""grep conformance: GNU grep 3.12 in the oracle is the reference for status, stdout and stderr."""

LOG = "printf 'INFO start\\nWARN disk 91%%\\nERROR db timeout\\nINFO retry\\nerror: lowercase\\nINFO done\\n' > /tmp/app.log"
CTX = "printf 'l1\\nfoo 1\\nl3\\nl4\\nfoo 2\\nl6\\nl7\\nl8\\nfoo 3\\nl10\\n' > /tmp/ctx"
TREE = (
    "mkdir -p /tmp/t/src/sub /tmp/t/node_modules/x && cd /tmp/t && "
    "printf 'fn main() {}\\n// TODO: fix\\n' > src/main.rs && "
    "printf 'todo later\\nTODO now\\n' > src/sub/notes.md && "
    "printf 'TODO vendored\\n' > node_modules/x/index.js && "
    "printf 'plain\\n' > README"
)

CASES = [
    ("grep: basic match", f"{LOG}; grep ERROR /tmp/app.log"),
    ("grep: ignore case", f"{LOG}; grep -i error /tmp/app.log"),
    ("grep: invert", f"{LOG}; grep -v INFO /tmp/app.log"),
    ("grep: count", f"{LOG}; grep -c INFO /tmp/app.log"),
    ("grep: line numbers", f"{LOG}; grep -n 'WARN\\|ERROR' /tmp/app.log"),
    ("grep: extended alternation", f"{LOG}; grep -E 'WARN|ERROR' /tmp/app.log"),
    ("grep: extended groups and counts", "printf 'ab abab ababab\\n' | grep -oE '(ab){2,}'"),
    ("grep: basic intervals and groups", "printf 'ab abab ababab\\n' | grep -o '\\(ab\\)\\{2\\}'"),
    ("grep: basic literal metacharacters", "printf 'a+b\\naab\\na?b\\n(x)\\n' | grep -e 'a+b' -e 'a?b' -e '(x)'"),
    ("grep: back references", "printf 'abab\\nabba\\nxyxy\\n' | grep '\\(..\\)\\1' && printf 'abab\\nabba\\n' | grep -E '(.)(.)\\2\\1'"),
    ("grep: only matching", "printf 'id=42 id=7\\nnone\\n' | grep -o '[0-9]\\+'"),
    ("grep: only matching extended", "printf 'v1.2.3 and v10.0.1\\n' | grep -oE 'v[0-9]+(\\.[0-9]+)*'"),
    ("grep: word match", "printf 'foo-bar foobar\\nbarfoo\\nfoo\\n' | grep -w foo"),
    ("grep: word match only", "printf 'foo-bar foobar foo\\n' | grep -ow foo"),
    ("grep: line match", "printf 'abc\\nabcd\\n' | grep -x abc"),
    ("grep: word boundaries", "printf 'cat catalog concat\\n' | grep -o '\\<cat\\>'; printf 'cat catalog\\n' | grep -o '\\bcat\\b'"),
    ("grep: anchors", "printf 'start mid\\nmid start\\nend\\n' | grep -e '^start' -e 'end$'"),
    ("grep: character classes", "printf 'a1\\nb22\\n   \\nC\\n' | grep -E '^[[:alpha:]][[:digit:]]{2}$|^[[:space:]]+$|^[[:upper:]]$'"),
    ("grep: bracket edge cases", "printf 'a]b\\na-b\\na\\\\b\\n' | grep '[]-]'; printf 'a\\\\b\\n' | grep '[\\\\]'"),
    ("grep: fixed strings", "printf 'a.b\\nab\\n[x]\\n' | grep -F -e . -e '[x]'"),
    ("grep: perl lookaround", "printf 'price: $42 and $7\\n' | grep -oP '(?<=\\$)\\d+'"),
    ("grep: perl classes", "printf 'user_1 x\\n2abc\\n' | grep -P '^\\w+\\d\\s'"),
    ("grep: multiple patterns", f"{LOG}; grep -e WARN -e retry /tmp/app.log"),
    ("grep: newline patterns", f"{LOG}; grep $'WARN\\nretry' /tmp/app.log"),
    ("grep: pattern file", f"{LOG}; printf 'WARN\\ndone\\n' > /tmp/p; grep -f /tmp/p /tmp/app.log"),
    ("grep: empty pattern file", f"{LOG}; grep -f /dev/null /tmp/app.log; echo status=$?"),
    ("grep: empty pattern", "printf 'a\\n\\nb\\n' | grep -c ''"),
    ("grep: blank lines", "printf 'a\\n\\n  \\nb\\n' | grep -cE '^\\s*$'"),
    ("grep: after context", f"{CTX}; grep -A1 -n foo /tmp/ctx"),
    ("grep: before context", f"{CTX}; grep -B2 'foo 2' /tmp/ctx"),
    ("grep: context", f"{CTX}; grep -C1 foo /tmp/ctx"),
    ("grep: numeric context", f"{CTX}; grep -1 'foo 3' /tmp/ctx"),
    ("grep: group separator", f"{CTX}; grep -A1 --group-separator=== foo /tmp/ctx; grep --no-group-separator -B1 foo /tmp/ctx"),
    ("grep: max count", f"{CTX}; grep -m2 foo /tmp/ctx; grep -m1 -A2 foo /tmp/ctx"),
    ("grep: max count count", f"{CTX}; grep -c -m2 foo /tmp/ctx"),
    ("grep: byte offset", "printf 'foo\\nbar foo\\n' | grep -b foo; printf 'foo\\nbar foo\\n' | grep -ob foo"),
    ("grep: files with matches", f"{LOG}; {CTX}; grep -l INFO /tmp/app.log /tmp/ctx; grep -L INFO /tmp/app.log /tmp/ctx"),
    ("grep: count per file", f"{LOG}; {CTX}; grep -c foo /tmp/app.log /tmp/ctx"),
    ("grep: with and without filename", f"{LOG}; grep -H WARN /tmp/app.log; grep -h WARN /tmp/app.log /tmp/app.log"),
    ("grep: null after names", f"{LOG}; {CTX}; grep -lZ foo /tmp/app.log /tmp/ctx | cat -v; echo"),
    ("grep: label", "echo hello | grep --label=input -H hello"),
    ("grep: quiet status", f"{LOG}; grep -q ERROR /tmp/app.log; echo found=$?; grep -q NOPE /tmp/app.log; echo missing=$?"),
    ("grep: no match status", "printf 'a\\n' | grep b; echo status=$?"),
    ("grep: recursive", f"{TREE}; grep -rn TODO . | sort"),
    ("grep: recursive implicit dot", f"{TREE}; grep -r TODO | sort"),
    ("grep: recursive files with matches", f"{TREE}; grep -rli todo src | sort"),
    ("grep: recursive include", f"{TREE}; grep -r --include='*.rs' TODO . | sort"),
    ("grep: recursive exclude", f"{TREE}; grep -r --exclude='*.md' --exclude-dir=node_modules TODO . | sort"),
    ("grep: directory without recursion", f"{TREE}; grep TODO src; echo status=$?"),
    ("grep: missing file", f"{LOG}; grep ERROR /tmp/nope /tmp/app.log; echo status=$?"),
    ("grep: missing file quiet", f"{LOG}; grep -s ERROR /tmp/nope /tmp/app.log; echo status=$?; grep -q ERROR /tmp/nope /tmp/app.log; echo status=$?"),
    ("grep: binary file", "printf 'bin\\0hello\\n' > /tmp/b; grep hello /tmp/b; echo status=$?; grep -c hello /tmp/b; grep -a hello /tmp/b | cat -v"),
    ("grep: binary without match", "printf 'bin\\0hello\\n' > /tmp/b; grep -I hello /tmp/b; echo status=$?"),
    ("grep: null data", "printf 'a\\0b1\\0b2\\0' | grep -z b | tr '\\0' '\\n'"),
    ("grep: unicode ignore case", "printf 'ÉCOLE école\\n' | grep -io 'école'"),
    ("grep: unicode dot", "printf 'héllo\\n' | grep -o 'h.l'"),
    ("grep: invalid option", "grep -k x /dev/null; echo status=$?"),
    ("grep: unrecognized long option", "grep --bogus x /dev/null; echo status=$?"),
    ("grep: missing pattern", "grep; echo status=$?"),
    ("grep: options after operands", f"{LOG}; grep ERROR /tmp/app.log -n"),
    ("grep: double dash", "printf -- '-v\\nx\\n' | grep -- -v"),
    ("grep: color never", f"{LOG}; grep --color=never WARN /tmp/app.log"),
    ("grep: pipeline status", "set -o pipefail; printf 'a\\n' | grep b | cat; echo \"status=$? ${PIPESTATUS[*]}\""),
    ("grep: endless max count", "while :; do echo x; done | grep -m1 x"),
    ("grep: endless quiet", "while :; do echo match; done | grep -q match; echo status=$?"),
    ("grep: endless count limit", "while :; do echo x; done | grep -c -m3 x"),
]

EXPECTED = {}

EXPECTED_STDERR = {}
