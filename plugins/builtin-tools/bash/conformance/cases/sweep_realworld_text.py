"""Real-world usage: tldr-pages examples for the text-processing commands, over many input shapes.

Source: tldr-pages (https://github.com/tldr-pages/tldr, CC BY 4.0), pages/common and pages/linux
at commit 8cf22035e60b167ebf224c1c527b27e0b5dca24d. Each example's {{placeholders}} are bound to
fixture files the script creates first; see ../NOTICE for the attribution and what was changed.

Every example runs once in the form tldr shows it, once per extra invocation form it supports
(a file operand, `-` on a pipe, input redirection) and once per input shape: an empty file, no
final newline, CRLF line ends, multibyte UTF-8, blank and whitespace-only lines, an embedded NUL,
a missing file and a directory operand. Long option spellings run once
each. Everything is generated with plain loops, so the scripts are the same on any Python 3.
"""

TIER = "sweep"


def _q(text):
    return "'" + text.replace("'", "'\\''") + "'"


def _pf(path, data):
    esc = (
        data.replace("\\", "\\\\").replace("%", "%%").replace("\n", "\\n")
        .replace("\t", "\\t").replace("\r", "\\r").replace("\0", "\\000")
    )
    return "printf " + _q(esc) + " >" + path


# -- contents -------------------------------------------------------------------------------------

C = {
    "words": "apple pie\nbanana split\ncherry tart\napple pie\norange juice\nbanana split\ndate loaf\n",
    "fruit": "apple and orange\norange then apple\npear\napple apple apple\n",
    "lines15": "".join("line %d\n" % i for i in range(1, 16)),
    "tsv": "name\tqty\tprice\tunit\tcolor\tsize\tnote\nbolt\t10\t0.25\tpcs\tgrey\tM6\tzinc\nnut\t200\t0.05\tpcs\tsilver\tM6\tsteel\n",
    "colon": "app:x:1001:1001:1.5e2:/srv/app:/bin/sh\nweb:x:33:33:2e1:/var/www:/bin/false\nci:x:1001:1000:9:/srv/ci:/bin/sh\ndb:x:70:70:1e3:/var/db:/bin/false\n",
    "spaced": "one two three four five\nalpha beta gamma\nsingle\n",
    "nums": "10\n2\n33\n4\n-5\n2.5\n0\n",
    "mixed": "banana\nApple\ncherry\napple\nBanana\n",
    "prose": "The quick brown fox jumps over the lazy dog, and the quick brown fox jumps over the lazy dog again until the line is long.\n\nShort line.\nAnother short line here.\n",
    "tabbed": "a\tb\tc\n\tindented\n  spaced\tmixed\n1234567\t8\n",
    "blanks": "        eight spaces\n    four  and  more    words\n                sixteen\n",
    "foobar": "FooBar1 first\nplain\nFooBar2 second\n\nFooBarX no\n",
    "bin": "ABCDEFGHIJKLMNOPABCDEFGHIJKLMNOPABCDEFGHIJKLMNOP0123\n",
    "sorted1": "apple\nbanana\ncherry\ndate\n",
    "sorted2": "banana\ndate\nelder\nfig\n",
    "join1": "1 alice\n2 bob\n3 carol\n5 eve\n",
    "join2": "1 paris\n2 lima\n4 rome\n5 oslo\n",
    "joincsv1": "1,alice\n2,bob\n3,carol\n",
    "joincsv2": "1,paris\n3,oslo\n4,rome\n",
    "join3": "x y 1\nx z 2\nw v 5\n",
    "tsort": "shirt tie\ntie jacket\nbelt jacket\npants shoes\npants belt\n",
    "csv": "a,b,c,d\n",
    "b64": "aGVsbG8gd29ybGQK\n",
    "b32": "NBSWY3DPEB3W64TMMQFA====\n",
    "slashes": "path////to////file\nno slashes\n",
    "blanklines": "keep\n\n   \n\t\nalso keep\n",
    "path": "/srv/app/src/main.rs\n/srv/app/README\n",
    "colors": "red:1\ngreen:2\nblue\nred:3\n",
    "wide": "abcdefghijklmnopqrstuvwxyz\n0123456789\n",
    "long": "x" * 100 + "\nshort\n",
    "dups": "aaaaaaaaaa-one\naaaaaaaaaa-two\nbbbbb-same\nccccc-same\nzz\nzz\n",
    "count": "a\nb\na\nc\na\nb\n",
    "big": "".join("%03d abcdefghij\n" % i for i in range(1, 50)),
    "lines30": "".join("row %d\n" % i for i in range(1, 31)),
    "sections": "header\nalpha\nbeta\n--\ngamma\ndelta\n--\nomega\n",
    "print": "tab\there\x07bell\x1b[1mesc\nplain\n",
}
SECOND = {
    "words": "zebra\napple pie\n",
    "sorted2": C["sorted2"],
    "join2": C["join2"],
    "joincsv2": C["joincsv2"],
    "lines15": "tail a\ntail b\n",
    "other": "other one\nother two\n",
    "fruit": "orange\nlime\n",
}


