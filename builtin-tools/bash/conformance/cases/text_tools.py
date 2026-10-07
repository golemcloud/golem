"""Text tools: grep, diff, sed, jq, patch and cmp edge cases."""

CASES = [
    (
        "text-tools: jq del multi-index descending order",
        "jq -nc '[1,2,3,4] | del(.[0,2])'",
    ),
    (
        "text-tools: jq del object keys",
        """jq -nc '{"a":1,"b":2,"c":3} | del(.a,.c)'""",
    ),
    (
        "text-tools: jq del nested paths",
        "jq -nc '[[1,2],[3,4]] | del(.[0][1], .[1][0])'",
    ),
    (
        "text-tools: jq scan ignores caller flags for globalness",
        """jq -nc '"a1b22" | [scan("[0-9]")]'""",
    ),
    (
        "text-tools: diff DIR FILE without -r",
        "mkdir dz; echo content >dz/f1; echo content2 >f1; diff dz f1; echo status=$?",
    ),
    (
        "text-tools: diff FILE DIR without -r",
        "mkdir dz; echo content >dz/f1; echo content2 >f1; diff f1 dz; echo status=$?",
    ),
    (
        "text-tools: diff DIR1 DIR2 without -r stays at the top level",
        "mkdir -p d1/sub d2/sub; echo a >d1/x; echo b >d2/x; echo c >d1/only1; "
        "echo same >d1/sub/y; echo same >d2/sub/y; diff d1 d2; echo status=$?",
    ),
    (
        "text-tools: patch .rej header uses the stripped name",
        "mkdir -p a b; printf 'AAA\\nBBB\\nCCC\\n' >x.txt; "
        "printf -- '--- a/x.txt\\n+++ b/x.txt\\n@@ -1,3 +1,3 @@\\n one\\n-two\\n+TWO\\n three\\n' >p.diff; "
        "patch -p1 x.txt p.diff; echo status=$?; cat x.txt.rej",
    ),
    (
        "text-tools: patch -R on a not-yet-applied patch says Unreversed",
        "printf 'one\\ntwo\\nthree\\n' >x.txt; "
        "printf -- '--- x.txt\\n+++ x.txt\\n@@ -1,3 +1,3 @@\\n one\\n-two\\n+TWO\\n three\\n' >p.diff; "
        "patch -R x.txt p.diff; echo status=$?; cat x.txt.rej",
    ),
    (
        "text-tools: patch backs up on offset but not on an exact match",
        "printf 'one\\ntwo\\nthree\\n' >f; "
        "printf -- '--- f\\n+++ f\\n@@ -1,3 +1,3 @@\\n one\\n-two\\n+TWO\\n three\\n' >p.diff; "
        "patch f p.diff; echo status=$?; ls f.orig 2>/dev/null; echo none=$?; "
        "printf '0\\n0\\n0\\n0\\n0\\nline1\\nline2\\nline3\\n' >g; "
        "printf -- '--- g\\n+++ g\\n@@ -1,3 +1,3 @@\\n line1\\n-line2\\n+LINE2\\n line3\\n' >p2.diff; "
        "patch g p2.diff; echo status=$?; cat g.orig",
    ),
    (
        "text-tools: patch --dry-run omits the saving-rejects suffix",
        "printf 'AAA\\nBBB\\nCCC\\n' >x.txt; "
        "printf -- '--- x.txt\\n+++ x.txt\\n@@ -1,3 +1,3 @@\\n one\\n-two\\n+TWO\\n three\\n' >p.diff; "
        "patch --dry-run x.txt p.diff; echo status=$?; ls x.txt.rej 2>/dev/null; echo none=$?",
    ),
    ("text-tools: diff -d is a real accepted no-op", "diff -d /dev/null /dev/null"),

    # diff -e (ed script) hunk order: GNU lists hunks from the end of the file backwards, since
    # every hunk's line numbers are against the *untouched original* file and only stay valid
    # for an unapplied later hunk while it's still unmodified -- `diffutilslib::ed_diff`'s own
    # hunks were individually correct but left in forward order with numbers computed as if
    # earlier hunks had already shifted the file (both fixed by routing ed format through the
    # same grouped_ops/similar-crate diff every other format already uses, instead of the
    # library, mirroring how render_normal already bypasses it for -i/-w/-b).
    (
        "text-tools: diff -e lists multiple change hunks from the end of the file backwards",
        "printf '1\\n2\\n3\\n4\\n5\\n6\\n7\\n8\\n9\\n10\\n' >eo1; "
        "printf '1\\n2\\nX\\n4\\n5\\n6\\nY\\n8\\n9\\nZ\\n' >en1; diff -e eo1 en1",
    ),
    (
        "text-tools: diff -e's later hunk keeps the original file's line numbers after an earlier delete",
        "printf '1\\n2\\n3\\n4\\n5\\n6\\n7\\n8\\n' >eo2; "
        "printf '1\\n2\\n4\\n5\\n6\\n7\\nX\\n' >en2; diff -e eo2 en2",
    ),
    (
        "text-tools: diff -e's later hunk keeps the original file's line numbers after an earlier insert",
        "printf '1\\n2\\n3\\n4\\n5\\n6\\n7\\n8\\n' >eo3; "
        "printf '1\\n2\\nZ\\n3\\n4\\n5\\n6\\n7\\nX\\n' >en3; diff -e eo3 en3",
    ),
    (
        "text-tools: diff -e mixes a, c and d commands in one script, still bottom-up",
        "seq 1 20 >eo4; "
        "printf '1\\n2\\n3\\n4\\n6\\n7\\n8\\n9\\nTEN\\n11\\n12\\n13\\n14\\n15\\nNEW\\n16\\n17\\n18\\n19\\n20\\n' >en4; "
        "diff -e eo4 en4",
    ),
    (
        "text-tools: diff -e of byte-identical files missing a trailing newline still warns",
        "printf '1\\n2\\n3' >eo5; printf '1\\n2\\n3' >en5; diff -e eo5 en5; echo status=$?",
    ),

    # diff -e's escaping of a body line that is only `.`: it would otherwise read as the a/c
    # command's own end-of-text terminator, so GNU doubles it (`..`), follows with an ed `s/.//`
    # to undo the doubling, and -- since that `s/.//` always acts on ed's *current* (last
    # written) line -- splits the hunk into a fresh address-less `a` for anything still to come
    # after it, so the fix-up always lands on the right line.
    (
        "text-tools: diff -e escapes a lone . line in an append",
        "printf '1\\n2\\n' >edo1; printf '1\\n2\\n.\\nafter\\n' >edn1; diff -e edo1 edn1",
    ),
    (
        "text-tools: diff -e escapes a lone . line in a change",
        "printf '1\\n2\\n' >edo2; printf '1\\n.\\n.foo\\n' >edn2; diff -e edo2 edn2",
    ),
    (
        "text-tools: diff -e escapes several . lines in the same hunk",
        "printf '1\\n2\\n' >edo3; printf '1\\n.\\n.\\n' >edn3; diff -e edo3 edn3; rm -f edo3 edn3",
    ),
    (
        "text-tools: diff -e escapes a . line at the very end of a hunk",
        "printf '1\\n2\\n' >edo4; printf '1\\nbefore\\n.\\n' >edn4; diff -e edo4 edn4",
    ),
    (
        "text-tools: diff -e escapes a . line with content on both sides of it",
        "printf '1\\n2\\n' >edo5; printf '1\\nbefore\\n.\\nafter\\n' >edn5; diff -e edo5 edn5",
    ),
    ("text-tools: cmp -b prints differing bytes", "printf a >f1; printf b >f2; cmp -b f1 f2"),
    ("text-tools: patch -f is refused not misread", "printf '' | patch -f /dev/null"),
    ("text-tools: jq division by an exact-zero divisor raises jq's own error", "jq -n '1/0'"),
    (
        "text-tools: jq remainder by an exact-zero divisor raises jq's own error",
        "jq -n '5 % 0'",
    ),
    (
        "text-tools: jq indexing an object by a non-string key raises jq's own error",
        "jq -n '{}[0]'",
    ),
    (
        "text-tools: jq nan and infinite still work as native constants",
        "jq -nc '[nan, infinite, (nan|isnan), (infinite|isinfinite), (1|isfinite)]'",
    ),
    (
        "text-tools: grep -r with one plain file operand omits the filename prefix",
        "printf 'one\\ntwo\\n' > nums; grep -rn one nums",
    ),
    (
        "text-tools: grep -r with one directory operand keeps the filename prefix",
        "mkdir d; printf 'one\\n' > d/nums; grep -rn one d",
    ),
    (
        "text-tools: grep -m0 selects nothing",
        "printf 'one\\ntwo\\n' > f; grep -m0 one f; echo status=$?",
    ),
    (
        "text-tools: grep -A0 still separates non-adjacent matches",
        "printf 'one\\ntwo\\nthree\\nfour\\nfive\\nsix\\n' > f; grep -A0 -e two -e six f",
    ),
    (
        "text-tools: grep -z lets . match an embedded newline",
        "printf 'one\\ntwo\\x00three\\n' | grep -z -o 'one.two'",
    ),
    (
        "text-tools: grep -P \\d is ASCII-only",
        "printf '\\u0661\\u0662\\u0663\\n' | grep -P '\\d'; echo status=$?",
    ),
    (
        "text-tools: grep --include filters an explicit file operand",
        "printf 'one\\n' > nums.txt; grep --include='*.log' one nums.txt; echo status=$?",
    ),
    (
        "text-tools: grep --exclude-dir filters a directory operand itself",
        "mkdir sub; printf 'one\\n' > sub/f; grep -r --exclude-dir=sub one sub; echo status=$?",
    ),
    (
        "text-tools: grep -f pattern file with a raw non-UTF-8 byte matches that byte",
        "printf '\\xff\\n' > /tmp/gp033; printf '\\xff\\n' | grep -c -f /tmp/gp033",
    ),
    (
        "text-tools: jq replaces invalid UTF-8 input with U+FFFD on output",
        "printf '\"a\\xffb\"\\n' | jq -r .",
    ),
    (
        "text-tools: sed \\xff in a script matches and produces a raw byte",
        "printf '\\xff\\n' | sed 's/\\xff/X/'; printf 'x\\n' | sed 's/x/\\xff/' | od -An -tx1",
    ),
    (
        "text-tools: sed \\xff matches a raw byte in ERE mode too",
        "printf '\\xff\\n' | sed -E 's/\\xff/X/'",
    ),
    (
        "text-tools: sed y/// with a \\xff byte escape matches a raw byte",
        "printf '\\xff\\n' | sed 'y/\\xff/Z/'",
    ),
    (
        "text-tools: sed \\xff does not match a real multi-byte UTF-8 character",
        "printf '\\xc3\\xa9\\n' | sed 's/\\xff/X/'",
    ),
    (
        "text-tools: sed matches a raw byte from a shell variable, not just \\xff syntax",
        "x=$'\\xff'; printf '\\xff\\n' | sed \"s/$x/X/\"",
    ),
    (
        "text-tools: sed -i matches a raw byte from a shell variable too",
        "x=$'\\xff'; printf '\\xff\\n' > gc072; sed -i \"s/$x/X/\" gc072; od -An -tx1 gc072",
    ),
    (
        "text-tools: grep -f - reads patterns from the shell's stdin",
        "printf 'needle\\n' > /tmp/gft107; printf 'needle\\n' | grep -f - /tmp/gft107",
    ),
    (
        "text-tools: grep -E -F together is refused as conflicting",
        "echo 'a+b' | grep -E -F 'a+b'; echo status=$?",
    ),
    (
        "text-tools: grep -G -G repeated is fine",
        "echo a | grep -G -G a; echo status=$?",
    ),
    (
        "text-tools: grep -- group separator appears between file operands, not just within one",
        "printf 'a\\nmatch\\nb\\n' > gsep1; grep -C1 match gsep1 gsep1",
    ),
    (
        "text-tools: grep -E a{,2} treats a missing lower bound as 0",
        "printf 'a\\naa\\naaa\\n' | grep -E 'a{,2}$'",
    ),
    (
        "text-tools: grep BRE a\\{,2\\} treats a missing lower bound as 0",
        "printf 'a\\naa\\naaa\\n' | grep 'a\\{,2\\}$'",
    ),
    (
        "text-tools: grep BRE a leading \\{1\\} is literal text",
        "printf '{1}a\\nba\\n' | grep '\\{1\\}a'",
    ),
    (
        "text-tools: grep -E a leading {1} is dropped, not literal",
        "printf '{1}a\\nba\\nc\\n' | grep -E '{1}a'",
    ),
    (
        "text-tools: sed compile error names the -e expression, not a raw location",
        "printf '' | sed -e 's/x/y/' -e 's/a/b'; echo status=$?",
    ),
    (
        "text-tools: grep --color=always highlights matches with GNU's SGR sequences",
        "printf 'hello world\\n' | grep --color=always o | cat -v",
    ),
    (
        "text-tools: grep --color=always colors the filename, line number and separator",
        "printf 'hello world\\n' > /tmp/gc066; grep -n --color=always o /tmp/gc066 | cat -v",
    ),
    (
        "text-tools: grep --color=always with -o colors just the matched text",
        "printf 'hello world\\n' | grep --color=always -o o | cat -v",
    ),
    (
        "text-tools: grep --color=always colors matches on context lines and the -- separator",
        "printf 'match\\na\\nb\\nc\\nd\\nmatch\\n' | grep --color=always -C1 match | cat -v",
    ),
    (
        "text-tools: grep -v with --color=always produces no color",
        "printf 'hello world\\n' | grep --color=always -v xyz | cat -v",
    ),
    (
        "text-tools: grep --color=auto produces no color when stdout is not a terminal",
        "echo match | grep --color=auto match | cat -v",
    ),
    (
        "text-tools: grep --color (bare, defaults to auto) produces no color",
        "echo match | grep --color match | cat -v",
    ),
    (
        "text-tools: grep --color=never produces no color",
        "printf 'hello world\\n' | grep --color=never o",
    ),
    (
        "text-tools: sed -i continues past an unreadable file",
        "printf 'a\\n' >fa082; sed -i 's/a/z/' nope082 fa082; echo s=$?; cat fa082",
    ),
    (
        "text-tools: sed e command refuses with the README's exact wording",
        "printf 'a\\n' | sed '1e date'; echo sed=$?",
    ),
    (
        "text-tools: jq zero-step range yields nothing instead of hanging",
        "jq -nc '[range(0;10;0)]'",
    ),
    (
        "text-tools: jq moderate string repeat still works",
        "jq -nc '\"ab\" * 5'",
    ),
    (
        "text-tools: jq excessive string repeat refuses instead of aborting",
        "jq -n '\"a\" * 4294967296'; echo status=$?",
    ),
    (
        "text-tools: sed --help respects stdout redirection instead of leaking to real stdio",
        "sed --help >/dev/null; echo done",
    ),
    (
        "text-tools: sed -@ respects stderr redirection instead of leaking to real stdio",
        "sed -@ 2>/dev/null; echo status=$?",
    ),
    (
        "text-tools: sed --version reports its own name after another uucore tool has run",
        "wc --version >/dev/null; sed --version | head -1 | cut -d' ' -f1; echo status=$?",
    ),
]

