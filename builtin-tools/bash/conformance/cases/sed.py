"""sed conformance: GNU sed 4.9 in the oracle is the reference for status, stdout and stderr."""

LINES = "printf 'alpha one\\nbeta two\\ngamma three\\ndelta four\\nepsilon five\\n'"
CODE = "printf 'int main() {\\n  // comment\\n  return 0; // done\\n}\\n'"

CASES = [
    ("sed: substitute first", f"{LINES} | sed 's/a/A/'"),
    ("sed: substitute global", f"{LINES} | sed 's/a/A/g'"),
    ("sed: substitute nth", f"{LINES} | sed 's/a/A/2'"),
    ("sed: substitute nth and after", f"{LINES} | sed 's/a/A/2g'"),
    ("sed: substitute ignore case", "printf 'Foo foo FOO\\n' | sed 's/foo/x/Ig'"),
    ("sed: ampersand and escapes", f"{LINES} | sed 's/[a-z]*/<&>/; s/ /\\t/'"),
    ("sed: basic groups", f"{LINES} | sed 's/\\([a-z]*\\) \\([a-z]*\\)/\\2 \\1/'"),
    ("sed: basic alternation and intervals", "printf 'aaa b ab\\n' | sed 's/a\\{2\\}\\|b/X/g'"),
    ("sed: basic literal plus", "printf 'a+b aab\\n' | sed 's/a+b/X/g'"),
    ("sed: basic escaped plus", "printf 'a+b aab\\n' | sed 's/a\\+b/X/g'"),
    ("sed: extended groups", f"{LINES} | sed -E 's/([a-z]+) ([a-z]+)/\\2-\\1/'"),
    ("sed: extended r flag", "printf 'aab\\n' | sed -r 's/(a+)b/[\\1]/'"),
    ("sed: backreference in pattern", "printf 'abab cdce\\n' | sed -E 's/(ab)\\1/X/'"),
    ("sed: character classes", "printf 'a1 b22 c333\\n' | sed 's/[[:digit:]]\\+/#/g; s/[[:space:]]/_/g'"),
    ("sed: case conversion", "printf 'hello world\\n' | sed -E 's/(\\w+) (\\w+)/\\u\\1 \\U\\2\\E!/'"),
    ("sed: newline in replacement", "printf 'a,b,c\\n' | sed 's/,/\\n/g'"),
    ("sed: custom delimiter", "printf '/usr/local/bin\\n' | sed 's|/usr/local|/opt|'"),
    ("sed: empty regex reuses last", "printf 'foo bar foo\\n' | sed '/foo/s//X/g'"),
    ("sed: print flag with n", f"{LINES} | sed -n 's/beta/BETA/p'"),
    ("sed: delete lines", f"{LINES} | sed '2d;4d'"),
    ("sed: delete regex", f"{CODE} | sed '/^ *\\/\\//d'"),
    ("sed: strip comments", f"{CODE} | sed 's|[[:space:]]*//.*||'"),
    ("sed: line ranges", f"{LINES} | sed -n '2,4p'"),
    ("sed: regex ranges", f"{LINES} | sed -n '/beta/,/delta/p'"),
    ("sed: relative range", f"{LINES} | sed -n '/beta/,+1p'"),
    ("sed: step addresses", "printf '1\\n2\\n3\\n4\\n5\\n6\\n7\\n8\\n9\\n10\\n' | sed -n '0~3p'"),
    ("sed: first step", "printf '1\\n2\\n3\\n4\\n5\\n6\\n' | sed -n '1~2p'"),
    ("sed: zero address range", f"{LINES} | sed '0,/a/s//A/'"),
    ("sed: negation", f"{LINES} | sed -n '/a/!p'"),
    ("sed: last line", f"{LINES} | sed -n '$p'; {LINES} | sed '$d'"),
    ("sed: last line unterminated", "printf 'a\\nb' | sed 's/b/B/'; echo; printf 'a\\nb' | sed -n '$p'"),
    ("sed: blocks", f"{LINES} | sed -n '/a/{{s/a/A/g;p;}}'"),
    ("sed: append insert change", f"{LINES} | sed '2a\\\nafter two' | sed '1i\\\nfirst' | sed '/gamma/c\\\nchanged'"),
    ("sed: gnu one-line text commands", f"{LINES} | sed -e '1i header' -e '$a footer' -e '3c three'"),
    ("sed: transliterate", f"{LINES} | sed 'y/abcdefghijklmnopqrstuvwxyz/ABCDEFGHIJKLMNOPQRSTUVWXYZ/'"),
    ("sed: quit", f"{LINES} | sed '2q'"),
    ("sed: quit with status", f"{LINES} | sed '3q7'; echo status=$?"),
    ("sed: quit silently", f"{LINES} | sed '3Q'; echo status=$?"),
    ("sed: line numbers", f"{LINES} | sed -n '/e/='"),
    ("sed: count lines", f"{LINES} | sed -n '$='"),
    ("sed: list", "printf 'a\\tb\\\\c\\001\\n' | sed -n 'l'"),
    ("sed: hold space reverse", f"{LINES} | sed -n '1!G;h;$p'"),
    ("sed: hold and exchange", f"{LINES} | sed -n 'x;$p'"),
    ("sed: join lines", f"{LINES} | sed ':a;N;$!ba;s/\\n/,/g'"),
    ("sed: join pairs", f"{LINES} | sed 'N;s/\\n/ + /'"),
    ("sed: next line", f"{LINES} | sed 'n;d'"),
    ("sed: next line quiet", f"{LINES} | sed -n 'n;p'"),
    ("sed: multiline delete", "printf 'a\\n\\n\\nb\\n\\nc\\n' | sed '/^$/N;/\\n$/D'"),
    ("sed: print first line of pattern", f"{LINES} | sed -n 'N;P'"),
    ("sed: branch on substitution", "printf 'a.b.c\\n' | sed ':x;s/\\./-/;tx'"),
    ("sed: branch if not substituted", "printf 'yes\\nno\\n' | sed 's/yes/Y/;T;s/$/!/'"),
    ("sed: comments and newlines", "printf 'a\\nb\\n' | sed '# leading comment\ns/a/A/\n# another\ns/b/B/'"),
    ("sed: multiple expressions", f"{LINES} | sed -e 's/a/A/' -e 's/e/E/'"),
    ("sed: script file", f"printf 's/one/1/\\ns/two/2/\\n' > /tmp/s.sed; {LINES} | sed -f /tmp/s.sed"),
    ("sed: file operands", "printf 'x\\n' > /tmp/a; printf 'y\\n' > /tmp/b; sed -n '$p;1p' /tmp/a /tmp/b"),
    ("sed: separate files", "printf 'x1\\nx2\\n' > /tmp/a; printf 'y1\\ny2\\n' > /tmp/b; sed -s -n '$p' /tmp/a /tmp/b"),
    ("sed: missing file", "printf 'x\\n' > /tmp/a; sed p /tmp/nope /tmp/a; echo status=$?"),
    ("sed: in place", f"{LINES} > /tmp/f; sed -i 's/beta/BETA/' /tmp/f; cat /tmp/f"),
    ("sed: in place with suffix", f"{LINES} > /tmp/f; sed -i.bak '1d' /tmp/f; cat /tmp/f; head -n 1 /tmp/f.bak"),
    ("sed: in place clustered", f"{LINES} > /tmp/f; sed -ni '2p' /tmp/f; cat /tmp/f"),
    ("sed: write file", f"{LINES} | sed -n '/a/w /tmp/w'; cat /tmp/w"),
    ("sed: read file", "printf 'INSERT\\n' > /tmp/r; printf 'a\\nb\\n' | sed '1r /tmp/r'"),
    ("sed: null data", "printf 'a\\0b\\0' | sed -z 's/^/>/' | tr '\\0' '\\n'"),
    ("sed: filename and stdin dash", "printf 'x\\n' > /tmp/a; printf 'y\\n' | sed 's/^/:/' /tmp/a -"),
    ("sed: unicode", "printf 'héllo wörld\\n' | sed 's/ö/o/; s/h./H/'"),
    ("sed: crlf", "printf 'a\\r\\nb\\r\\n' | sed 's/\\r$//' | cat -v"),
    ("sed: unterminated s", "printf 'a\\n' | sed 's/a/b'; echo status=$?"),
    ("sed: unknown command", "printf 'a\\n' | sed 'k'; echo status=$?"),
    ("sed: missing script", "sed; echo status=$?"),
    ("sed: exec refused", "printf 'a\\n' | sed 's/a/echo hi/e'; echo status=$?"),
    ("sed: endless quit", "while :; do echo x; done | sed 2q | cat"),
    ("sed: endless address quit", "i=0; while :; do i=$((i+1)); echo $i; done | sed -n '3{p;q}'"),
    ("sed: endless last line lookahead", "while :; do echo x; done | sed '$d' | head -n 2"),
    ("sed: v is a no-op", "echo before; echo a | sed v; echo a | sed -n 'v 4.2;p'; echo a | sed -n 'v 3.99;p'; echo after"),
]

EXPECTED_REASON = "bash-tool refuses sed's `e` at compile time: there is no shell to run it"
EXPECTED = {
    # reworded and re-statused to this tool's own `… is unsupported in bash-tool`
    # refusal convention (status 2), not the fork's own compile-error status/wording verbatim
    # (status 1, with a `<script argument N>:L:C: error:` location prefix the README never
    # promised).
    "sed: exec refused": (
        0,
        b"status=2\n",
        b"sed: the 'e' command and substitute flag are unsupported in bash-tool: no shell to run\n",
    ),
}

# Status and stdout come from GNU sed.
EXPECTED_STDERR_REASON = "uutils sed words its diagnostics differently from GNU sed"
EXPECTED_STDERR = {
    "sed: missing script": b"sed: missing script\n",
}