def shape(content, name):
    if name == "empty":
        return ""
    if name == "nonl":
        return content[:-1] if content.endswith("\n") else content
    if name == "crlf":
        return content.replace("\n", "\r\n")
    if name == "utf8":
        return "naïve café ✓ 日本語\n" + content + "Ωmega ünï 🙂\n"
    if name == "ws":
        lines = content.split("\n")
        body = lines[:-1] if content.endswith("\n") else lines
        if not body:
            return "\n  \t  \n"
        out = [body[0], "", "  \t  "] + body[1:-1] + ["  " + body[-1] + "  "] if len(body) > 1 else [body[0], "", "  \t  "]
        return "\n".join(out) + "\n"
    if name == "nul":
        return content[:3] + "\0" + content[3:]
    return content


CONTENT_SHAPES = ["empty", "nonl", "crlf", "utf8", "ws", "nul"]
ERROR_SHAPES = ["missing", "dir"]
ALL = CONTENT_SHAPES + ERROR_SHAPES
NOERR = CONTENT_SHAPES
NONE = []

# (command, slug, template, content, second-file content key or None, forms, shapes, extra setup)
# Forms: f = file operand, p = `cat F | cmd` (F dropped), d = `cat F | cmd -`, r = `cmd < F`.
# The first form is the one tldr shows; the rest run once each with the plain content.
EXAMPLES = [
    # cat
    ("cat", "print a file", "cat {F}", "words", None, "fpdr", ALL, ""),
    ("cat", "concatenate into an output file", "cat {F} other.txt > out.txt && cat out.txt", "words", "other", "f", ALL, ""),
    ("cat", "append several files to an output file", "cat {F} other.txt >> out.txt && cat out.txt", "words", "other", "f", ALL, "printf 'old\\n' >out.txt"),
    ("cat", "copy without buffering", "cat -u {F} > out.txt && cat out.txt", "words", None, "fd", NOERR, ""),
    ("cat", "write stdin to a file", "cat - > out.txt < {F} && cat out.txt", "words", None, "f", NOERR, ""),
    ("cat", "number all output lines", "cat -n {F}", "words", None, "fpd", ALL, ""),
    ("cat", "number all output lines (long)", "cat --number {F}", "words", None, "f", NONE, ""),
    ("cat", "show all characters", "cat -A {F}", "print", None, "fp", ALL, ""),
    ("cat", "show all characters (long)", "cat --show-all {F}", "print", None, "f", NONE, ""),
    ("cat", "pass contents to another program", "cat {F} | sort", "words", None, "f", ALL, ""),
    # head
    ("head", "first few lines", "head -n 3 {F}", "lines15", None, "fprd", ALL, ""),
    ("head", "first 10 lines", "head {F}", "lines15", None, "fpr", ALL, ""),
    ("head", "first 10 lines of multiple files", "head {F} other.txt", "lines15", "lines15", "fd", ALL, ""),
    ("head", "first 5 lines", "head -5 {F}", "lines15", None, "fp", ALL, ""),
    ("head", "first 5 lines (long)", "head --lines 5 {F}", "lines15", None, "f", NONE, ""),
    ("head", "first few lines (long)", "head --lines 3 {F}", "lines15", None, "f", NONE, ""),
    ("head", "first few bytes", "head -c 7 {F}", "lines15", None, "fp", ALL, ""),
    ("head", "first few bytes (long)", "head --bytes 7 {F}", "lines15", None, "f", NONE, ""),
    ("head", "all but the last few lines", "head -n -2 {F}", "lines15", None, "fp", ALL, ""),
    ("head", "all but the last few lines (long)", "head --lines -2 {F}", "lines15", None, "f", NONE, ""),
    ("head", "all but the last few bytes", "head -c -4 {F}", "lines15", None, "fp", ALL, ""),
    ("head", "all but the last few bytes (long)", "head --bytes -4 {F}", "lines15", None, "f", NONE, ""),
    # tail
    ("tail", "last 10 lines", "tail {F}", "lines15", None, "fprd", ALL, ""),
    ("tail", "last 10 lines of multiple files", "tail {F} other.txt", "lines15", "lines15", "fd", ALL, ""),
    ("tail", "last 5 lines", "tail -5 {F}", "lines15", None, "fp", ALL, ""),
    ("tail", "last 5 lines (long)", "tail --lines 5 {F}", "lines15", None, "f", NONE, ""),
    ("tail", "from a line number", "tail -n +3 {F}", "lines15", None, "fp", ALL, ""),
    ("tail", "from a line number (long)", "tail --lines +3 {F}", "lines15", None, "f", NONE, ""),
    ("tail", "last bytes", "tail -c 7 {F}", "lines15", None, "fp", ALL, ""),
    ("tail", "last bytes (long)", "tail --bytes 7 {F}", "lines15", None, "f", NONE, ""),
    # cut
    ("cut", "fifth character", "cut -c 5 {F}", "words", None, "pfr", ALL, ""),
    ("cut", "fifth character (long)", "cut --characters 5 {F}", "words", None, "p", NONE, ""),
    ("cut", "fifth to tenth character", "cut -c 5-10 {F}", "words", None, "fp", ALL, ""),
    ("cut", "fifth to tenth character (long)", "cut --characters 5-10 {F}", "words", None, "f", NONE, ""),
    ("cut", "fields two and six", "cut -f 2,6 {F}", "tsv", None, "fp", ALL, ""),
    ("cut", "fields two and six (long)", "cut --fields 2,6 {F}", "tsv", None, "f", NONE, ""),
    ("cut", "second field onward", "cut -d \":\" -f 2- {F}", "colon", None, "pf", ALL, ""),
    ("cut", "second field onward (long)", "cut --delimiter \":\" --fields 2- {F}", "colon", None, "p", NONE, ""),
    ("cut", "first three space-separated fields", "cut -d \" \" -f -3 {F}", "spaced", None, "pf", ALL, ""),
    ("cut", "first three space-separated fields (long)", "cut --delimiter \" \" --fields -3 {F}", "spaced", None, "p", NONE, ""),
    ("cut", "only delimited lines", "cut -d \":\" -f 1 -s {F}", "colors", None, "pf", ALL, ""),
    ("cut", "only delimited lines (long)", "cut --delimiter \":\" --fields 1 --only-delimited {F}", "colors", None, "p", NONE, ""),
    # sort
    ("sort", "ascending", "sort {F}", "words", None, "fprd", ALL, ""),
    ("sort", "descending", "sort -r {F}", "words", None, "fp", ALL, ""),
    ("sort", "descending (long)", "sort --reverse {F}", "words", None, "f", NONE, ""),
    ("sort", "case-insensitive", "sort -f {F}", "mixed", None, "fp", ALL, ""),
    ("sort", "case-insensitive (long)", "sort --ignore-case {F}", "mixed", None, "f", NONE, ""),
    ("sort", "numeric", "sort -n {F}", "nums", None, "fp", ALL, ""),
    ("sort", "numeric (long)", "sort --numeric-sort {F}", "nums", None, "f", NONE, ""),
    ("sort", "third field onward numerically", "sort -t : -k 3n {F}", "colon", None, "fp", ALL, ""),
    ("sort", "third field onward numerically (long)", "sort --field-separator : --key 3n {F}", "colon", None, "f", NONE, ""),
    ("sort", "third then fourth field with exponents", "sort -t : -k 3,3n -k 5,5g {F}", "colon", None, "fp", ALL, ""),
    ("sort", "third then fourth field with exponents (long)", "sort --field-separator : --key 3,3n --key 5,5g {F}", "colon", None, "f", NONE, ""),
    ("sort", "unique lines", "sort -u {F}", "words", None, "fp", ALL, ""),
    ("sort", "unique lines (long)", "sort --unique {F}", "words", None, "f", NONE, ""),
    ("sort", "output to a file in place", "sort -o {F} {F} && cat {F}", "words", None, "f", ALL, ""),
    ("sort", "output to a file (long)", "sort --output out.txt {F} && cat out.txt", "words", None, "f", NONE, ""),
    # uniq
    ("uniq", "each line once", "sort {F} | uniq", "words", None, "f", ALL, ""),
    ("uniq", "only unique lines", "sort {F} | uniq -u", "words", None, "f", ALL, ""),
    ("uniq", "only unique lines (long)", "sort {F} | uniq --unique", "words", None, "f", NONE, ""),
    ("uniq", "only duplicate lines", "sort {F} | uniq -d", "words", None, "f", ALL, ""),
    ("uniq", "only duplicate lines (long)", "sort {F} | uniq --repeated", "words", None, "f", NONE, ""),
    ("uniq", "count occurrences", "sort {F} | uniq -c", "count", None, "f", ALL, ""),
    ("uniq", "count occurrences (long)", "sort {F} | uniq --count", "count", None, "f", NONE, ""),
    ("uniq", "most frequent first", "sort {F} | uniq -c | sort -nr", "count", None, "f", ALL, ""),
    ("uniq", "most frequent first (long)", "sort {F} | uniq --count | sort --numeric-sort --reverse", "count", None, "f", NONE, ""),
    ("uniq", "compare first 10 characters", "sort {F} | uniq -w 10", "dups", None, "f", ALL, ""),
    ("uniq", "compare first 10 characters (long)", "sort {F} | uniq --check-chars 10", "dups", None, "f", NONE, ""),
    ("uniq", "skip the first 5 characters", "sort {F} | uniq -s 5", "dups", None, "f", ALL, ""),
    ("uniq", "skip the first 5 characters (long)", "sort {F} | uniq --skip-chars 5", "dups", None, "f", NONE, ""),
    # wc
    ("wc", "lines", "wc -l {F}", "words", None, "fprd", ALL, ""),
    ("wc", "lines (long)", "wc --lines {F}", "words", None, "f", NONE, ""),
    ("wc", "words", "wc -w {F}", "words", None, "fp", ALL, ""),
    ("wc", "words (long)", "wc --words {F}", "words", None, "f", NONE, ""),
    ("wc", "bytes", "wc -c {F}", "words", None, "fp", ALL, ""),
    ("wc", "bytes (long)", "wc --bytes {F}", "words", None, "f", NONE, ""),
    ("wc", "characters", "wc -m {F}", "words", None, "fp", ALL, ""),
    ("wc", "characters (long)", "wc --chars {F}", "words", None, "f", NONE, ""),
    ("wc", "lines words and bytes", "wc {F}", "words", None, "pf", ALL, ""),
    ("wc", "longest line", "wc -L {F}", "words", None, "fp", ALL, ""),
    ("wc", "longest line (long)", "wc --max-line-length {F}", "words", None, "f", NONE, ""),
    # tr
    ("tr", "replace a character in a file", "tr < {F} a A", "words", None, "f", ALL, ""),
    ("tr", "map a set of characters", "tr 'abcd' 'jkmn' {F}", "words", None, "rp", ALL, ""),
    ("tr", "delete characters", "tr -d 'aeiou' {F}", "words", None, "rp", ALL, ""),
    ("tr", "delete characters (long)", "tr --delete 'aeiou' {F}", "words", None, "r", NONE, ""),
    ("tr", "squeeze repeats", "tr -s 'p' {F}", "words", None, "rp", ALL, ""),
    ("tr", "squeeze repeats (long)", "tr --squeeze-repeats 'p' {F}", "words", None, "r", NONE, ""),
    ("tr", "to upper case", "tr \"[:lower:]\" \"[:upper:]\" {F}", "words", None, "rp", ALL, ""),
    ("tr", "strip non-printable characters", "tr -cd \"[:print:]\" {F}", "print", None, "rp", ALL, ""),
    ("tr", "strip non-printable characters (long)", "tr --complement --delete \"[:print:]\" {F}", "print", None, "r", NONE, ""),
    # sed
    ("sed", "replace all occurrences", "sed 's/apple/mango/g' {F}", "fruit", None, "pfrd", ALL, ""),
    ("sed", "run a script file", "sed -f script.sed {F}", "fruit", None, "pf", ALL, "printf 's/apple/mango/g\\n/pear/d\\n' >script.sed"),
    ("sed", "print just the first line", "sed -n '1p' {F}", "fruit", None, "pf", ALL, ""),
    ("sed", "print just the first line (long)", "sed --quiet '1p' {F}", "fruit", None, "p", NONE, ""),
    ("sed", "replace in place", "sed -i 's/apple/mango/g' {F} && cat {F}", "fruit", None, "f", ALL, ""),
    ("sed", "replace in place (long)", "sed --in-place 's/apple/mango/g' {F} && cat {F}", "fruit", None, "f", NONE, ""),
    ("sed", "multiple substitutions", "sed -e 's/apple/mango/g' -e 's/orange/lime/g' {F}", "fruit", None, "pf", ALL, ""),
    ("sed", "custom delimiter", "sed 's#////#____#g' {F}", "slashes", None, "pf", ALL, ""),
    ("sed", "delete lines with a backup", "sed -i.orig '1,5d' {F} && cat {F} && cat {F}.orig", "lines15", None, "f", ALL, ""),
    ("sed", "delete lines with a backup (long)", "sed --in-place=.orig '1,5d' {F} && cat {F} && cat {F}.orig", "lines15", None, "f", NONE, ""),
    ("sed", "insert a line at the beginning", "sed -i '1i\\your new line text\\' {F} && cat {F}", "fruit", None, "f", ALL, ""),
    ("sed", "insert a line at the beginning (long)", "sed --in-place '1i\\your new line text\\' {F} && cat {F}", "fruit", None, "f", NONE, ""),
    ("sed", "delete blank lines in place", "sed -i '/^[[:space:]]*$/d' {F} && cat {F}", "blanklines", None, "f", ALL, ""),
    ("sed", "delete blank lines in place (long)", "sed --in-place '/^[[:space:]]*$/d' {F} && cat {F}", "blanklines", None, "f", NONE, ""),
    # grep
    ("grep", "search several files", "grep \"apple\" {F} other.txt", "words", "words", "fd", ALL, ""),
    ("grep", "exact string", "grep -F \"e p\" {F}", "words", None, "fp", ALL, ""),
    ("grep", "exact string (long)", "grep --fixed-strings \"e p\" {F}", "words", None, "f", NONE, ""),
    ("grep", "context around matches", "grep --context 1 \"cherry\" {F}", "words", None, "fp", ALL, ""),
    ("grep", "context before matches", "grep --before-context 1 \"cherry\" {F}", "words", None, "fp", ALL, ""),
    ("grep", "context after matches", "grep --after-context 1 \"cherry\" {F}", "words", None, "fp", ALL, ""),
    ("grep", "file name and line number in color", "grep -Hn --color=always \"an\" {F}", "words", None, "fd", ALL, ""),
    ("grep", "file name and line number in color (long)", "grep --with-filename --line-number --color=always \"an\" {F}", "words", None, "f", NONE, ""),
    ("grep", "only the matched text", "grep -o \"an\" {F}", "words", None, "fp", ALL, ""),
    ("grep", "only the matched text (long)", "grep --only-matching \"an\" {F}", "words", None, "f", NONE, ""),
    ("grep", "invert from stdin", "grep -v \"apple\" {F}", "words", None, "pfr", ALL, ""),
    ("grep", "invert from stdin (long)", "grep --invert-match \"apple\" {F}", "words", None, "p", NONE, ""),
    ("grep", "extended case-insensitive", "grep -Ei \"^(APPLE|date)\" {F}", "words", None, "fp", ALL, ""),
    ("grep", "extended case-insensitive (long)", "grep --extended-regexp --ignore-case \"^(APPLE|date)\" {F}", "words", None, "f", NONE, ""),
    # nl
    ("nl", "number non-blank lines", "nl {F}", "foobar", None, "fpr", ALL, ""),
    ("nl", "read from stdin with a dash", "nl - {F}", "foobar", None, "p", NONE, ""),
    ("nl", "number all body lines", "nl -b a {F}", "foobar", None, "fp", ALL, ""),
    ("nl", "number no body lines", "nl -b n {F}", "foobar", None, "fp", ALL, ""),
    ("nl", "number all body lines (long)", "nl --body-numbering a {F}", "foobar", None, "f", NONE, ""),
    ("nl", "number no body lines (long)", "nl --body-numbering n {F}", "foobar", None, "f", NONE, ""),
    ("nl", "number lines matching a pattern", "nl -b p'FooBar[0-9]' {F}", "foobar", None, "fp", ALL, ""),
    ("nl", "number lines matching a pattern (long)", "nl --body-numbering p'FooBar[0-9]' {F}", "foobar", None, "f", NONE, ""),
    ("nl", "increment", "nl -i 5 {F}", "foobar", None, "fp", ALL, ""),
    ("nl", "increment (long)", "nl --line-increment 5 {F}", "foobar", None, "f", NONE, ""),
    ("nl", "right-justified with zeros", "nl -n rz {F}", "foobar", None, "rf", ALL, ""),
    ("nl", "left-justified", "nl -n ln {F}", "foobar", None, "rf", ALL, ""),
    ("nl", "right-justified", "nl -n rn {F}", "foobar", None, "rf", ALL, ""),
    ("nl", "number format (long)", "nl --number-format rz {F}", "foobar", None, "r", NONE, ""),
    ("nl", "width", "nl -w 3 {F}", "foobar", None, "fp", ALL, ""),
    ("nl", "width (long)", "nl --number-width 3 {F}", "foobar", None, "f", NONE, ""),
    ("nl", "separator", "nl -s ': ' {F}", "foobar", None, "fp", ALL, ""),
    ("nl", "separator (long)", "nl --number-separator ': ' {F}", "foobar", None, "f", NONE, ""),
    # fold
    ("fold", "default width", "fold {F}", "long", None, "fpr", ALL, ""),
    ("fold", "width 30", "fold -w30 {F}", "prose", None, "fp", ALL, ""),
    ("fold", "width 5 at spaces", "fold -w5 -s {F}", "spaced", None, "fp", ALL, ""),
    ("fold", "fixed width", "fold -w 10 {F}", "wide", None, "fp", ALL, ""),
    ("fold", "fixed width (long)", "fold --width 10 {F}", "wide", None, "f", NONE, ""),
    ("fold", "width in bytes", "fold -b -w 10 {F}", "wide", None, "fp", ALL, ""),
    ("fold", "width in bytes (long)", "fold --bytes --width 10 {F}", "wide", None, "f", NONE, ""),
    ("fold", "break at blanks", "fold -s -w 12 {F}", "prose", None, "fp", ALL, ""),
    ("fold", "break at blanks (long)", "fold --spaces --width 12 {F}", "prose", None, "f", NONE, ""),
    # fmt
    ("fmt", "reformat", "fmt {F}", "prose", None, "fpr", ALL, ""),
    ("fmt", "width", "fmt -w 20 {F}", "prose", None, "fp", ALL, ""),
    ("fmt", "width (long)", "fmt --width 20 {F}", "prose", None, "f", NONE, ""),
    ("fmt", "split only", "fmt -s {F}", "prose", None, "fp", ALL, ""),
    ("fmt", "split only (long)", "fmt --split-only {F}", "prose", None, "f", NONE, ""),
    ("fmt", "uniform spacing", "fmt -u {F}", "blanks", None, "fp", ALL, ""),
    ("fmt", "uniform spacing (long)", "fmt --uniform-spacing {F}", "blanks", None, "f", NONE, ""),
    # expand
    ("expand", "tabs to spaces", "expand {F}", "tabbed", None, "fr", ALL, ""),
    ("expand", "from stdin", "expand {F}", "tabbed", None, "p", NONE, ""),
    ("expand", "initial tabs only", "expand -i {F}", "tabbed", None, "fp", ALL, ""),
    ("expand", "initial tabs only (long)", "expand --initial {F}", "tabbed", None, "f", NONE, ""),
    ("expand", "tab stops every 4", "expand -t 4 {F}", "tabbed", None, "fp", ALL, ""),
    ("expand", "tab stops every 4 (long)", "expand --tabs 4 {F}", "tabbed", None, "f", NONE, ""),
    ("expand", "explicit tab positions", "expand -t 1,4,6 {F}", "tabbed", None, "pf", ALL, ""),
    ("expand", "explicit tab positions (long)", "expand --tabs 1,4,6 {F}", "tabbed", None, "p", NONE, ""),
    # unexpand
    ("unexpand", "blanks to tabs", "unexpand {F}", "blanks", None, "fr", ALL, ""),
    ("unexpand", "from stdin", "unexpand {F}", "blanks", None, "p", NONE, ""),
    ("unexpand", "all blanks", "unexpand -a {F}", "blanks", None, "fp", ALL, ""),
    ("unexpand", "all blanks (long)", "unexpand --all {F}", "blanks", None, "f", NONE, ""),
    ("unexpand", "leading blanks only", "unexpand --first-only {F}", "blanks", None, "fp", ALL, ""),
    ("unexpand", "tab stops every 4", "unexpand -t 4 {F}", "blanks", None, "fp", ALL, ""),
    ("unexpand", "tab stops every 4 (long)", "unexpand --tabs 4 {F}", "blanks", None, "f", NONE, ""),
    # paste
    ("paste", "join lines with tabs", "paste -s {F}", "sorted1", None, "fpd", ALL, ""),
    ("paste", "join lines with tabs (long)", "paste --serial {F}", "sorted1", None, "f", NONE, ""),
    ("paste", "join lines with a delimiter", "paste -s -d , {F}", "sorted1", None, "fd", ALL, ""),
    ("paste", "join lines with a delimiter (long)", "paste --serial --delimiters , {F}", "sorted1", None, "f", NONE, ""),
    ("paste", "merge side by side", "paste {F} other.txt", "sorted1", "sorted2", "fd", ALL, ""),
    ("paste", "merge with a delimiter", "paste -d , {F} other.txt", "sorted1", "sorted2", "fd", ALL, ""),
    ("paste", "merge with a delimiter (long)", "paste --delimiters , {F} other.txt", "sorted1", "sorted2", "f", NONE, ""),
    ("paste", "alternate lines", "paste -d '\\n' {F} other.txt", "sorted1", "sorted2", "fd", ALL, ""),
    # rev
    ("rev", "reverse stdin", "rev {F}", "words", None, "rp", ALL, ""),
    ("rev", "reverse a file", "rev {F}", "words", None, "f", ALL, ""),
    # tac
    ("tac", "files in reverse", "tac {F} other.txt", "words", "words", "fd", ALL, ""),
    ("tac", "stdin in reverse", "tac {F}", "words", None, "pr", ALL, ""),
    ("tac", "separator", "tac -s , {F}", "csv", None, "fp", ALL, ""),
    ("tac", "separator (long)", "tac --separator , {F}", "csv", None, "f", NONE, ""),
    ("tac", "separator before", "tac -b {F}", "words", None, "fp", ALL, ""),
    ("tac", "separator before (long)", "tac --before {F}", "words", None, "f", NONE, ""),
    # od
    ("od", "default settings", "od {F}", "bin", None, "fpr", ALL, ""),
    ("od", "verbose", "od -v {F}", "bin", None, "fp", ALL, ""),
    ("od", "verbose (long)", "od --output-duplicates {F}", "bin", None, "f", NONE, ""),
    ("od", "hex with decimal offsets", "od -t x -A d -v {F}", "bin", None, "fp", ALL, ""),
    ("od", "hex with decimal offsets (long)", "od --format x --address-radix d --output-duplicates {F}", "bin", None, "f", NONE, ""),
    ("od", "hex bytes 4 per line", "od -t x1 -w4 -v {F}", "wide", None, "fp", ALL, ""),
    ("od", "hex bytes 4 per line (long)", "od --format x1 --width=4 --output-duplicates {F}", "wide", None, "f", NONE, ""),
    ("od", "hex with characters and no offsets", "od -t xz -A n -v {F}", "wide", None, "fp", ALL, ""),
    ("od", "hex with characters and no offsets (long)", "od --format xz --address-radix n --output-duplicates {F}", "wide", None, "f", NONE, ""),
    ("od", "read bytes from an offset", "od -N 20 -j 500 -v {F}", "big", None, "fp", ALL, ""),
    ("od", "read bytes from an offset (long)", "od --read-bytes 20 --skip-bytes 500 --output-duplicates {F}", "big", None, "f", NONE, ""),
    # comm
    ("comm", "three columns", "comm {F} other.txt", "sorted1", "sorted2", "fd", ALL, ""),
    ("comm", "common lines", "comm -12 {F} other.txt", "sorted1", "sorted2", "f", ALL, ""),
    ("comm", "common lines with one file from stdin", "comm -12 {F} other.txt", "sorted1", "sorted2", "d", NONE, ""),
    ("comm", "first file only into a file", "comm -23 {F} other.txt > only.txt && cat only.txt", "sorted1", "sorted2", "f", ALL, ""),
    ("comm", "second file only of unsorted files", "comm -13 <(sort {F}) <(sort other.txt)", "words", "words", "f", ALL, ""),
    # join
    ("join", "first field", "join {F} other.txt", "join1", "join2", "fd", ALL, ""),
    ("join", "comma separator", "join -t ',' {F} other.txt", "joincsv1", "joincsv2", "fd", ALL, ""),
    ("join", "third field with first field", "join -1 3 -2 1 {F} other.txt", "join3", "join2", "f", ALL, ""),
    ("join", "unpairable lines of the first file", "join -a 1 {F} other.txt", "join1", "join2", "f", ALL, ""),
    ("join", "first file from stdin", "join {F} other.txt", "join1", "join2", "d", NONE, ""),
    # tsort
    ("tsort", "partial orders", "tsort {F}", "tsort", None, "fpr", ALL, ""),
    ("tsort", "strings from echo -e", "echo -e \"UI Backend\\nBackend Database\\nDocs UI\" | tsort", None, None, "", NONE, ""),
    # shuf
    ("shuf", "randomize lines", "shuf {F} | sort", "words", None, "fd", ALL, ""),
    ("shuf", "first 5 entries", "shuf -n 5 {F} | wc -l", "words", None, "f", ALL, ""),
    ("shuf", "first 5 entries (long)", "shuf --head-count 5 {F} | wc -l", "words", None, "f", NONE, ""),
    ("shuf", "write to a file", "shuf {F} -o out.txt && sort out.txt", "words", None, "f", ALL, ""),
    ("shuf", "write to a file (long)", "shuf {F} --output out.txt && sort out.txt", "words", None, "f", NONE, ""),
    # split
    ("split", "by lines", "split -l 4 {F} && head x*", "lines15", None, "f", ALL, ""),
    ("split", "by lines (long)", "split --lines 4 {F} && head x*", "lines15", None, "f", NONE, ""),
    ("split", "into 5 files", "split -n 5 {F} && wc -c x*", "lines15", None, "f", ALL, ""),
    ("split", "into 5 files (long)", "split --number 5 {F} && wc -c x*", "lines15", None, "f", NONE, ""),
    ("split", "by bytes", "split -b 16 {F} && wc -c x*", "lines15", None, "f", ALL, ""),
    ("split", "by bytes (long)", "split --bytes 16 {F} && wc -c x*", "lines15", None, "f", NONE, ""),
    ("split", "by bytes without breaking lines", "split -C 16 {F} && wc -c x*", "lines15", None, "f", ALL, ""),
    ("split", "by bytes without breaking lines (long)", "split --line-bytes 16 {F} && wc -c x*", "lines15", None, "f", NONE, ""),
    ("split", "from stdin with a prefix", "cat {F} | split -l 6 - part_ && wc -l part_*", "lines15", None, "f", ALL, ""),
    ("split", "from stdin with a prefix (long)", "cat {F} | split --lines 6 - part_ && wc -l part_*", "lines15", None, "f", NONE, ""),
    # csplit
    ("csplit", "in two at line 10", "csplit {F} 10 && wc -l xx*", "lines15", None, "f", ALL, ""),
    ("csplit", "in three at lines 7 and 12", "csplit {F} 7 12 && wc -l xx*", "lines15", None, "f", ALL, ""),
    ("csplit", "every 5th line", "csplit {F} 5 '{*}'; echo \"status=$?\"; ls", "lines15", None, "f", ALL, ""),
    ("csplit", "every 5th line keeping files", "csplit -k {F} 4 '{*}'; echo \"status=$?\"; wc -l xx*", "lines15", None, "f", ALL, ""),
    ("csplit", "every 5th line keeping files (long)", "csplit --keep-files {F} 4 '{*}'; echo \"status=$?\"; wc -l xx*", "lines15", None, "f", NONE, ""),
    ("csplit", "custom prefix", "csplit {F} 5 -f part && wc -l part*", "lines15", None, "f", ALL, ""),
    ("csplit", "custom prefix (long)", "csplit {F} 5 --prefix part && wc -l part*", "lines15", None, "f", NONE, ""),
    ("csplit", "above a regex match", "csplit {F} /--/ && cat xx00", "sections", None, "f", ALL, ""),
    # base64 / base32 / basenc
    ("base64", "encode a file", "base64 {F}", "words", None, "fprd", ALL, ""),
    ("base64", "no wrapping", "base64 -w 0 {F}", "words", None, "fp", ALL, ""),
    ("base64", "wrap at 20", "base64 -w 20 {F}", "words", None, "fp", ALL, ""),
    ("base64", "no wrapping (long)", "base64 --wrap 0 {F}", "words", None, "f", NONE, ""),
    ("base64", "decode a file", "base64 -d {F}", "b64", None, "fp", ALL, ""),
    ("base64", "decode a file (long)", "base64 --decode {F}", "b64", None, "f", NONE, ""),
    ("base32", "encode a file", "base32 {F}", "words", None, "fprd", ALL, ""),
    ("base32", "no wrapping", "base32 -w 0 {F}", "words", None, "fp", ALL, ""),
    ("base32", "wrap at 20", "base32 -w 20 {F}", "words", None, "fp", ALL, ""),
    ("base32", "no wrapping (long)", "base32 --wrap 0 {F}", "words", None, "f", NONE, ""),
    ("base32", "decode a file", "base32 -d {F}", "b32", None, "fp", ALL, ""),
    ("base32", "decode a file (long)", "base32 --decode {F}", "b32", None, "f", NONE, ""),
    ("basenc", "base64 encode", "basenc --base64 {F}", "words", None, "fp", ALL, ""),
    ("basenc", "base64 decode", "basenc -d --base64 {F}", "b64", None, "fp", ALL, ""),
    ("basenc", "base64 decode (long)", "basenc --decode --base64 {F}", "b64", None, "f", NONE, ""),
    ("basenc", "base32 with 42 columns", "basenc --base32 -w 42 {F}", "words", None, "pf", ALL, ""),
    ("basenc", "base32 with 42 columns (long)", "basenc --base32 --wrap 42 {F}", "words", None, "p", NONE, ""),
    ("basenc", "base32", "basenc --base32 {F}", "words", None, "pf", ALL, ""),
    # cksum
    ("cksum", "checksum size and name", "cksum {F}", "words", None, "fpd", ALL, ""),
]

