"""diff/cmp/patch conformance: GNU diffutils 3.12 and GNU patch 2.8 in the oracle are the
reference for status, stdout and stderr.

Any script that would embed a real file's mtime in its output (a `-u`/`-c`/`-y` header without
`--label`) is avoided here — the WASM sandbox and the oracle container never share a clock, so
those cases either pass `--label` (which drops the timestamp from GNU's own header entirely),
stick to a header-less format (normal, ed, `-q`), or, for `patch`, embed a literal patch text
with a fake timestamp instead of generating one with `diff` inside the script.
"""

CASES = [
    # --- diff ---------------------------------------------------------------------------------
    (
        "diff: normal format default",
        "printf 'one\\ntwo\\nthree\\n' > /tmp/a; printf 'one\\nTWO\\nthree\\n' > /tmp/b; diff /tmp/a /tmp/b",
    ),
    (
        "diff: unified with labels",
        "printf '1\\n2\\n3\\n4\\n5\\n' > /tmp/a; printf '1\\n2\\nX\\n4\\n5\\n' > /tmp/b; "
        "diff -u --label L --label R /tmp/a /tmp/b",
    ),
    (
        "diff: context with labels",
        "printf 'one\\n' > /tmp/a; printf 'two\\n' > /tmp/b; diff -c --label L --label R /tmp/a /tmp/b",
    ),
    (
        "diff: unified custom context",
        "printf '1\\n2\\n3\\n4\\n5\\n6\\n7\\n' > /tmp/a; printf '1\\n2\\n3\\nX\\n5\\n6\\n7\\n' > /tmp/b; "
        "diff -U1 --label L --label R /tmp/a /tmp/b",
    ),
    (
        "diff: context custom count",
        "printf '1\\n2\\n3\\n4\\n5\\n' > /tmp/a; printf '1\\n2\\nX\\n4\\n5\\n' > /tmp/b; "
        "diff -C1 --label L --label R /tmp/a /tmp/b",
    ),
    (
        "diff: ed format",
        "printf 'one\\ntwo\\nthree\\n' > /tmp/a; printf 'one\\nTWO\\nthree\\n' > /tmp/b; diff -e /tmp/a /tmp/b",
    ),
    (
        "diff: brief differ",
        "printf 'x\\n' > /tmp/a; printf 'y\\n' > /tmp/b; diff -q /tmp/a /tmp/b",
    ),
    (
        "diff: report identical",
        "printf 'x\\n' > /tmp/a; printf 'x\\n' > /tmp/c; diff -s /tmp/a /tmp/c",
    ),
    (
        "diff: identical files are silent",
        "printf 'x\\n' > /tmp/a; printf 'x\\n' > /tmp/c; diff /tmp/a /tmp/c; echo status=$?",
    ),
    (
        "diff: binary files differ",
        "printf 'a\\000b' > /tmp/bin1; printf 'a\\000c' > /tmp/bin2; diff /tmp/bin1 /tmp/bin2",
    ),
    (
        "diff: binary files identical",
        "printf 'a\\000b' > /tmp/bin1; cp /tmp/bin1 /tmp/bin3; diff /tmp/bin1 /tmp/bin3; echo status=$?",
    ),
    (
        "diff: new file treats missing as empty",
        "printf 'hello\\n' > /tmp/b; diff -N /tmp/nope /tmp/b",
    ),
    (
        "diff: missing file without new-file errors",
        "printf 'hello\\n' > /tmp/b; diff /tmp/nope /tmp/b; echo status=$?",
    ),
    (
        "diff: stdin operand",
        "printf 'hello\\n' > /tmp/b; printf 'hello\\n' | diff - /tmp/b; echo status=$?",
    ),
    (
        "diff: recursive walk",
        "mkdir -p /tmp/rd/a/sub /tmp/rd/b/sub; cd /tmp/rd; "
        "printf 'old\\n' > a/common.txt; printf 'new\\n' > b/common.txt; "
        "printf 'a\\n' > a/onlya.txt; printf 'b\\n' > b/onlyb.txt; "
        "printf '1\\n' > a/sub/f.txt; printf '2\\n' > b/sub/f.txt; "
        "diff -r a b",
    ),
    (
        "diff: recursive type mismatch",
        "mkdir -p /tmp/rt/a/x /tmp/rt/b; cd /tmp/rt; printf 'hi\\n' > b/x; diff -r a b",
    ),
    (
        "diff: ignore case treats lines as identical",
        "printf 'Hello World\\n' > /tmp/ica; printf 'hello world\\n' > /tmp/icb; "
        "diff -i /tmp/ica /tmp/icb; echo status=$?",
    ),
    (
        "diff: ignore all space treats lines as identical",
        "printf 'hello world\\n' > /tmp/iwa; printf 'helloworld\\n' > /tmp/iwb; "
        "diff -w /tmp/iwa /tmp/iwb; echo status=$?",
    ),
    (
        "diff: ignore space change collapses amount not presence",
        "printf '  x\\n' > /tmp/iba; printf '   x\\n' > /tmp/ibb; diff -b /tmp/iba /tmp/ibb; echo status=$?; "
        "printf ' x\\n' > /tmp/ibc; printf 'x\\n' > /tmp/ibd; diff -b /tmp/ibc /tmp/ibd; echo status=$?",
    ),
    (
        "diff: ignore space change real change still reported",
        "printf 'a   b\\nold\\nc\\n' > /tmp/ibx; printf 'a b\\nnew\\nc\\n' > /tmp/iby; "
        "diff -bu --label L --label R /tmp/ibx /tmp/iby",
    ),
    (
        "diff: ignore blank lines still refused",
        "printf 'x\\n' > /tmp/a; printf 'y\\n' > /tmp/b; diff -B /tmp/a /tmp/b; echo status=$?",
    ),
    (
        "diff: color flag accepted",
        "printf 'x\\n' > /tmp/a; printf 'y\\n' > /tmp/b; diff --color -q /tmp/a /tmp/b",
    ),
    (
        "diff: side by side",
        "printf 'one\\ntwo\\nthree\\n' > /tmp/a; printf 'one\\nTWO\\nthree\\nfour\\n' > /tmp/b; diff -y /tmp/a /tmp/b",
    ),
    # --- cmp ------------------------------------------------------------------------------------
    (
        "cmp: default differ message",
        "printf 'aaaa' > /tmp/f1; printf 'aaab' > /tmp/f2; cmp /tmp/f1 /tmp/f2; echo status=$?",
    ),
    (
        "cmp: identical files silent",
        "printf 'aaaa' > /tmp/f1; cp /tmp/f1 /tmp/f1b; cmp /tmp/f1 /tmp/f1b; echo status=$?",
    ),
    (
        "cmp: verbose lists differences",
        "printf 'aaaa' > /tmp/f1; printf 'abab' > /tmp/f2; cmp -l /tmp/f1 /tmp/f2; echo status=$?",
    ),
    (
        "cmp: silent suppresses output",
        "printf 'aaaa' > /tmp/f1; printf 'aaab' > /tmp/f2; cmp -s /tmp/f1 /tmp/f2; echo status=$?",
    ),
    (
        "cmp: eof on shorter file",
        "printf 'aaa' > /tmp/short; printf 'aaaa' > /tmp/long; cmp /tmp/short /tmp/long; echo status=$?",
    ),
    (
        "cmp: eof at a line boundary",
        "printf 'x\\ny\\n' > /tmp/l1; printf 'x\\ny\\nz' > /tmp/l2; cmp /tmp/l1 /tmp/l2; echo status=$?; cmp -l /tmp/l1 /tmp/l2; echo status=$?",
    ),
    (
        "cmp: missing file errors",
        "printf 'aaaa' > /tmp/f1; cmp /tmp/nope /tmp/f1; echo status=$?",
    ),
    # --- patch ------------------------------------------------------------------------------------
    (
        "patch: apply from stdin",
        "printf 'line1\\nline2\\nline3\\n' > /tmp/pt1.txt; "
        "printf -- '--- a\\n+++ b\\n@@ -1,3 +1,3 @@\\n line1\\n-line2\\n+line2-changed\\n line3\\n' | "
        "patch /tmp/pt1.txt; cat /tmp/pt1.txt",
    ),
    (
        "patch: apply via -i flag",
        "printf 'line1\\nline2\\nline3\\n' > /tmp/pt2.txt; "
        "printf -- '--- a\\n+++ b\\n@@ -1,3 +1,3 @@\\n line1\\n-line2\\n+line2-changed\\n line3\\n' > /tmp/pt2.patch; "
        "patch -i /tmp/pt2.patch /tmp/pt2.txt; cat /tmp/pt2.txt",
    ),
    (
        "patch: positional file and patchfile",
        "printf 'line1\\nline2\\nline3\\n' > /tmp/pt3.txt; "
        "printf -- '--- a\\n+++ b\\n@@ -1,3 +1,3 @@\\n line1\\n-line2\\n+line2-changed\\n line3\\n' > /tmp/pt3.patch; "
        "patch /tmp/pt3.txt /tmp/pt3.patch; cat /tmp/pt3.txt",
    ),
    (
        "patch: git style p1 strip",
        "printf 'one\\ntwo\\nthree\\n' > /tmp/x.txt; cd /tmp; "
        "printf -- '--- a/x.txt\\n+++ b/x.txt\\n@@ -1,3 +1,3 @@\\n one\\n-two\\n+TWO\\n three\\n' | "
        "patch -p1; cat /tmp/x.txt",
    ),
    (
        "patch: reverse undoes a patch",
        "printf 'one\\nTWO\\nthree\\n' > /tmp/xr.txt; cd /tmp; "
        "printf -- '--- a/xr.txt\\n+++ b/xr.txt\\n@@ -1,3 +1,3 @@\\n one\\n-two\\n+TWO\\n three\\n' | "
        "patch -p1 -R; cat /tmp/xr.txt",
    ),
    (
        "patch: dry run does not modify",
        "printf 'line1\\nline2\\nline3\\n' > /tmp/pt4.txt; "
        "printf -- '--- a\\n+++ b\\n@@ -1,3 +1,3 @@\\n line1\\n-line2\\n+line2-changed\\n line3\\n' | "
        "patch --dry-run /tmp/pt4.txt; cat /tmp/pt4.txt",
    ),
    (
        "patch: output flag redirects",
        "printf 'line1\\nline2\\nline3\\n' > /tmp/pt5.txt; "
        "printf -- '--- a\\n+++ b\\n@@ -1,3 +1,3 @@\\n line1\\n-line2\\n+line2-changed\\n line3\\n' | "
        "patch -o /tmp/pt5.out /tmp/pt5.txt; cat /tmp/pt5.out; echo ---; cat /tmp/pt5.txt",
    ),
    (
        "patch: silent suppresses success message",
        "printf 'line1\\nline2\\nline3\\n' > /tmp/pt6.txt; "
        "printf -- '--- a\\n+++ b\\n@@ -1,3 +1,3 @@\\n line1\\n-line2\\n+line2-changed\\n line3\\n' | "
        "patch -s /tmp/pt6.txt; cat /tmp/pt6.txt",
    ),
    (
        "patch: hunk offset search",
        "printf '0\\n0\\n0\\n0\\n0\\nline1\\nline2\\nline3\\n' > /tmp/pt7.txt; "
        "printf -- '--- a\\n+++ b\\n@@ -1,3 +1,3 @@\\n line1\\n-line2\\n+line2-changed\\n line3\\n' | "
        "patch /tmp/pt7.txt; cat /tmp/pt7.txt",
    ),
    (
        "patch: failing hunk writes rej",
        "printf 'AAA\\nBBB\\nCCC\\n' > /tmp/pt8.txt; "
        "printf -- '--- a\\n+++ b\\n@@ -1,3 +1,3 @@\\n line1\\n-line2\\n+line2-changed\\n line3\\n' | "
        "patch /tmp/pt8.txt; echo ---; cat /tmp/pt8.txt; echo ---; cat /tmp/pt8.txt.rej",
    ),
    (
        "patch: creation via dev null",
        "cd /tmp; printf -- '--- /dev/null\\n+++ newfile.txt\\n@@ -0,0 +1,2 @@\\n+hello\\n+world\\n' | "
        "patch -p0; cat /tmp/newfile.txt",
    ),
    (
        "patch: deletion via dev null",
        "printf 'hello\\nworld\\n' > /tmp/delfile.txt; cd /tmp; "
        "printf -- '--- delfile.txt\\n+++ /dev/null\\n@@ -1,2 +0,0 @@\\n-hello\\n-world\\n' | "
        "patch -p0; test -f /tmp/delfile.txt && echo EXISTS || echo DELETED",
    ),
    (
        "patch: multi file patch",
        "printf 'alpha\\n' > /tmp/alpha.txt; cd /tmp; "
        "printf -- 'diff --git a/alpha.txt b/alpha.txt\\n--- a/alpha.txt\\n+++ b/alpha.txt\\n"
        "@@ -1 +1 @@\\n-alpha\\n+ALPHA\\ndiff --git a/beta.txt b/beta.txt\\nnew file mode 100644\\n"
        "--- /dev/null\\n+++ b/beta.txt\\n@@ -0,0 +1 @@\\n+beta\\n' | patch -p1; "
        "cat /tmp/alpha.txt; cat /tmp/beta.txt",
    ),
    (
        "patch: already applied is detected",
        "printf 'line1\\nline2-changed\\nline3\\n' > /tmp/pt9.txt; "
        "printf -- '--- a\\n+++ b\\n@@ -1,3 +1,3 @@\\n line1\\n-line2\\n+line2-changed\\n line3\\n' | "
        "patch -N /tmp/pt9.txt; cat /tmp/pt9.txt",
    ),
    # --- item 2: diffutilslib doesn't group nearby changes into one hunk; ours does -----------
    (
        "diff: unified groups nearby changes into one hunk",
        "printf 'one\\ntwo\\nthree\\nfour\\nfive\\nsix\\nseven\\n' > /tmp/ga; "
        "printf 'one\\ntwo\\nTHREE\\nfour\\nfive\\nsix\\nseven\\neight\\n' > /tmp/gb; "
        "diff -u --label L --label R /tmp/ga /tmp/gb",
    ),
    (
        "diff: context groups nearby changes into one hunk",
        "printf 'one\\ntwo\\nthree\\nfour\\nfive\\nsix\\nseven\\n' > /tmp/ga; "
        "printf 'one\\ntwo\\nTHREE\\nfour\\nfive\\nsix\\nseven\\neight\\n' > /tmp/gb; "
        "diff -c --label L --label R /tmp/ga /tmp/gb",
    ),
    # --- item 3: bundled short options ----------------------------------------------------------
    (
        "diff: bundled short options ruaN",
        "printf 'one\\ntwo\\nthree\\n' > /tmp/ba; printf 'one\\nTWO\\nthree\\n' > /tmp/bb; "
        "diff -ruaN --label L --label R /tmp/ba /tmp/bb",
    ),
    # --- `-r` header echoes original argv tokens verbatim ----------------------
    (
        "diff: recursive header echoes bundled short options",
        "mkdir -p /tmp/rh1/o /tmp/rh1/n; cd /tmp/rh1; "
        "printf 'one\\n' > o/f; printf 'two\\n' > n/f; diff -ruN --label L --label R o n",
    ),
    (
        "diff: recursive header echoes separate short options",
        "mkdir -p /tmp/rh2/o /tmp/rh2/n; cd /tmp/rh2; "
        "printf 'one\\n' > o/f; printf 'two\\n' > n/f; diff -r -u -N --label L --label R o n",
    ),
    (
        "diff: recursive header echoes long options",
        "mkdir -p /tmp/rh3/o /tmp/rh3/n; cd /tmp/rh3; "
        "printf 'one\\n' > o/f; printf 'two\\n' > n/f; diff --recursive --unified --label L --label R o n",
    ),
    (
        "diff: recursive header echoes separate numeric argument",
        "mkdir -p /tmp/rh4/o /tmp/rh4/n; cd /tmp/rh4; "
        "printf 'one\\n' > o/f; printf 'two\\n' > n/f; diff -r -U 5 --label L --label R o n",
    ),
    (
        "diff: recursive header echoes attached numeric argument",
        "mkdir -p /tmp/rh5/o /tmp/rh5/n; cd /tmp/rh5; "
        "printf 'one\\n' > o/f; printf 'two\\n' > n/f; diff -rU5 --label L --label R o n",
    ),
    (
        "diff: recursive header quotes an option needing escaping",
        "mkdir -p /tmp/rh6/o /tmp/rh6/n; cd /tmp/rh6; "
        "printf 'one\\n' > o/f; printf 'two\\n' > n/f; diff -ru --color=never --label L --label R o n",
    ),
    # --- item 4: default -p rule + can't-find-file transcript ------------------------------------
    (
        "patch: default strip cant find file transcript",
        "mkdir -p /tmp/proj4/src; cd /tmp/proj4; printf 'fn main() {}\\n' > src/main.rs; "
        "printf -- '--- a/src/main.rs\\n+++ b/src/main.rs\\n@@ -1 +1 @@\\n"
        "-fn main() {}\\n+fn main() { println!(\"hi\"); }\\n' | patch; echo status=$?",
    ),
    # --- GNU diagnostics and statuses ----------------------------------------
    (
        "patch: empty input is silent success",
        "printf 'a\\n' > /tmp/pe1.txt; printf '' | patch /tmp/pe1.txt; echo status=$?",
    ),
    (
        "patch: garbage only input is reported",
        "printf 'a\\n' > /tmp/pe2.txt; printf 'garbage\\n' | patch /tmp/pe2.txt; echo status=$?",
    ),
    (
        "patch: truncated hunk reports unexpected eof",
        "printf 'a\\n' > /tmp/pe3.txt; "
        "printf -- '--- f\\n+++ f\\n@@ -100,5 +100,5 @@\\n-x\\n+y\\n' | patch /tmp/pe3.txt; echo status=$?",
    ),
    (
        "diff: invalid context length diagnostic",
        "printf 'a\\n' > /tmp/de1; diff -U -1 /tmp/de1 /tmp/de1; echo status=$?",
    ),
    (
        "diff: invalid width diagnostic",
        "printf 'a\\n' > /tmp/de2; diff -y -W 0 /tmp/de2 /tmp/de2; echo status=$?",
    ),
    (
        "diff: missing operand diagnostic",
        "diff; echo status=$?",
    ),
    (
        "diff: extra operand diagnostic",
        "printf 'a\\n' > /tmp/de3; diff /tmp/de3 /tmp/de3 /tmp/de3; echo status=$?",
    ),
    (
        "diff: unrecognized long option diagnostic",
        "printf 'a\\n' > /tmp/de4; diff --bogus /tmp/de4 /tmp/de4; echo status=$?",
    ),
    (
        "diff: invalid short option diagnostic",
        "printf 'a\\n' > /tmp/de5; diff -K /tmp/de5 /tmp/de5; echo status=$?",
    ),
    (
        "diff: option requires argument diagnostic",
        "diff -U; echo status=$?",
    ),
    (
        "cmp: ignore initial skips both sides",
        "printf 'abcdef\\n' > /tmp/ce1; printf 'ABCdef\\n' > /tmp/ce2; "
        "cmp -i 3 /tmp/ce1 /tmp/ce2; echo status=$?",
    ),
    (
        "cmp: ignore initial colon pair skips independently",
        "printf 'abcdef\\n' > /tmp/ce3; printf 'ABCdef\\n' > /tmp/ce4; "
        "cmp -i 3:0 /tmp/ce3 /tmp/ce4; echo status=$?",
    ),
    (
        "cmp: positional skip applies to first file only",
        "printf 'abcdef\\n' > /tmp/ce5; printf 'ABCdef\\n' > /tmp/ce6; "
        "cmp /tmp/ce5 /tmp/ce6 3; echo status=$?",
    ),
    (
        "cmp: invalid ignore initial value diagnostic",
        "printf 'a\\n' > /tmp/ce7; cmp /tmp/ce7 /tmp/ce7 /tmp/ce7; echo status=$?",
    ),
    (
        "cmp: missing operand diagnostic",
        "cmp; echo status=$?",
    ),
    (
        "cmp: single operand defaults second to stdin",
        "printf 'a\\n' > /tmp/ce8; cmp /tmp/ce8; echo status=$?",
    ),
    # --- round trips --------------------------------------------------------------------------
    (
        "diff: round trip forward and reverse",
        "mkdir -p /tmp/rtd && cd /tmp/rtd; "
        "printf 'one\\ntwo\\nthree\\n' > a; printf 'one\\nTWO\\nthree\\nfour\\n' > b; "
        "diff -u a b > x.patch; cp a a.new; patch a.new < x.patch >/dev/null; "
        "diff -q a.new b && echo FORWARD_OK; "
        "patch -R a.new < x.patch >/dev/null; diff -q a.new a && echo REVERSE_OK",
    ),
    (
        "diff: round trip via cmp",
        "mkdir -p /tmp/rtc && cd /tmp/rtc; "
        "printf 'alpha\\nbeta\\ngamma\\n' > a; printf 'alpha\\nBETA\\ngamma\\n' > b; "
        "diff -u a b > x.patch; cp a a.new; patch -s a.new < x.patch; "
        "cmp a.new b && echo CMP_OK",
    ),
    (
        "diff: round trip -Naur tree then patch -p1",
        "mkdir -p /tmp/rtn/src /tmp/rtn/src2; cd /tmp/rtn; "
        "printf 'one\\ntwo\\nthree\\n' > src/common.txt; "
        "printf 'one\\nTWO\\nthree\\n' > src2/common.txt; "
        "printf 'only-in-src2\\n' > src2/newfile.txt; "
        "diff -Naur src src2 > tree.patch; "
        "mkdir -p target; cp src/common.txt target/; cd target; "
        "patch -p1 < ../tree.patch >/dev/null; cd ..; "
        "diff -rq target src2 && echo ROUNDTRIP_OK",
    ),
    (
        "patch: compound command preserves piped stdin",
        "printf 'x\\n' | { mkdir -p /tmp/compoundq; cat; }",
    ),
    (
        "diff: large files that differ throughout",
        "seq 30000 >/tmp/a; seq 2 30001 >/tmp/b; diff /tmp/a /tmp/b; echo status=$?",
    ),
    (
        "diff: -e and -y on files this large are refused",
        "seq 30000 >/tmp/a; seq 2 30001 >/tmp/b; diff -e /tmp/a /tmp/b | wc -c; echo status=${PIPESTATUS[0]}; "
        "diff -y -W 20 /tmp/a /tmp/b | wc -l; echo status=${PIPESTATUS[0]}",
    ),
]

