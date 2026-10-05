"""xargs conformance: GNU findutils 4.10 xargs in the oracle is the reference for status, stdout and
stderr. Commands must exist as programs in the oracle too, so cases use echo, printf, cat, grep, wc,
jq, true and false rather than shell functions."""

CASES = [
    ("xargs: default command is echo", "printf 'a b\\nc\\n' | xargs"),
    ("xargs: max args", "printf '1 2 3 4 5\\n' | xargs -n 2 echo"),
    ("xargs: quotes and backslashes", "printf \"'a b' \\\"c d\\\" e\\\\\\\\ f\\n\" | xargs -n1 echo"),
    ("xargs: unmatched quote", "printf \"it's\\n\" | xargs echo; echo status=$?; printf 'a \"b' | xargs echo; echo status=$?"),
    ("xargs: null delimiter", "printf 'a b\\0\\0c\\0' | xargs -0 -n1 echo x"),
    ("xargs: custom delimiter keeps empty items", "printf 'a:b::c' | xargs -d: -n1 echo x"),
    ("xargs: delimiter escapes", "printf 'a\\tb' | xargs -d '\\t' -n1 echo; printf 'aAb' | xargs -d '\\x41' echo; printf 'a b' | xargs -d '\\040' -n1 echo"),
    ("xargs: replace string", "printf 'one\\ntwo\\n' | xargs -I{} echo 'item-{}-{}'"),
    ("xargs: replace takes whole lines", "printf '  a b  \\n\\n c \\n' | xargs -I{} echo '[{}]'; printf 'x\\n' | xargs -i echo {} y"),
    ("xargs: max lines", "printf 'a b\\nc d \\ne\\nf\\n' | xargs -L 2 echo; printf 'a\\nb\\n' | xargs -l echo"),
    ("xargs: empty input", "printf '' | xargs echo hi; printf '\\n\\n' | xargs echo x; printf '' | xargs -r echo hi; echo status=$?; printf '' | xargs -I{} echo {}; echo status=$?"),
    ("xargs: verbose", "printf 'a b c\\n' | xargs -t -n2 echo; printf '' | xargs -t echo"),
    ("xargs: verbose quoting", "printf 'a b\\0it'\\''s\\0x=y\\0\\0a\\tb\\0plain\\0' | xargs -0 -t true"),
    ("xargs: eof string", "printf 'a b EOF c\\n' | xargs -E EOF echo; printf 'a b _ c\\n' | xargs -e_ echo; printf 'a b\\n' | xargs -E '' echo"),
    ("xargs: arg file", "printf 'x\\ny\\n' > /tmp/args; xargs -a /tmp/args echo; xargs --arg-file=/tmp/nope echo; echo status=$?"),
    ("xargs: max chars", "printf 'a b c d e' | xargs -s 10 echo; printf 'abcdefghijk' | xargs -s 10 echo; echo status=$?"),
    ("xargs: exit on size", "printf 'a b c' | xargs -x -n 3 -s 9 echo; echo status=$?; printf 'a b c' | xargs -n 3 -s 9 echo"),
    ("xargs: failing command gives 123", "printf 'x\\n' > /tmp/x1; printf 'y\\n' > /tmp/y1; printf '/tmp/x1\\n/tmp/y1\\n/tmp/x1\\n' | xargs -n1 grep -q x; echo status=$?"),
    ("xargs: status 255 aborts", "printf 'a\\nb\\n' | xargs -n1 jq -n '\"x\\n\" | halt_error(255)'; echo status=$?"),
    ("xargs: command not found", "echo a | xargs nosuchcmd; echo status=$?"),
    ("xargs: directory as command", "echo a | xargs /tmp; echo status=$?"),
    ("xargs: option errors", "xargs -Z; echo $?; xargs --bogus; echo $?; xargs -n; echo $?; xargs -n 0; echo $?; xargs -n 1x; echo $?; xargs -P x; echo $?"),
    ("xargs: delimiter errors", "xargs -d ab; echo $?; xargs -d '\\q'; echo $?; xargs -d ''; echo $?"),
    ("xargs: mutually exclusive options", "printf 'a b\\n' | xargs -L1 -n1 echo; printf 'a b\\nc\\n' | xargs -n1 -L1 echo; printf 'a\\n' | xargs -n1 -I{} echo {}"),
    ("xargs: parallel is accepted", "printf 'a\\nb\\n' | xargs -P 4 -n1 echo | sort; printf 'a\\n' | xargs --max-procs=0 echo"),
    ("xargs: long options", "printf 'a b\\n' | xargs --max-args=1 --verbose echo; printf 'a:b' | xargs --delimiter=: --no-run-if-empty echo"),
    ("xargs: double dash and passthrough", "printf 'a\\n' | xargs -- echo x; printf 'a\\n' | xargs echo -n; echo"),
    ("xargs: child stdin is empty", "printf 'a\\nb\\n' | xargs -I{} cat; echo status=$?"),
    ("xargs: find pipeline", "mkdir -p /tmp/x/s && printf 'ab\\n' > '/tmp/x/a b' && printf 'c\\n' > /tmp/x/s/c && find /tmp/x -type f -print0 | xargs -0 wc -c | sort"),
    ("xargs: endless producer with replace", "while :; do echo x; done | xargs -I{} echo {} | head -n 2"),
    ("xargs: endless producer default batching", "while :; do echo x; done | xargs | head -c 10; echo"),
    ("xargs: refuses prompt", "printf 'a\\n' | xargs -p echo; echo status=$?"),
]

EXPECTED_REASON = "the oracle cannot prompt without a terminal; bash-tool refuses -p up front"
EXPECTED = {
    "xargs: refuses prompt": (0, b"status=2\n", b"xargs: -p is unsupported in bash-tool\n"),
}

EXPECTED_STDERR = {}