# The checksum commands share tldr's seven examples.
for _tool in ("b2sum", "md5sum", "sha1sum", "sha256sum", "sha512sum"):
    EXAMPLES += [
        (_tool, "one or more files", _tool + " {F} other.txt", "words", "other", "fd", ALL, ""),
        (_tool, "save the list to a file", _tool + " {F} other.txt > sums && cat sums", "words", "other", "f", ALL, ""),
        (_tool, "from stdin", _tool + " {F}", "words", None, "pr", ALL, ""),
        (_tool, "verify a list", _tool + " -c sums", "words", "other", "f", ALL, _tool + " {F} other.txt > sums; printf 'changed\\n' >other.txt"),
        (_tool, "verify a list (long)", _tool + " --check sums", "words", "other", "f", NONE, _tool + " {F} other.txt > sums"),
        (_tool, "quiet verification", _tool + " -c --quiet sums", "words", "other", "f", ALL, _tool + " {F} other.txt > sums; printf 'changed\\n' >other.txt"),
        (_tool, "ignore missing files", _tool + " --ignore-missing -c --quiet sums", "words", "other", "f", ALL, _tool + " {F} other.txt > sums; rm other.txt"),
        (_tool, "check a known checksum", "echo $(" + _tool + " < {F} | cut -d ' ' -f 1) {F} | " + _tool + " -c", "words", None, "f", ALL, ""),
    ]


