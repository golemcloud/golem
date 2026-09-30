"""Scripts in the shapes coding agents actually run: file edits, searches, JSON, loops, cleanup.

Each case is a small task as an agent would write it, so failures show up the way an agent would
meet them. Commands this shell does not have (awk, tar, ...) appear too: an agent will reach for
them, and the difference is worth seeing.
"""

SETUP = (
    "mkdir -p /tmp/proj/src /tmp/proj/docs && cd /tmp/proj && "
    "printf 'fn main() {\\n    println!(\"hello\");\\n}\\n' >src/main.rs && "
    "printf 'pub fn add(a: i32, b: i32) -> i32 { a + b }\\n// TODO: sub\\n' >src/lib.rs && "
    "printf '# Title\\n\\nSome TODO here.\\n' >docs/README.md && "
    "printf 'name,age\\nann,31\\nbob,27\\ncid,45\\n' >people.csv && "
    "printf '{\"items\":[{\"name\":\"a\",\"n\":1},{\"name\":\"b\",\"n\":5},{\"name\":\"c\",\"n\":3}]}\\n' >data.json; "
)

CASES = [
    # Writing and editing files
    ("agent: write a file with a heredoc", "cat > /tmp/config.toml <<'EOF'\n[server]\nport = 8080\nhost = \"${HOST}\"\nEOF\ncat /tmp/config.toml"),
    ("agent: write a file with an expanding heredoc", "name=demo; cat > /tmp/out.txt <<EOF\nproject: $name\nfiles: $(echo 3)\nEOF\ncat /tmp/out.txt"),
    ("agent: append lines to a file", "printf 'a\\n' >/tmp/f; echo b >> /tmp/f; printf '%s\\n' c d >>/tmp/f; cat /tmp/f; wc -l </tmp/f"),
    ("agent: in-place sed edit", SETUP + "sed -i 's/hello/world/' src/main.rs && cat src/main.rs"),
    ("agent: in-place sed with backup", SETUP + "sed -i.bak 's/TODO/DONE/' src/lib.rs && cat src/lib.rs && cat src/lib.rs.bak | tail -n 1"),
    ("agent: sed delete and insert lines", SETUP + "sed -i '2d' src/main.rs; sed -i '1i // header' src/main.rs; cat src/main.rs"),
    ("agent: sed print a line range", "seq 1 20 >/tmp/n; sed -n '5,8p' /tmp/n; sed -n '$p' /tmp/n"),
    ("agent: replace across files", SETUP + "grep -rl TODO . | sort | xargs sed -i 's/TODO/FIXME/'; grep -rn FIXME . | sort"),
    ("agent: rename files in a loop", "cd /tmp && touch a.txt b.txt && for f in *.txt; do mv \"$f\" \"${f%.txt}.md\"; done; ls"),
    ("agent: create a directory tree", "mkdir -p /tmp/app/{src,tests,docs} && touch /tmp/app/src/{a,b}.rs && find /tmp/app | sort"),
    ("agent: copy and remove trees", "mkdir -p /tmp/s/d && echo x >/tmp/s/d/f && cp -r /tmp/s /tmp/t && ls -R /tmp/t && rm -rf /tmp/s && ls /tmp"),
    ("agent: patch a file with diff output", "printf 'a\\nb\\nc\\n' >/tmp/old; printf 'a\\nB\\nc\\n' >/tmp/new; diff -u /tmp/old /tmp/new >/tmp/p.diff; cp /tmp/old /tmp/work; patch /tmp/work /tmp/p.diff && cat /tmp/work"),
    ("agent: patch from stdin with strip level", "mkdir -p /tmp/r/src && printf 'x\\n' >/tmp/r/src/f && cd /tmp/r && printf -- '--- a/src/f\\n+++ b/src/f\\n@@ -1 +1 @@\\n-x\\n+y\\n' | patch -p1 && cat src/f"),
    # Searching
    ("agent: grep recursively with line numbers", SETUP + "grep -rn 'TODO' . | sort"),
    ("agent: grep for files only", SETUP + "grep -rl 'fn ' src | sort; grep -rL 'TODO' src | sort"),
    ("agent: grep with include and exclude", SETUP + "grep -rn --include='*.rs' 'fn' . | sort; grep -rn --exclude-dir=src 'TODO' ."),
    ("agent: grep count and quiet", SETUP + "grep -c fn src/lib.rs; if grep -q hello src/main.rs; then echo found; fi; grep -q nothing src/main.rs || echo absent"),
    ("agent: grep extended regex with capture via sed", SETUP + "grep -Eo 'fn [a-z]+' src/*.rs | sed 's/.*fn //' | sort"),
    ("agent: find files by name", SETUP + "find . -name '*.rs' | sort; find . -type d | sort"),
    ("agent: find and exec grep", SETUP + "find . -type f -name '*.rs' -exec grep -l 'pub' {} + | sort"),
    ("agent: find newer and size", SETUP + "touch -d '2020-01-01' docs/README.md; find . -type f -newer docs/README.md | sort; find . -type f -size +0 | wc -l"),
    ("agent: find and delete", "mkdir -p /tmp/c && touch /tmp/c/a.log /tmp/c/b.log /tmp/c/keep.txt && find /tmp/c -name '*.log' -delete && ls /tmp/c"),
    ("agent: find with print0 and xargs -0", "mkdir -p '/tmp/sp' && touch '/tmp/sp/a b' /tmp/sp/c && find /tmp/sp -type f -print0 | sort -z | xargs -0 -n1 echo"),
    ("agent: count lines of code", SETUP + "find . -name '*.rs' -print0 | xargs -0 cat | wc -l; wc -l src/*.rs"),
    ("agent: word frequency", "printf 'b a c a b a\\n' | tr ' ' '\\n' | sort | uniq -c | sort -rn | head -n 2"),
    # JSON
    ("agent: jq select and project", SETUP + "jq -r '.items[] | select(.n > 2) | .name' data.json"),
    ("agent: jq with arguments", SETUP + "jq --arg k b --argjson min 2 '.items[] | select(.name == $k and .n >= $min)' data.json"),
    ("agent: jq build JSON", "jq -n --arg name demo --argjson n 3 '{name: $name, count: $n, tags: [\"x\", \"y\"]}'"),
    ("agent: jq modify and write back", SETUP + "jq '.items[0].n = 10' data.json >/tmp/d.json && mv /tmp/d.json data.json && jq -c '.items[0]' data.json"),
    ("agent: jq length keys and sort", SETUP + "jq '.items | length' data.json; jq -c '.items | sort_by(.n) | map(.name)' data.json; jq -r '.items[0] | keys[]' data.json"),
    ("agent: jq slurp lines", "printf '{\"a\":1}\\n{\"a\":2}\\n' | jq -s 'map(.a) | add'"),
    ("agent: jq raw input to array", "printf 'x\\ny\\n' | jq -R . | jq -sc ."),
    ("agent: jq exit status", "echo '{}' | jq -e '.missing' >/dev/null; echo status=$?; echo '{\"a\":1}' | jq -e .a; echo status=$?"),
    ("agent: jq to CSV", SETUP + "jq -r '.items[] | [.name, .n] | @csv' data.json"),
    # CSV and text
    ("agent: cut columns from CSV", SETUP + "cut -d, -f1 people.csv | tail -n +2; cut -d, -f2 people.csv | tail -n +2 | sort -n | head -n 1"),
    ("agent: sort CSV by a numeric column", SETUP + "tail -n +2 people.csv | sort -t, -k2,2n"),
    ("agent: read CSV in a loop", SETUP + "tail -n +2 people.csv | while IFS=, read -r name age; do printf '%-5s %3d\\n' \"$name\" \"$age\"; done"),
    ("agent: sum a column", SETUP + "total=0; while IFS=, read -r _ age; do total=$((total + age)); done < <(tail -n +2 people.csv) 2>/dev/null; echo \"$total\""),
    ("agent: sum a column without process substitution", SETUP + "total=0; tail -n +2 people.csv | { while IFS=, read -r _ age; do total=$((total + age)); done; echo $total; }"),
    ("agent: awk one-liner", SETUP + "awk -F, 'NR>1 {s+=$2} END {print s}' people.csv"),
    ("agent: normalise line endings", "printf 'a\\r\\nb\\r\\n' >/tmp/w; tr -d '\\r' </tmp/w | od -c | head -n 1; sed 's/\\r$//' /tmp/w | wc -c"),
    ("agent: uppercase and trim", "echo '  Hello World  ' | tr '[:lower:]' '[:upper:]' | sed 's/^ *//;s/ *$//'"),
    ("agent: join lines with a comma", "printf 'a\\nb\\nc\\n' | paste -sd, -; printf 'a\\nb\\n' | tr '\\n' ',' | sed 's/,$//'; echo"),
    ("agent: dedupe preserving order", "printf 'b\\na\\nb\\nc\\na\\n' | awk '!seen[$0]++' 2>/dev/null || printf 'b\\na\\nb\\nc\\na\\n' | cat -n | sort -uk2 | sort -n | cut -f2-"),
    ("agent: head and tail of output", "seq 1 100 | head -n 3; seq 1 100 | tail -n 2; seq 1 100 | head -n -97"),
    ("agent: number lines", "printf 'x\\n\\ny\\n' | nl -ba; printf 'x\\ny\\n' | cat -n"),
    ("agent: show whitespace", "printf 'a\\tb \\n' | cat -A"),
    # Strings and variables
    ("agent: strip path components", "p=/a/b/file.tar.gz; echo \"${p##*/}\" \"${p%/*}\" \"${p%%.*}\" \"$(basename \"$p\" .gz)\" \"$(dirname \"$p\")\""),
    ("agent: default and required variables", "name=${NAME:-anon}; echo $name; : \"${REQUIRED:?REQUIRED must be set}\" 2>/dev/null; echo status=$?"),
    ("agent: split a string into an array", "IFS=, read -ra parts <<< 'a,b,c d'; echo ${#parts[@]}; printf '[%s]' \"${parts[@]}\"; echo"),
    ("agent: iterate an array with indices", "arr=(x y z); for i in \"${!arr[@]}\"; do echo \"$i=${arr[$i]}\"; done"),
    ("agent: associative counter", "declare -A count; for w in a b a c a; do count[$w]=$(( ${count[$w]:-0} + 1 )); done; for k in \"${!count[@]}\"; do echo \"$k ${count[$k]}\"; done | sort"),
    ("agent: string contains and matches", "s=feature/login; [[ $s == feature/* ]] && echo branch; [[ $s =~ ^([a-z]+)/(.+)$ ]] && echo \"${BASH_REMATCH[2]}\"; case $s in *login*) echo login;; esac"),
    ("agent: lowercase and replace", "s='Hello World'; echo \"${s,,}\" \"${s// /_}\" \"${s^^}\""),
    ("agent: printf a table", "printf '%-6s|%5s\\n' name size; printf '%-6s|%5d\\n' a 10 bb 200"),
    ("agent: printf -v to build a string", "printf -v line '%s-%s' a b; echo \"$line\""),
    ("agent: arithmetic counters", "i=0; for f in a b c; do ((i++)) || true; done; echo $i; n=$((i * 2 + 1)); echo $n"),
    ("agent: counter under set -e", "set -e; count=0; ((count++)); echo not-reached"),
    ("agent: let and compound arithmetic", "a=5; b=3; echo $(( a > b ? a : b )) $(( (a + b) / 2 )) $(( a % b ))"),
    # Control flow and robustness
    ("agent: strict mode script", "set -euo pipefail\nfiles=(a b)\nfor f in \"${files[@]}\"; do echo \"processing $f\"; done\necho done"),
    ("agent: strict mode catches an unset variable", "set -euo pipefail; echo start; echo \"$undefined_var\"; echo not-reached"),
    ("agent: strict mode catches a failing pipeline", "set -euo pipefail; echo start; false | cat; echo not-reached"),
    ("agent: guard with an error message", "[ -f /tmp/missing ] || { echo 'missing input' >&2; exit 3; }; echo not-reached"),
    ("agent: check a command exists", "if command -v jq >/dev/null 2>&1; then echo have-jq; fi; command -v nosuchtool >/dev/null || echo no-tool"),
    ("agent: cleanup trap", "tmp=$(mktemp -d); trap 'rm -rf \"$tmp\"; echo cleaned' EXIT; touch \"$tmp/f\"; echo working"),
    ("agent: temporary file round trip", "t=$(mktemp); echo payload >\"$t\"; cat \"$t\"; rm -f \"$t\"; [ -e \"$t\" ] || echo removed"),
    ("agent: retry loop", "n=0; until [ $n -ge 3 ]; do n=$((n+1)); [ $n -eq 3 ] && { echo \"ok after $n\"; break; }; sleep 0.01; done"),
    ("agent: argument parsing loop", "set -- --name demo -v --count=3 rest; while [ $# -gt 0 ]; do case $1 in --name) name=$2; shift 2;; --count=*) count=${1#*=}; shift;; -v) verbose=1; shift;; *) args+=(\"$1\"); shift;; esac; done; echo \"$name $count $verbose ${args[*]}\""),
    ("agent: functions with local and return", "check() { local file=$1; [ -s \"$file\" ] && return 0; echo \"empty: $file\" >&2; return 1; }; echo x >/tmp/a; : >/tmp/b; check /tmp/a && echo a-ok; check /tmp/b; echo status=$?"),
    ("agent: function returning a value by output", "slug() { local s=${1,,}; echo \"${s// /-}\"; }; v=$(slug 'Hello Big World'); echo \"$v\""),
    ("agent: exit codes of pipelines", "false | true; echo \"${PIPESTATUS[@]}\"; set -o pipefail; false | true; echo $?"),
    ("agent: conditional chains", "mkdir -p /tmp/x && cd /tmp/x && touch ok && ls && cd / && echo back || echo failed"),
    ("agent: while read from a file", "printf 'one\\ntwo\\nthree\\n' >/tmp/l; while IFS= read -r line; do echo \"<$line>\"; done </tmp/l"),
    ("agent: while read without a trailing newline", "printf 'one\\ntwo' | while IFS= read -r line || [ -n \"$line\" ]; do echo \"<$line>\"; done"),
    ("agent: readarray lines", "printf 'a\\nb\\nc\\n' >/tmp/l; mapfile -t lines </tmp/l; echo ${#lines[@]} \"${lines[-1]}\""),
    ("agent: parallel jobs and wait", "for i in 1 2 3; do (sleep 0.0$i; echo job$i >/tmp/j$i) & done; wait; cat /tmp/j1 /tmp/j2 /tmp/j3"),
    ("agent: background job status", "(exit 3) & pid=$!; wait $pid; echo status=$?"),
    # Encoding, hashing and numbers
    ("agent: base64 round trip", "echo 'secret data' | base64 | tee /tmp/b64; base64 -d /tmp/b64"),
    ("agent: checksum and verify", "echo data >/tmp/f; sha256sum /tmp/f >/tmp/sums; sha256sum -c /tmp/sums; echo tampered >/tmp/f; sha256sum -c /tmp/sums; echo status=$?"),
    ("agent: hex dump", "printf 'AB\\n' | od -An -tx1"),
    ("agent: human readable sizes", "numfmt --to=iec 1048576 1536; numfmt --from=iec 2K"),
    ("agent: sequence formatting", "seq -w 8 10; seq -s, 1 5; seq 1 2 7"),
    ("agent: fixed dates", "TZ=UTC date -d @0 +%Y-%m-%dT%H:%M:%S; TZ=UTC date -d '2024-02-29 12:00' +%s; TZ=UTC date -u -d @86400 +%A"),
    ("agent: file size and existence checks", "printf 'abc' >/tmp/f; wc -c </tmp/f; stat -c %s /tmp/f; [ -s /tmp/f ] && echo nonempty; [ -d /tmp ] && echo dir"),
    ("agent: realpath and readlink", "mkdir -p /tmp/r/d && ln -sf /tmp/r/d /tmp/r/link && cd /tmp/r && realpath link; readlink link; readlink -f ./link/../d"),
    ("agent: split and reassemble", "seq 1 10 >/tmp/n; split -l 4 /tmp/n /tmp/part.; ls /tmp/part.*; cat /tmp/part.* | wc -l"),
    ("agent: compare files", "printf 'a\\nb\\n' >/tmp/1; printf 'a\\nc\\n' >/tmp/2; cmp -s /tmp/1 /tmp/2 || echo differ; diff /tmp/1 /tmp/2; echo status=$?; comm -12 /tmp/1 /tmp/2"),
    ("agent: xargs with placeholders", "printf 'a\\nb\\n' | xargs -I{} echo 'item {} done'; printf '1 2 3 4\\n' | xargs -n 2 echo pair"),
    ("agent: tee to a file and stdout", "echo logged | tee /tmp/log | tr a-z A-Z; cat /tmp/log"),
    ("agent: here string into a command", "grep -o '[0-9]\\+' <<< 'v1.22.3' | paste -sd. -"),
    ("agent: command substitution in a condition", "if [ \"$(printf 'a\\nb\\n' | wc -l)\" -eq 2 ]; then echo two-lines; fi"),
    ("agent: nested command substitution", "echo \"dir: $(basename \"$(dirname /a/b/c)\")\""),
    ("agent: environment for a single command", "FOO=bar env | grep '^FOO='; echo \"[${FOO-}]\""),
    ("agent: export for child shells", "export API_URL=http://localhost; bash -c 'echo $API_URL'"),
    ("agent: source an env file", "printf 'export PORT=8080\\nNAME=\"my app\"\\n' >/tmp/.env; set -a; . /tmp/.env; set +a; bash -c 'echo \"$PORT $NAME\"'"),
    ("agent: bash -c with arguments", "bash -c 'echo \"$0 got $# args: $*\"' runner a b"),
    ("agent: sh -c in find exec", "mkdir -p /tmp/e && touch /tmp/e/x.txt /tmp/e/y.txt && find /tmp/e -name '*.txt' -exec sh -c 'echo \"${1##*/}\"' _ {} \\; | sort"),
    # Commands an agent may reach for; `tar`/`gzip` are still missing (EXPECTED below).
    # `timeout` is now embedded (tool-layer) and `uname`/`nproc`/`du` are too (this agent);
    # all behave normally here, no override needed -- `uname`/`nproc`'s sandbox-specific field
    # values are covered directly in coreutils_edge.py instead.
    ("agent: timeout", "timeout 5 echo fast; echo status=$?"),
    ("agent: uname", "uname -s >/dev/null; echo status=$?"),
    ("agent: nproc", "nproc >/dev/null; echo status=$?"),
    ("agent: du and df", "mkdir -p /tmp/du && du -s /tmp/du >/dev/null; echo status=$?"),
    ("agent: tar", "mkdir -p /tmp/t && echo x >/tmp/t/f && tar -cf /tmp/t.tar -C /tmp t 2>/dev/null; echo status=$?"),
    ("agent: gzip", "echo x | gzip | gzip -d; echo status=$?"),
    ("agent: tr squeeze and delete", "echo 'aaa   bbb' | tr -s ' '; echo 'a1b2c3' | tr -d '0-9'"),
    ("agent: expr and factor", "expr 7 + 5; expr length hello; factor 84"),
    ("agent: date arithmetic", "TZ=UTC date -d '2024-01-31 +1 day' +%F"),
    ("agent: sort unique and reverse", "printf '3\\n1\\n2\\n3\\n' | sort -u; printf '3\\n1\\n2\\n' | sort -rn; printf 'b\\nA\\na\\n' | sort -f"),
    ("agent: sort by multiple keys", "printf 'b 2\\na 2\\nc 1\\n' | sort -k2,2n -k1,1"),
    ("agent: cut characters and fields", "echo 'abcdef' | cut -c2-4; echo 'a:b:c' | cut -d: -f2-; echo 'a b' | cut -d' ' -f2"),
    ("agent: tail follow is refused or ends", "echo x >/tmp/f; tail -n 1 /tmp/f"),
    ("agent: long pipeline", "seq 1 1000 | grep 7 | sed 's/7/seven/' | sort | uniq | wc -l"),
    ("agent: loop over command output with spaces", "printf 'a b\\nc\\n' >/tmp/l; while read -r l; do echo \"[$l]\"; done </tmp/l; for w in $(cat /tmp/l); do echo \"<$w>\"; done"),
    ("agent: generate files from a template", "for n in 1 2; do sed \"s/NUM/$n/\" <<< 'file NUM' >/tmp/f$n; done; cat /tmp/f1 /tmp/f2"),
    ("agent: check exit codes explicitly", "grep -q x /dev/null; rc=$?; if [ $rc -eq 1 ]; then echo no-match; elif [ $rc -gt 1 ]; then echo error; fi"),
    ("agent: write JSON with printf", "printf '{\"name\":\"%s\",\"n\":%d}\\n' demo 3 | jq -c ."),
    ("agent: escape for JSON with jq", "msg='he said \"hi\"'; jq -n --arg m \"$msg\" '{msg: $m}' -c"),
    ("agent: process substitution", "diff <(printf 'a\\n') <(printf 'b\\n'); echo status=$?"),
    ("agent: stderr to a log and stdout to a pipe", "{ echo data; echo warn >&2; } 2>/tmp/err.log | tr a-z A-Z; cat /tmp/err.log"),
    ("agent: silence everything", "ls /nonexistent >/dev/null 2>&1; echo status=$?; ls /nonexistent &>/dev/null; echo status=$?"),
]

MISSING = 'ignored (missing-commands): bash-tool has no such command'
EXPECTED = {
    'agent: awk one-liner': (
        127, b'', b'bash: line 1: awk: command not found\n', MISSING,
    ),
    'agent: gzip': (
        0, b'status=127\n', b'bash: line 1: gzip: command not found\nbash: line 1: gzip: command not found\n', MISSING,
    ),
    'agent: tar': (
        0, b'status=127\n', b'', MISSING,
    ),
}

SYMLINKS = 'fixture (symlinks): links to absolute paths are refused, since WASI cannot create them'
EXPECTED |= {
    'agent: realpath and readlink': (
        1, b'', b"ln: failed to create symbolic link to '/tmp/r/d': links to absolute paths are unsupported in bash-tool; use a relative target\n", SYMLINKS,
    ),
}