# `diffutilslib::side_by_side_diff` is a best-effort port (its own doc comment disclaims exact
# GNU fidelity): a changed line that GNU shows as one `|`-joined row comes out here as a
# deleted-then-inserted pair of rows. Captured from an actual WASM run rather than the oracle.
EXPECTED = {
    "diff: side by side": (
        1,
        b"one\t\t\t\t\t\t\t\tone\ntwo\t\t\t\t\t\t\t      <\n\t\t\t\t\t\t\t      >\tTWO\nthree\t\t\t\t\t\t\t\tthree\n\t\t\t\t\t\t\t      >\tfour\n",
        b"",
        "diffutils' side-by-side port shows a changed line as a deleted-then-inserted pair",
    ),
    # `-B`/`--ignore-blank-lines` alone stays refused (see the module note in
    # shell/src/tools/diff.rs); GNU would instead ignore lines that are entirely blank.
    "diff: ignore blank lines still refused": (
        0,
        b"status=2\n",
        b"diff: -B is unsupported in bash-tool\n",
        "bash-tool refuses -B alone; GNU ignores wholly blank lines",
    ),
    "diff: -e and -y on files this large are refused": (
        0,
        b"0\nstatus=2\n0\nstatus=2\n",
        b"diff: -e on files this large is unsupported in bash-tool\n"
        b"diff: -y on files this large is unsupported in bash-tool\n",
        "the engine behind -e and -y needs memory for every pair of differing lines; past about "
        "4,000 a side bash-tool refuses instead of running out of memory",
    ),
}

EXPECTED_STDERR = {}