def _render(template, form, name):
    if form == "f":
        return template.replace("{F}", name)
    if form == "p":
        return "cat " + name + " | " + template.replace(" {F}", "", 1).replace("{F}", name)
    if form == "d":
        return "cat " + name + " | " + template.replace("{F}", "-", 1).replace("{F}", name)
    if form == "r":
        return template.replace("{F}", "< " + name, 1).replace("{F}", name)
    raise ValueError(form)


FORM_LABEL = {"f": "file operand", "p": "stdin pipe", "d": "dash operand", "r": "redirected stdin"}


def _script(example, shape_name, form):
    command, _slug, template, content, second, _forms, _shapes, extra = example
    lines = ["mkdir -p /tmp/w && cd /tmp/w"]
    name = "input.txt"
    if shape_name == "spaced":
        name = "'my file.txt'"
        lines.append(_pf("'my file.txt'", C[content]))
    elif shape_name == "missing":
        name = "missing.txt"
    elif shape_name == "dir":
        name = "sub"
        lines.append("mkdir sub")
    elif content is not None:
        lines.append(_pf("input.txt", shape(C[content], shape_name)))
    if second:
        lines.append(_pf("other.txt", SECOND.get(second, C.get(second, ""))))
    if extra:
        lines.append(extra.replace("{F}", name))
    lines.append(_render(template, form, name) if content is not None else template)
    return "\n".join(lines)