# bash-tool refuses these real GNU diff/cmp/patch options outright (exit 2) instead of
# implementing them; the oracle would actually perform them. See the README's refusal
# paragraph ("Commands" section).
EXPECTED_REASON = "diff/cmp/patch options bash-tool refuses rather than implements"
EXPECTED = {
    "text-tools: sed e command refuses with the README's exact wording": (
        0,
        b"sed=2\n",
        b"sed: the 'e' command and substitute flag are unsupported in bash-tool: no shell to run\n",
        "GNU actually runs `date`; this build has no shell to run it, and refuses "
        "with the wording/status the README documents for that.",
    ),
    "text-tools: patch -f is refused not misread": (
        2,
        b"",
        b"patch: -f/--force is unsupported in bash-tool\n",
    ),
    "text-tools: grep BRE a leading \\{1\\} is literal text": (
        0,
        b"{1}a\n",
        b"",
        "Matches GNU byte for byte on stdout; GNU's own stderr warning "
        "('stray \\ before {') isn't reproduced, since it doesn't affect matching or "
        "status and grep's basic()/extended() translators have no way to emit one.",
    ),
    "text-tools: grep -E a leading {1} is dropped, not literal": (
        0,
        b"{1}a\nba\n",
        b"",
        "Matches GNU byte for byte on stdout; GNU's own stderr warning "
        "('{...} at start of expression') isn't reproduced, for the same reason as the "
        "BRE case above.",
    ),
}