CASES = []
_seen = set()


def _add(name, script, tags):
    # tldr shows some invocations under two headings; keep the first.
    if script in _seen:
        return
    _seen.add(script)
    CASES.append((name, script, tags))


for _ex in EXAMPLES:
    _cmd, _slug, _tmpl, _content, _second, _forms, _shapes, _extra = _ex
    _base = "real tldr " + _cmd + ": " + _slug
    _tags = ("cmd." + _cmd,)
    if not _forms:
        _add(_base, "mkdir -p /tmp/w && cd /tmp/w\n" + _tmpl, _tags)
        continue
    _add(_base, _script(_ex, "plain", _forms[0]), _tags)
    for _form in _forms[1:]:
        _add(_base + " [" + FORM_LABEL[_form] + "]", _script(_ex, "plain", _form), _tags)
    for _shape in _shapes:
        if _shape in ERROR_SHAPES or _shape == "spaced":
            _form = "f" if "f" in _forms else ("r" if "r" in _forms else None)
            if _form is None:
                continue
            _t = _tags + (("error",) if _shape in ERROR_SHAPES else ())
            _add(_base + " [" + _shape + "]", _script(_ex, _shape, _form), _t)
        else:
            _add(_base + " [" + _shape + "]", _script(_ex, _shape, _forms[0]), _tags)


# Dropped after recording, with the reason for each group.
# GNU quotes the name with curly quotes under the oracle's UTF-8 locale and nothing else differs: the documented
# deliberate difference UTF8_QUOTES (env_facts.py, command_errors.py).
_DROPPED_Q = (
    'real tldr csplit: above a regex match [empty]',
    'real tldr csplit: custom prefix [empty]',
    'real tldr csplit: every 5th line',
    'real tldr csplit: every 5th line [crlf]',
    'real tldr csplit: every 5th line [empty]',
    'real tldr csplit: every 5th line [nonl]',
    'real tldr csplit: every 5th line [nul]',
    'real tldr csplit: every 5th line [utf8]',
    'real tldr csplit: every 5th line [ws]',
    'real tldr csplit: every 5th line keeping files',
    'real tldr csplit: every 5th line keeping files (long)',
    'real tldr csplit: every 5th line keeping files [crlf]',
    'real tldr csplit: every 5th line keeping files [empty]',
    'real tldr csplit: every 5th line keeping files [nonl]',
    'real tldr csplit: every 5th line keeping files [nul]',
    'real tldr csplit: every 5th line keeping files [utf8]',
    'real tldr csplit: every 5th line keeping files [ws]',
    'real tldr csplit: in three at lines 7 and 12 [empty]',
    'real tldr csplit: in two at line 10 [empty]',
)
# The oracle's rev is BusyBox, not util-linux: it stops at a NUL and reads a directory as empty, so it is no
# reference for these shapes.
_DROPPED_B = (
    'real tldr rev: reverse a file [dir]',
    'real tldr rev: reverse a file [nul]',
    'real tldr rev: reverse stdin [dir]',
    'real tldr rev: reverse stdin [nul]',
)
_DROPPED = set(_DROPPED_Q + _DROPPED_B)
CASES = [case for case in CASES if case[0] not in _DROPPED]
