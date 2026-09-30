"""Option sweep, part B: date, env, expand, unexpand, expr, factor, fmt, fold, head, join, link,
ln, ls, mkdir, mktemp, mv, nl, numfmt, od, paste, readlink, realpath, rm and rmdir.

Only deterministic forms: dates are fixed (`-d @N`, `-r` on a file whose time `touch -d` set),
and mktemp's random names are only measured, never printed. Error paths fold GNU's curly quotes
into ASCII ones, as in part A.
"""

TIER = "sweep"
CASES = []


def err(script):
    return "{ " + script + "; } 2>/tmp/err; echo \"status=$?\"; sed \"s/[\u2018\u2019]/'/g\" /tmp/err"


def add(cmd, label, script, error=False):
    tags = [f"cmd.{cmd}"]
    if error:
        tags += [f"cmd.{cmd}.error", "error"]
        script = err(script)
    CASES.append((f"opt {cmd}: {label}", script, tags))


# --- date ----------------------------------------------------------------------------------------
# One fixed instant (a Tuesday in a leap year, 13:04:05.123456789 UTC) for every conversion.
T = "1709211845.123456789"
for spec in "a A b B c C d D e F g G h H I j k l m M n N p P q r R s S t T u U V w W x X y Y z Z".split():
    add("date", f"conversion %{spec}", f"date -u -d @{T} '+[%{spec}]'")
for spec in ["%:z", "%::z", "%:::z", "%%", "%-d", "%_d", "%0e", "%^a", "%^B", "%#Z", "%#p", "%10Y", "%-j", "%_H", "%-I", "%05d", "%3N", "%6N", "%-m/%-d", "%Ey", "%Od", "%+4Y", "%_10m"]:
    add("date", f"conversion {spec}", f"date -u -d @{T} '+[{spec}]'")
add("date", "unknown conversion", f"date -u -d @{T} '+[%Q]'")
add("date", "trailing percent", f"date -u -d @{T} '+x%'")
add("date", "default format", "date -u -d @0")
add("date", "iso default", "date -u -d @0 -I")
for p in ["date", "hours", "minutes", "seconds", "ns"]:
    add("date", f"iso {p}", f"date -u -d @{T} --iso-8601={p}")
add("date", "iso bad precision", "date -u -d @0 --iso-8601=weeks", error=True)
add("date", "rfc email", f"date -u -d @{T} -R")
add("date", "rfc email long", f"date -u -d @{T} --rfc-email")
for p in ["date", "seconds", "ns"]:
    add("date", f"rfc-3339 {p}", f"date -u -d @{T} --rfc-3339={p}")
add("date", "rfc-3339 missing precision", "date -u -d @0 --rfc-3339", error=True)
add("date", "utc long option", "date --utc -d @0 +%T")
add("date", "universal option", "date --universal -d @0 +%T")
add("date", "tz variable utc offset", "TZ=UTC-3 date -d @0 '+%H %Z %z'")
add("date", "tz posix rule", "TZ='EST5EDT,M3.2.0,M11.1.0' date -d @1720000000 '+%H %Z %z'")
# POSIX TZ strings: the oracle image has no zone database, so an IANA name there falls back to UTC.
add("date", "tz named zone", "TZ=JST-9 date -d @0 '+%H %Z %z'")
add("date", "tz named zone with dst", "TZ=EST5EDT,M3.2.0,M11.1.0 date -d @1720000000 '+%H %Z %z'")
add("date", "tz empty", "TZ= date -d @0 '+%H %Z'")
add("date", "tz in date string", "date -u -d 'TZ=\"IST-5:30\" 2024-01-01 00:00' +%T")
for s in ["2024-02-29", "2024-02-29 12:34:56", "2024-02-29T12:34:56Z", "2024-02-29 12:34:56 +0530", "Feb 29 2024",
          "29 Feb 2024", "2024-03-01 -1 day", "2024-01-31 +1 month", "2024-01-01 12:00 +3 hours", "2024-01-01 next monday",
          "2024-01-01 last friday", "1970-01-01 00:00:00 UTC +100000 seconds", "@-1", "@1e3", "2024-12-31 23:59:59.5",
          "20240229", "2024-W09", "12/31/2024", "noon 2024-01-01", "midnight 2024-01-01", "2024-01-01 3pm", "2024-01-01 1 fortnight",
          "2024-06-15 week", "2024-01-01 00:00 Z", "2024-01-01 00:00 EST"]:
    add("date", f"date string {s}", f"date -u -d '{s}' '+%F %T %a'", error=True)
add("date", "invalid date", "date -d 'not a date'", error=True)
add("date", "invalid month", "date -u -d 2024-13-01", error=True)
add("date", "feb 30", "date -u -d 2023-02-30", error=True)
add("date", "extra operand", "date -u +%Y +%m", error=True)
add("date", "operand without plus", "date -u -d @0 Y", error=True)
add("date", "file option", "printf '@0\\n@86400\\n2024-02-29\\n' >/tmp/d; date -u -f /tmp/d +%F")
add("date", "file option bad line", "printf '@0\\nbogus\\n@60\\n' >/tmp/d; date -u -f /tmp/d +%T", error=True)
add("date", "file from stdin", "printf '@0\\n' | date -u -f - +%s")
add("date", "file missing", "date -f /tmp/nosuch", error=True)
add("date", "date and file conflict", "printf '@0\\n' >/tmp/d; date -d @0 -f /tmp/d", error=True)
add("date", "reference file", "touch -d '2001-02-03 04:05:06 UTC' /tmp/r; date -u -r /tmp/r '+%F %T'")
add("date", "reference missing", "date -r /tmp/nosuch", error=True)
add("date", "date long option", "date -u --date=@0 +%F")
add("date", "format with newline escape", "date -u -d @0 '+%Y%n%m%t%d' | od -An -c")
add("date", "resolution", "date --resolution >/dev/null; echo \"status=$?\"")
add("date", "unknown option", "date --bogus", error=True)
add("date", "debug", "date -u --debug -d '2024-01-01 12:00' +%F", error=True)
add("date", "empty format", "date -u -d @0 +; echo \"[$?]\"")
add("date", "day overflow in string", "date -u -d '2024-01-01 +40 days' +%F")
add("date", "large epoch", "date -u -d @4102444800 +%F")
add("date", "negative epoch", "date -u -d @-86400 '+%F %A'")

# --- env -----------------------------------------------------------------------------------------
add("env", "ignore environment", "env -i A=1 B=2 env")
add("env", "dash alone", "env - A=1 env")
add("env", "ignore environment long", "env --ignore-environment X=y env")
add("env", "unset", "A=1 B=2 env -u A env | grep -E '^(A|B)='")
add("env", "unset long", "A=1 env --unset=A env | grep -c '^A='")
add("env", "null separated", "env -i A=1 B=2 env -0 | od -An -c")
add("env", "null with command", "env -0 true", error=True)
add("env", "chdir", "mkdir -p /tmp/w; env -C /tmp/w pwd")
add("env", "chdir long", "mkdir -p /tmp/w; env --chdir=/tmp/w pwd")
add("env", "chdir missing", "env -C /tmp/nosuch pwd", error=True)
add("env", "chdir without command", "env -C /tmp", error=True)
add("env", "split string", "env -S 'printf %s-%s\\\\n a b'")
add("env", "split string with variable", "X=hi env -S 'echo ${X}'")
add("env", "split string quotes", "env -S \"printf '%s|' 'a b' c\"; echo")
# `-a` shown through a program whose behavior argv[0] sets by definition: the oracle's own
# `true` is a coreutils multi-call binary that argv[0] would select instead.
add("env", "argv0", "env -a foo bash -c 'echo \"$0\"'; echo \"status=$?\"")
add("env", "command not found", "env nosuchcommand", error=True)
add("env", "double dash", "env -i -- A=1 env")
add("env", "assignment then command", "env A=1 B=2 sh -c 'echo $A$B'")
add("env", "unset missing argument", "env -u", error=True)
add("env", "invalid assignment name", "env =x true", error=True)
add("env", "debug", "env -i -v A=1 true", error=True)
add("env", "exit status of command", "env sh -c 'exit 7'; echo \"status=$?\"")
add("env", "ignore signal", "env --ignore-signal=PIPE true; echo \"status=$?\"")
add("env", "default signal", "env --default-signal true; echo \"status=$?\"")
add("env", "block signal", "env --block-signal=INT true; echo \"status=$?\"")
add("env", "invalid signal", "env --ignore-signal=NOPE true", error=True)
add("env", "unset with ignore environment", "env -i -u A B=1 env")
add("env", "empty value", "env -i A= env")
add("env", "value with equals", "env -i 'A=b=c' env")

# --- expand / unexpand ---------------------------------------------------------------------------
TAB = r"printf 'a\tb\tc\n\t\tx\n  \ty\n' >/tmp/t; "
add("expand", "default", TAB + "expand /tmp/t | od -An -c")
add("expand", "tab size 4", TAB + "expand -t 4 /tmp/t")
add("expand", "tab size long", TAB + "expand --tabs=3 /tmp/t")
add("expand", "tab list", TAB + "expand -t 2,6 /tmp/t")
add("expand", "tab list with spaces", TAB + "expand -t '2 6' /tmp/t")
add("expand", "tab list slash", TAB + "expand -t 2,/5 /tmp/t")
add("expand", "tab list plus", TAB + "expand -t 2,+5 /tmp/t")
add("expand", "initial only", TAB + "expand -i /tmp/t")
add("expand", "obsolete size", TAB + "expand -3 /tmp/t")
add("expand", "tab size zero", TAB + "expand -t 0 /tmp/t", error=True)
add("expand", "tab list not ascending", TAB + "expand -t 6,2 /tmp/t", error=True)
add("expand", "tab size invalid", TAB + "expand -t x /tmp/t", error=True)
add("expand", "backspace", "printf 'ab\\b\\tc\\n' | expand | od -An -c")
add("expand", "multibyte before tab", "printf '\u00e9\\tx\\n' | expand -t 4")
add("expand", "stdin dash", "printf '\\tx\\n' | expand -t 2 -")
add("expand", "missing file", "expand /tmp/nosuch", error=True)
UN = r"printf '        a       b\n    x   y\n  \t z\n' >/tmp/u; "
add("unexpand", "default leading", UN + "unexpand /tmp/u | od -An -c")
add("unexpand", "all blanks", UN + "unexpand -a /tmp/u | od -An -c")
add("unexpand", "all long", UN + "unexpand --all /tmp/u | od -An -c")
add("unexpand", "tab size implies all", UN + "unexpand -t 4 /tmp/u | od -An -c")
add("unexpand", "first only", UN + "unexpand -t 4 --first-only /tmp/u | od -An -c")
add("unexpand", "tab list", UN + "unexpand -t 4,8 /tmp/u | od -An -c")
add("unexpand", "tab size zero", UN + "unexpand -t 0 /tmp/u", error=True)
add("unexpand", "tab list not ascending", UN + "unexpand -t 8,4 /tmp/u", error=True)
add("unexpand", "obsolete size", UN + "unexpand -4 /tmp/u | od -An -c")
add("unexpand", "missing file", "unexpand /tmp/nosuch", error=True)
add("unexpand", "single space not converted", "printf 'a b\\n' | unexpand -a | od -An -c")

# --- expr ----------------------------------------------------------------------------------------
X = "; echo \"status=$?\""
for label, e in [("add", "3 + 4"), ("subtract", "3 - 10"), ("multiply", "6 '*' 7"), ("divide", "17 / 5"), ("negative divide", "-17 / 5"),
                 ("modulo", "17 % 5"), ("negative modulo", "-17 % 5"), ("less", "2 '<' 10"), ("string less", "abc '<' abd"),
                 ("mixed less", "2 '<' abc"), ("numeric not string", "10 '<' 9"), ("less equal", "3 '<=' 3"), ("equal", "a = a"),
                 ("numeric equal", "01 = 1"), ("not equal", "a != b"), ("greater", "b '>' a"), ("greater equal", "1 '>=' 2"),
                 ("or first", "0 '|' 5"), ("or empty", "'' '|' ''"), ("and", "3 '&' 4"), ("and zero", "3 '&' 0"),
                 ("match count", "abcde : 'a.c'"), ("match group", "abcde : 'a\\(.*\\)e'"), ("match fails", "abc : 'x'"),
                 ("match anchored", "xabc : 'abc'"), ("match keyword", "match hello 'h.l'"), ("substr", "substr hello 2 3"),
                 ("substr past end", "substr hello 4 10"), ("substr zero", "substr hello 0 2"), ("index", "index hello lo"),
                 ("index none", "index hello z"), ("length", "length hello"), ("length multibyte", "length '\u00e9t\u00e9'"),
                 ("plus token", "+ match"), ("parentheses", "'(' 2 + 3 ')' '*' 4"), ("precedence", "2 + 3 '*' 4"),
                 ("big numbers", "99999999999999999999 + 1"), ("negative zero", "-0"), ("leading plus number", "+5 + 1"),
                 ("string result", "hello"), ("zero result", "0"), ("empty result", "''"), ("match multibyte", "'\u00e9t\u00e9' : '.*'"),
                 ("regex interval", "aaa : 'a\\{2\\}'"), ("regex star first", "'*a' : '*a'"), ("regex anchor literal", "a^b : 'a^b'"),
                 ("string compare numbers with spaces", "' 1' = 1"), ("substr keyword arg", "substr + 1 1"), ("length of keyword", "length length")]:
    add("expr", label, f"expr {e}" + X)
add("expr", "division by zero", "expr 1 / 0", error=True)
add("expr", "modulo by zero", "expr 1 % 0", error=True)
add("expr", "non-integer argument", "expr a + 1", error=True)
add("expr", "syntax error", "expr 1 +", error=True)
add("expr", "missing argument", "expr", error=True)
add("expr", "unbalanced parenthesis", "expr '(' 1", error=True)
add("expr", "extra closing parenthesis", "expr 1 ')'", error=True)
add("expr", "invalid regex", "expr a : '\\('", error=True)
add("expr", "substr non-numeric", "expr substr hello a 2", error=True)
add("expr", "overflow", "expr 9223372036854775807 '*' 9223372036854775807" + X)
add("expr", "double dash", "expr -- 3 + 1" + X)
add("expr", "unknown option like", "expr --bogus", error=True)

# --- factor --------------------------------------------------------------------------------------
add("factor", "several", "factor 1 2 12 97 1001")
add("factor", "zero and one", "factor 0 1")
add("factor", "large prime", "factor 18446744073709551557")
add("factor", "above 64 bits", "factor 18446744073709551617")
add("factor", "128 bit", "factor 340282366920938463463374607431768211455")
add("factor", "exponents", "factor -h 1024 360")
add("factor", "exponents long", "factor --exponents 72")
add("factor", "stdin", "printf '10 21\\n33\\n' | factor")
add("factor", "leading plus", "factor +15")
add("factor", "whitespace padded", "factor ' 15 '")
add("factor", "negative", "factor -- -5", error=True)
add("factor", "not a number", "factor 12x", error=True)
add("factor", "empty operand", "factor ''", error=True)
add("factor", "continues after invalid", "factor 4 x 9", error=True)
add("factor", "stdin invalid token", "echo '6 z 8' | factor", error=True)
add("factor", "very large", "factor 1000000000000000000000000000000000000000000000000000001", error=True)

# --- fmt -----------------------------------------------------------------------------------------
P = r"printf 'The quick brown fox jumps over the lazy dog. It barked.  Then it ran away quickly.\n\n  indented line one\nsecond line of the para\n' >/tmp/p; "
add("fmt", "default", P + "fmt /tmp/p")
add("fmt", "width 20", P + "fmt -w 20 /tmp/p")
add("fmt", "width long", P + "fmt --width=30 /tmp/p")
add("fmt", "obsolete width", P + "fmt -25 /tmp/p")
add("fmt", "goal", P + "fmt -w 40 -g 30 /tmp/p")
add("fmt", "crown margin", P + "fmt -c -w 20 /tmp/p")
add("fmt", "tagged paragraph", P + "fmt -t -w 20 /tmp/p")
add("fmt", "split only", P + "fmt -s -w 20 /tmp/p")
add("fmt", "uniform spacing", P + "fmt -u /tmp/p")
add("fmt", "prefix", "printf '# aa bb cc dd\\n# ee ff\\nplain text here\\n' | fmt -p '# ' -w 10")
add("fmt", "goal above width", P + "fmt -w 10 -g 20 /tmp/p", error=True)
add("fmt", "invalid width", P + "fmt -w x /tmp/p", error=True)
add("fmt", "missing file", "fmt /tmp/nosuch", error=True)
add("fmt", "long word", "printf 'aaaaaaaaaaaaaaaaaaaaaaaaa b\\n' | fmt -w 10")
add("fmt", "stdin", "printf 'a b c d e f\\n' | fmt -w 5")
add("fmt", "tabs kept in indent", "printf '\\tone two three four\\n' | fmt -w 15 | od -An -c")

# --- fold ----------------------------------------------------------------------------------------
FL = r"printf 'abcdefghij klmnop qrstuvwxyz\n\tab\n' >/tmp/f; "
add("fold", "width 8", FL + "fold -w 8 /tmp/f")
add("fold", "width long", FL + "fold --width=5 /tmp/f")
add("fold", "obsolete width", FL + "fold -6 /tmp/f")
add("fold", "spaces", FL + "fold -s -w 12 /tmp/f")
add("fold", "bytes", "printf '\u00e9\u00e9\u00e9\\n' | fold -b -w 3 | od -An -c")
add("fold", "characters", "printf '\u00e9\u00e9\u00e9\u00e9\\n' | fold -c -w 3")
add("fold", "tab counts to next stop", "printf 'a\\tbcdefghij\\n' | fold -w 10")
add("fold", "tab with bytes", "printf 'a\\tbcdefghij\\n' | fold -b -w 4")
add("fold", "backspace", "printf 'abc\\bdefgh\\n' | fold -w 4 | od -An -c")
add("fold", "carriage return", "printf 'abcdef\\rghijk\\n' | fold -w 4 | od -An -c")
add("fold", "default width", "printf '%0100d\\n' 0 | fold")
add("fold", "width zero", "echo abc | fold -w 0", error=True)
add("fold", "invalid width", "echo abc | fold -w x", error=True)
add("fold", "missing file", "fold /tmp/nosuch", error=True)
add("fold", "no trailing newline", "printf 'abcdef' | fold -w 4; echo")
add("fold", "spaces no break point", "printf 'abcdefghij\\n' | fold -s -w 4")

# --- head ----------------------------------------------------------------------------------------
H = "seq 1 20 >/tmp/n; seq 21 25 >/tmp/m; "
for o in ["-n 3", "-n -17", "-n 0", "-c 5", "-c -45", "-c 0", "-3", "--lines=2", "--bytes=4", "-n +3", "-c 1k", "-n 1K", "-c 2b", "-n 1kB",
          "-q", "-v", "--quiet", "--verbose", "--silent", "-n 2 -v", "-c3", "-n2 -q"]:
    add("head", f"option {o}", H + f"head {o} /tmp/n")
add("head", "two files headers", H + "head -n 2 /tmp/n /tmp/m")
add("head", "two files quiet", H + "head -q -n 1 /tmp/n /tmp/m")
add("head", "stdin and file", H + "echo in | head -n 1 - /tmp/m")
add("head", "zero terminated", "printf 'a\\0b\\0c\\0' | head -z -n 2 | od -An -c")
add("head", "zero terminated negative", "printf 'a\\0b\\0c' | head -z -n -1 | od -An -c")
add("head", "missing file continues", H + "head -n 1 /tmp/nosuch /tmp/m", error=True)
add("head", "invalid count", H + "head -n x /tmp/n", error=True)
add("head", "invalid byte count", H + "head -c 2x /tmp/n", error=True)
add("head", "huge count", H + "head -n 99999999999999999999 /tmp/m")
add("head", "directory", "mkdir /tmp/d; head /tmp/d", error=True)
add("head", "obsolete with suffix", H + "head -2c /tmp/n")
add("head", "unknown option", "head --bogus", error=True)
add("head", "no trailing newline", "printf 'a\\nb' | head -n 5; echo")
add("head", "negative lines without trailing newline", "printf 'a\\nb\\nc' | head -n -1")
add("head", "lines and bytes last wins", H + "head -n 2 -c 3 /tmp/n")

# --- join ----------------------------------------------------------------------------------------
J = r"printf '1 a x\n2 b y\n3 c z\n' >/tmp/1; printf '1 A\n3 C\n4 D\n' >/tmp/2; "
add("join", "default", J + "join /tmp/1 /tmp/2")
add("join", "unpaired from 1", J + "join -a 1 /tmp/1 /tmp/2")
add("join", "unpaired from both", J + "join -a 1 -a 2 /tmp/1 /tmp/2")
add("join", "only unpaired 2", J + "join -v 2 /tmp/1 /tmp/2")
add("join", "only unpaired 1", J + "join -v1 /tmp/1 /tmp/2")
add("join", "empty filler", J + "join -a 2 -e NONE -o 1.2,2.2 /tmp/1 /tmp/2")
add("join", "output format", J + "join -o 2.2,1.3,0 /tmp/1 /tmp/2")
add("join", "output auto", J + "join -a 1 -e - -o auto /tmp/1 /tmp/2")
add("join", "field option", "printf 'a 1\\nb 2\\n' >/tmp/1; printf 'x 1\\ny 2\\n' >/tmp/2; join -j 2 /tmp/1 /tmp/2")
add("join", "fields per file", "printf 'a 1\\nb 2\\n' >/tmp/1; printf '1 x\\n2 y\\n' >/tmp/2; join -1 2 -2 1 /tmp/1 /tmp/2")
add("join", "separator", "printf '1,a\\n2,b\\n' >/tmp/1; printf '1,x\\n' >/tmp/2; join -t , /tmp/1 /tmp/2")
add("join", "tab separator", "printf '1\\ta b\\n' >/tmp/1; printf '1\\tx\\n' >/tmp/2; join -t \"$(printf '\\t')\" /tmp/1 /tmp/2")
add("join", "ignore case", "printf 'A 1\\nb 2\\n' >/tmp/1; printf 'a x\\nB y\\n' >/tmp/2; join -i /tmp/1 /tmp/2")
add("join", "header", "printf 'id n\\n1 a\\n' >/tmp/1; printf 'id m\\n1 x\\n' >/tmp/2; join --header /tmp/1 /tmp/2")
add("join", "zero terminated", "printf '1 a\\0' >/tmp/1; printf '1 b\\0' >/tmp/2; join -z /tmp/1 /tmp/2 | od -An -c")
add("join", "unsorted warns", "printf '2 a\\n1 b\\n' >/tmp/1; printf '1 x\\n2 y\\n' >/tmp/2; join /tmp/1 /tmp/2", error=True)
add("join", "check order", "printf '2 a\\n1 b\\n' >/tmp/1; printf '2 y\\n' >/tmp/2; join --check-order /tmp/1 /tmp/2", error=True)
add("join", "nocheck order", "printf '2 a\\n1 b\\n' >/tmp/1; printf '1 x\\n2 y\\n' >/tmp/2; join --nocheck-order /tmp/1 /tmp/2; echo \"status=$?\"")
add("join", "stdin", J + "cat /tmp/2 | join /tmp/1 -")
add("join", "invalid file number", J + "join -a 3 /tmp/1 /tmp/2", error=True)
add("join", "invalid field", J + "join -1 0 /tmp/1 /tmp/2", error=True)
add("join", "invalid output format", J + "join -o 3.1 /tmp/1 /tmp/2", error=True)
add("join", "multi-char separator", J + "join -t ab /tmp/1 /tmp/2", error=True)
add("join", "missing operand", J + "join /tmp/1", error=True)
add("join", "missing file", J + "join /tmp/1 /tmp/nosuch", error=True)
add("join", "duplicate keys", "printf '1 a\\n1 b\\n' >/tmp/1; printf '1 x\\n1 y\\n' >/tmp/2; join /tmp/1 /tmp/2")
add("join", "blank separated fields", "printf '1   a\\n' >/tmp/1; printf ' 1 x\\n' >/tmp/2; join /tmp/1 /tmp/2")

# --- link / ln -----------------------------------------------------------------------------------
add("link", "creates", "cd /tmp; printf x >a; link a b; cat b")
add("link", "missing operand", "link", error=True)
add("link", "one operand", "link /tmp/a", error=True)
add("link", "extra operand", "link a b c", error=True)
add("link", "existing destination", "cd /tmp; printf x >a; printf y >b; link a b", error=True)
add("link", "missing source", "link /tmp/nosuch /tmp/b", error=True)
add("link", "directory source", "mkdir /tmp/d; link /tmp/d /tmp/e", error=True)
LN = "mkdir -p /tmp/w/d; cd /tmp/w; printf x >a; "
add("ln", "hard link", LN + "ln a b; cat b")
add("ln", "symbolic relative", LN + "ln -s a s; readlink s; cat s")
add("ln", "symbolic long", LN + "ln --symbolic a s; readlink s")
add("ln", "into directory", LN + "ln a d; ls d")
add("ln", "symbolic into directory", LN + "ln -s ../a d/; readlink d/a")
add("ln", "force", LN + "printf y >b; ln -f a b; cat b")
add("ln", "force symbolic", LN + "ln -s a s; ln -sf d s; readlink s")
add("ln", "no dereference", LN + "ln -s d s; ln -sfn a s; readlink s")
add("ln", "dereference directory link", LN + "ln -s d s; ln -sf a s; readlink d/a")
add("ln", "backup", LN + "printf y >b; ln -b a b; cat b~")
add("ln", "backup suffix", LN + "printf y >b; ln -b -S .old a b; cat b.old")
add("ln", "backup numbered", LN + "printf y >b; ln --backup=numbered a b; ls b*")
add("ln", "target directory", LN + "ln -t d a; ls d")
add("ln", "no target directory", LN + "ln -T a d", error=True)
add("ln", "verbose", LN + "ln -v a b; ln -sv a s")
add("ln", "relative", LN + "ln -sr a d/r; readlink d/r")
add("ln", "relative absolute paths", LN + "ln -sr /tmp/w/a /tmp/w/d/r2; readlink d/r2")
add("ln", "logical", LN + "ln -s a s; ln -L s h; [ -L h ] && echo link || cat h")
add("ln", "physical", LN + "ln -s a s; ln -P s h; readlink h")
add("ln", "existing destination", LN + "printf y >b; ln a b", error=True)
add("ln", "missing source", LN + "ln nosuch b", error=True)
add("ln", "dangling symlink allowed", LN + "ln -s nosuch s; readlink s; [ -e s ] || echo dangling")
add("ln", "hard link directory", LN + "ln d e", error=True)
add("ln", "missing operand", "ln", error=True)
add("ln", "target not directory", LN + "printf y >b; ln a b c", error=True)
add("ln", "same file", LN + "ln -f a a", error=True)
add("ln", "interactive no", LN + "printf y >b; echo n | ln -i a b", error=True)
add("ln", "one operand", LN + "ln -s d/../a; ls", error=True)
add("ln", "unknown option", "ln --bogus", error=True)

# --- ls ------------------------------------------------------------------------------------------
L = ("mkdir -p /tmp/l/sub /tmp/l/.hid; cd /tmp/l; printf 12345 >b.txt; printf 1 >a.log; printf 123 >C.txt; "
     "printf '' >'with space'; ln -s b.txt link; touch -d '2001-01-01' a.log; touch -d '2002-01-01' b.txt; "
     "touch -d '2003-01-01' C.txt; touch -d '2000-01-01' 'with space'; ")
for o in ["", "-1", "-a", "-A", "-r", "-R", "-d", "-F", "-p", "-S *.txt a.log", "-t", "-X", "-v", "-U | sort", "-m", "-x -w 30", "-C -w 30", "-Q", "-b",
          "-N", "--indicator-style=slash", "--indicator-style=classify", "--indicator-style=file-type", "--group-directories-first",
          "-B", "--hide='*.txt'", "-I '*.log'", "--color=never", "-tr", "-Sr b.txt a.log C.txt", "-aR", "-AF", "--sort=size C.txt b.txt", "--sort=time", "--sort=extension",
          "--sort=none | sort", "--sort=version", "--format=commas", "--format=single-column", "--quoting-style=literal",
          "--quoting-style=shell", "--quoting-style=shell-always", "--quoting-style=c", "--quoting-style=escape", "--file-type",
          "--literal", "--escape", "--quote-name", "--zero | od -An -c", "--classify=never", "-1 --dereference-command-line link", "-L", "-w 20",
          "--width=0", "-T 4 -C -w 40", "-k", "--directory sub"]:
    add("ls", f"option {o or 'none'}", L + f"ls {o}")
add("ls", "several operands", L + "ls sub b.txt a.log")
add("ls", "directory operands headers", L + "ls sub .")
add("ls", "missing operand", L + "ls nosuch b.txt", error=True)
add("ls", "missing only", "ls /tmp/nosuch", error=True)
add("ls", "invalid sort", L + "ls --sort=bogus", error=True)
add("ls", "invalid width", L + "ls -w x", error=True)
add("ls", "invalid quoting style", L + "ls --quoting-style=nope", error=True)
add("ls", "unknown option", "ls --bogus", error=True)
add("ls", "ignore backups", "mkdir /tmp/k; cd /tmp/k; touch a a~ b; ls -B")
add("ls", "version sort", "mkdir /tmp/k; cd /tmp/k; touch f10 f9 f1 f1.10 f1.9; ls -v")
add("ls", "control characters", "mkdir /tmp/k; cd /tmp/k; touch \"$(printf 'a\\tb')\"; ls -q; ls --show-control-chars | od -An -c")
add("ls", "dotfiles order", "mkdir /tmp/k; cd /tmp/k; touch .b a .a B; ls -a")
add("ls", "empty directory", "mkdir /tmp/k; ls /tmp/k; echo \"status=$?\"")
add("ls", "recursive nested", "mkdir -p /tmp/k/x/y; touch /tmp/k/x/y/z; ls -R /tmp/k")
add("ls", "directory slash operand", L + "ls -d sub/ .hid")

# --- mkdir ---------------------------------------------------------------------------------------
add("mkdir", "parents", "mkdir -p /tmp/a/b/c; find /tmp/a | sort")
add("mkdir", "parents existing", "mkdir -p /tmp; echo \"status=$?\"")
add("mkdir", "verbose", "mkdir -v /tmp/a", error=True)
add("mkdir", "verbose parents", "mkdir -pv /tmp/a/b", error=True)
add("mkdir", "mode", "mkdir -m 700 /tmp/a; echo \"status=$?\"; [ -d /tmp/a ] && echo made")
add("mkdir", "mode symbolic", "mkdir -m u=rwx,go= /tmp/a; echo \"status=$?\"")
add("mkdir", "invalid mode", "mkdir -m zz /tmp/a", error=True)
add("mkdir", "several", "mkdir /tmp/a /tmp/b; ls /tmp")
add("mkdir", "existing", "mkdir /tmp", error=True)
add("mkdir", "missing parent", "mkdir /tmp/x/y", error=True)
add("mkdir", "file in path", "printf x >/tmp/f; mkdir -p /tmp/f/g", error=True)
add("mkdir", "missing operand", "mkdir", error=True)
add("mkdir", "continues after failure", "mkdir /tmp/x/y /tmp/z; ls /tmp", error=True)
add("mkdir", "trailing slash", "mkdir /tmp/a/; ls -d /tmp/a")
add("mkdir", "parents with dot dot", "mkdir -p /tmp/a/../b/./c; find /tmp | sort")
add("mkdir", "unknown option", "mkdir --bogus x", error=True)

# --- mktemp --------------------------------------------------------------------------------------
LEN = " | wc -c"
add("mktemp", "default creates in tmp", "f=$(mktemp); case $f in /tmp/tmp.??????????) [ -f \"$f\" ] && echo ok;; *) echo \"bad $f\";; esac")
add("mktemp", "directory", "d=$(mktemp -d); [ -d \"$d\" ] && echo dir; case $d in /tmp/tmp.*) echo prefix;; esac")
add("mktemp", "template", "f=$(mktemp /tmp/fooXXXX); echo ${#f}; case $f in /tmp/foo????) echo shape;; esac")
add("mktemp", "template in cwd", "cd /tmp; f=$(mktemp abcXXXXXX); echo ${#f}; [ -f \"/tmp/$f\" ] && echo there")
add("mktemp", "dry run", "f=$(mktemp -u /tmp/xXXXXX); echo ${#f}; [ -e \"$f\" ] || echo absent")
add("mktemp", "quiet failure", "mktemp -q /tmp/nosuch/XXXX; echo \"status=$?\"")
add("mktemp", "suffix", "f=$(mktemp --suffix=.txt /tmp/aXXX); case $f in /tmp/a???.txt) echo shape;; esac")
add("mktemp", "suffix in template", "f=$(mktemp /tmp/aXXX.txt); case $f in /tmp/a???.txt) echo shape;; esac")
add("mktemp", "tmpdir option", "mkdir /tmp/d; f=$(mktemp -p /tmp/d zzXXX); case $f in /tmp/d/zz???) echo shape;; esac")
add("mktemp", "tmpdir long", "mkdir /tmp/d; f=$(mktemp --tmpdir=/tmp/d); case $f in /tmp/d/tmp.*) echo shape;; esac")
add("mktemp", "t option", "f=$(mktemp -t fooXXX); case $f in /tmp/foo???) echo shape;; esac")
add("mktemp", "tmpdir env", "mkdir /tmp/e; f=$(TMPDIR=/tmp/e mktemp -t zXXX); case $f in /tmp/e/z???) echo shape;; esac")
add("mktemp", "too few X", "mktemp /tmp/aXX", error=True)
add("mktemp", "no X", "mktemp /tmp/abc", error=True)
add("mktemp", "slash in suffix", "mktemp --suffix=/x /tmp/aXXX", error=True)
add("mktemp", "missing directory", "mktemp /tmp/nosuch/aXXX", error=True)
add("mktemp", "too many templates", "mktemp aXXX bXXX", error=True)
add("mktemp", "template with slash and p", "mktemp -p /tmp a/bXXX", error=True)
add("mktemp", "directory dry run", "d=$(mktemp -d -u); [ -e \"$d\" ] || echo absent")
add("mktemp", "X in the middle", "f=$(mktemp /tmp/aXXXb); echo ${#f}", error=True)
add("mktemp", "unknown option", "mktemp --bogus", error=True)
add("mktemp", "files are empty", "f=$(mktemp); wc -c <\"$f\"")

# --- mv ------------------------------------------------------------------------------------------
MV = "mkdir -p /tmp/m/d; cd /tmp/m; printf one >a; printf two >b; "
add("mv", "rename", MV + "mv a c; cat c; ls")
add("mv", "into directory", MV + "mv a b d; ls d")
add("mv", "force", MV + "mv -f a b; cat b")
add("mv", "no clobber", MV + "mv -n a b; cat b; ls")
add("mv", "interactive no", MV + "echo n | mv -i a b", error=True)
add("mv", "interactive yes", MV + "echo y | mv -i a b 2>/dev/null; cat b; ls")
add("mv", "update older source", MV + "touch -d 2000-01-01 a; mv -u a b; cat b; ls")
add("mv", "update newer source", MV + "touch -d 2000-01-01 b; mv -u a b; cat b; ls")
add("mv", "update none", MV + "mv --update=none a b; cat b")
add("mv", "verbose", MV + "mv -v a c", error=True)
add("mv", "backup", MV + "mv -b a b; cat b b~")
add("mv", "backup suffix", MV + "mv -b -S .bak a b; cat b.bak")
add("mv", "backup numbered", MV + "mv --backup=t a b; ls")
add("mv", "target directory", MV + "mv -t d a b; ls d")
add("mv", "no target directory", MV + "mv -T a d", error=True)
add("mv", "no target directory rename dir", MV + "mkdir e; mv -T d e; ls")
add("mv", "strip trailing slashes", MV + "mv --strip-trailing-slashes d/ e; ls")
add("mv", "directory into itself", MV + "mv d d/x", error=True)
add("mv", "missing source", MV + "mv nosuch c", error=True)
add("mv", "missing operand", "mv", error=True)
add("mv", "missing destination", MV + "mv a", error=True)
add("mv", "target not directory", MV + "mv a b c", error=True)
add("mv", "overwrite directory with file", MV + "mkdir c; mv a c; ls c")
add("mv", "file over empty directory with T", MV + "mkdir c; mv -T a c", error=True)
add("mv", "directory over file", MV + "mv d a", error=True)
add("mv", "same file", MV + "mv a a", error=True)
add("mv", "rename directory", MV + "printf x >d/f; mv d e; ls e")
add("mv", "exchange", MV + "mv --exchange a b; cat a b")
add("mv", "unknown option", "mv --bogus a b", error=True)

# --- nl ------------------------------------------------------------------------------------------
NL = r"printf 'a\n\nb\n\n\n\nc\n' >/tmp/n; "
for o in ["-b a", "-b t", "-b n", "-b pb", "-b 'p^[ac]'", "-n ln", "-n rn", "-n rz", "-w 2", "-w 1", "-s ': '", "-s ''", "-v 5", "-v -2",
          "-i 3", "-l 2 -b a", "-l 3 -b a", "--body-numbering=a", "--number-format=rz --number-width=3", "--starting-line-number=0",
          "--line-increment=10", "--number-separator='|'", "--join-blank-lines=2 -ba", "-ba -nln -w3 -s."]:
    add("nl", f"option {o}", NL + f"nl {o} /tmp/n")
SEC = r"printf '\\:\\:\\:\nhead\n\\:\\:\nbody1\nbody2\n\\:\nfoot\n\\:\\:\nbody3\n' >/tmp/s; "
add("nl", "sections default", SEC + "nl /tmp/s")
add("nl", "sections header numbering", SEC + "nl -h a -f a /tmp/s")
add("nl", "sections no renumber", SEC + "nl -p /tmp/s")
add("nl", "section delimiter", "printf '@@@@\\nb\\n@@\\nc\\n' | nl -d @@")
add("nl", "section delimiter one char", "printf 'x:\\nb\\n' | nl -d x")
add("nl", "invalid style", NL + "nl -b x /tmp/n", error=True)
add("nl", "invalid format", NL + "nl -n xx /tmp/n", error=True)
add("nl", "invalid width", NL + "nl -w 0 /tmp/n", error=True)
add("nl", "invalid regex", NL + "nl -b 'p[' /tmp/n", error=True)
add("nl", "missing file", "nl /tmp/nosuch", error=True)
add("nl", "two files continue numbering", NL + "nl /tmp/n /tmp/n")
add("nl", "stdin", "printf 'x\\ny\\n' | nl -")
add("nl", "no trailing newline", "printf 'x\\ny' | nl; echo")

# --- numfmt --------------------------------------------------------------------------------------
for label, cmd in [
    ("to si", "numfmt --to=si 1000 1500 999999 1000000"), ("to iec", "numfmt --to=iec 1024 1536 1048576"),
    ("to iec-i", "numfmt --to=iec-i 2048 5000000"), ("from si", "numfmt --from=si 1K 2.5M 1G"),
    ("from iec", "numfmt --from=iec 1K 1M 1.5G"), ("from iec-i", "numfmt --from=iec-i 1Ki 3Mi"),
    ("from auto", "numfmt --from=auto 1K 1Ki 2M"), ("from none", "numfmt --from=none 123"),
    ("to none", "numfmt --to=none 12345"), ("to unit", "numfmt --to-unit=1024 1048576"),
    ("from unit", "numfmt --from-unit=512 4"), ("from and to units", "numfmt --from-unit=1024 --to=si 1000"),
    ("suffix", "numfmt --suffix=B --to=si 5000"), ("padding right", "numfmt --padding=8 42"),
    ("padding left", "numfmt --padding=-8 42; echo end"), ("grouping", "numfmt --grouping 1234567"),
    ("format width", "numfmt --format='%10f' 42"), ("format left", "numfmt --format='%-10f|' 42"),
    ("format zero pad", "numfmt --format='%010f' 42"), ("format precision", "numfmt --format='%.3f' --to=si 12345"),
    ("format prefix suffix", "numfmt --format='x%fy' 5"), ("format grouping flag", "numfmt --format=\"%'f\" 1234567"),
    ("round up", "numfmt --to=si --round=up 1001"), ("round down", "numfmt --to=si --round=down 1999"),
    ("round nearest", "numfmt --to=si --round=nearest 1450 1550"), ("round from zero", "numfmt --to=si --round=from-zero -- -1001"),
    ("round towards zero", "numfmt --to=si --round=towards-zero -- -1999"), ("negative", "numfmt --to=iec -- -2048"),
    ("field", "echo 'a 2000 3000' | numfmt --to=si --field=2"), ("field range", "echo 'a 2000 3000 4000' | numfmt --to=si --field=2-3"),
    ("field list", "echo '1000 a 3000' | numfmt --to=si --field=1,3"), ("field all", "echo '1000 2000' | numfmt --to=si --field=-"),
    ("delimiter", "echo 'x:2000:y' | numfmt -d: --field=2 --to=si"), ("header", "printf 'size\\n2000\\n' | numfmt --header --to=si"),
    ("header count", "printf 'h1\\nh2\\n2000\\n' | numfmt --header=2 --to=si"), ("stdin lines", "printf '1000\\n2000\\n' | numfmt --to=si"),
    ("zero terminated", "printf '1000\\0' | numfmt -z --to=si | od -An -c"), ("invalid warn", "numfmt --invalid=warn x 1000; echo \"status=$?\""),
    ("invalid ignore", "numfmt --invalid=ignore x 1000; echo \"status=$?\""), ("invalid fail", "numfmt --invalid=fail x 1000 2>/dev/null; echo \"status=$?\""),
    ("unit separator", "numfmt --to=si --unit-separator=' ' 5000"), ("large", "numfmt --to=si 123456789012345"),
    ("decimal input", "numfmt --from=si 1.25K"), ("to si small", "numfmt --to=si 999 1"), ("iec rounding boundary", "numfmt --to=iec 1023 1025"),
    ("leading spaces kept", "echo '   2000' | numfmt --to=si"), ("whitespace fields", "echo ' 1000  2000' | numfmt --field=2 --to=si"),
    ("debug", "numfmt --debug --to=si 1000 2>&1"),
]:
    add("numfmt", label, cmd)
add("numfmt", "invalid number", "numfmt x", error=True)
add("numfmt", "invalid suffix", "numfmt --from=si 1X", error=True)
add("numfmt", "suffix without from", "numfmt 1K", error=True)
add("numfmt", "invalid unit", "numfmt --to=bogus 1", error=True)
add("numfmt", "invalid round", "numfmt --round=bogus 1", error=True)
add("numfmt", "invalid format", "numfmt --format=%d 1", error=True)
add("numfmt", "format two directives", "numfmt --format='%f %f' 1", error=True)
add("numfmt", "invalid padding", "numfmt --padding=0 1", error=True)
add("numfmt", "invalid field", "echo 1 | numfmt --field=0", error=True)
add("numfmt", "too large for si", "numfmt --to=si 1e40", error=True)
add("numfmt", "invalid header", "numfmt --header=0 1", error=True)
add("numfmt", "invalid mode", "numfmt --invalid=bogus 1", error=True)

# --- od ------------------------------------------------------------------------------------------
OD = r"printf 'Hello, World!\n\001\377\000\177abcdefghijklmnop' >/tmp/o; "
for o in ["", "-c", "-a", "-b", "-d", "-o", "-s", "-x", "-i", "-l", "-f", "-h", "-An", "-Ad", "-Ao", "-Ax", "-An -c", "-t x1", "-t x2", "-t x4",
          "-t x8", "-t o1", "-t o2", "-t u1", "-t u2", "-t u4", "-t d1", "-t d2", "-t d4", "-t d8", "-t c", "-t a", "-t x1z", "-t d1 -t c",
          "-t f4", "-t f8", "-t xC", "-t dS", "-t uL", "-t oI", "-j 3 -c", "-N 5 -c", "-j 2 -N 4 -t x1", "-w4 -c", "-w -t x1", "-v -c",
          "-S 3", "-S 5", "--endian=big -t x2", "--endian=little -t x4", "--traditional", "-t x1 --width=8", "-j 0x10 -c", "-N 010 -c",
          "-j 1k -c", "--skip-bytes=5 --read-bytes=3 -t c", "--address-radix=d -t u1", "-t z", "-tx1 -Ax -N 4", "--strings=4"]:
    add("od", f"option {o or 'none'}", OD + f"od {o} /tmp/o")
add("od", "duplicate lines collapsed", "printf '%032d' 0 | od -c")
add("od", "duplicate lines shown", "printf '%032d' 0 | od -v -c")
add("od", "stdin", "printf 'ab' | od -An -tx1")
add("od", "two files", "printf 'ab' >/tmp/x; printf 'cd' >/tmp/y; od -c /tmp/x /tmp/y")
add("od", "empty input", "od </dev/null; echo \"status=$?\"")
add("od", "invalid type", OD + "od -t q /tmp/o", error=True)
add("od", "invalid size", OD + "od -t x3 /tmp/o", error=True)
add("od", "invalid radix", OD + "od -A z /tmp/o", error=True)
add("od", "invalid skip", OD + "od -j x /tmp/o", error=True)
add("od", "skip past end", OD + "od -j 1000 /tmp/o", error=True)
add("od", "invalid width", OD + "od -w0 /tmp/o", error=True)
add("od", "missing file", "od /tmp/nosuch", error=True)
add("od", "float values", "printf '\\000\\000\\200\\077' | od -An -t f4")
add("od", "double values", "printf '\\000\\000\\000\\000\\000\\000\\360\\077' | od -An -t f8")
add("od", "odd length x2", "printf 'abc' | od -An -tx2")
add("od", "traditional offset", OD + "od -c /tmp/o +5")
add("od", "multibyte chars", "printf '\u00e9' | od -c")

# --- paste ---------------------------------------------------------------------------------------
PA = r"printf 'a\nb\nc\n' >/tmp/1; printf '1\n2\n' >/tmp/2; "
for o in ["", "-s", "-d ,", "-d ',;'", "-d '\\n'", "-d '\\t'", "-d '\\\\'", "-d '\\0'", "-s -d :", "--serial --delimiters=+", "-d ''", "-z"]:
    add("paste", f"option {o or 'none'}", PA + f"paste {o} /tmp/1 /tmp/2 | od -An -c")
add("paste", "stdin columns", "seq 1 6 | paste - - -")
add("paste", "stdin with file", PA + "echo x | paste /tmp/1 -")
add("paste", "one file", PA + "paste /tmp/1")
add("paste", "missing file", PA + "paste /tmp/1 /tmp/nosuch", error=True)
add("paste", "trailing backslash delimiter", PA + "paste -d 'a\\' /tmp/1 /tmp/2", error=True)
add("paste", "no trailing newline", "printf 'a\\nb' >/tmp/1; printf 'c' >/tmp/2; paste /tmp/1 /tmp/2")
add("paste", "serial no trailing newline", "printf 'a\\nb' | paste -s -d,")

# --- readlink / realpath -------------------------------------------------------------------------
RL = "mkdir -p /tmp/r/d/e; cd /tmp/r; printf x >d/f; ln -s d/f l1; ln -s l1 l2; ln -s nosuch dangling; ln -s d dl; "
for o in ["l1", "l2", "-f l2", "-e l2", "-m l2", "-f dangling", "-e dangling", "-m dangling/x/y", "-n l1", "-z l1 | od -An -c", "-f dl/e/..",
          "-f ./d/../d/f", "-v d/f", "-q d/f", "-s d/f", "-f l1 l2", "-e nosuch", "-m nosuch/../x", "--canonicalize l2", "--canonicalize-existing dl/e",
          "--canonicalize-missing a/b", "--no-newline l1", "--zero -f l1 | od -An -c", "-f ''", "-f /", "-f ."]:
    add("readlink", f"operands {o}", RL + f"readlink {o}; echo \"status=$?\"")
add("readlink", "missing operand", "readlink", error=True)
add("readlink", "verbose not a link", RL + "readlink -v d/f", error=True)
add("readlink", "verbose missing", RL + "readlink -v nosuch", error=True)
for o in ["d/f", "l2", "dl/e/..", "-s dl/e/..", "-L dl/e/..", "-P dl/e/..", "-e nosuch", "-m nosuch/x", "-E nosuch/x", "-E nosuch/x/y",
          "-q nosuch", "-z d/f | od -An -c", "--relative-to=d/e d/f", "--relative-to=/ /tmp/r/d", "--relative-base=/tmp/r d/f /",
          "--relative-to=d --relative-base=/tmp d/f", "--relative-to=/tmp/r/d/e /tmp", "-s l1", "--no-symlinks l2", "--strip l2",
          "dangling", "-e dangling", ". ..", "''", "d/f l1 nosuch/x", "--relative-to=nosuch d/f", "-m --relative-to=x/y x/z"]:
    add("realpath", f"operands {o}", RL + f"realpath {o}; echo \"status=$?\"")
add("realpath", "missing operand", "realpath", error=True)
add("realpath", "missing file message", RL + "realpath nosuch/x", error=True)
add("realpath", "relative-to missing existing", RL + "realpath -e --relative-to=nosuch d/f", error=True)

# --- rm / rmdir ----------------------------------------------------------------------------------
RM = "mkdir -p /tmp/x/d/e /tmp/x/empty; cd /tmp/x; printf a >f; printf b >g; printf c >d/e/h; "
add("rm", "file", RM + "rm f; ls")
add("rm", "several", RM + "rm f g; ls")
add("rm", "force missing", RM + "rm -f nosuch; echo \"status=$?\"")
add("rm", "force no operand", "rm -f; echo \"status=$?\"")
add("rm", "missing", RM + "rm nosuch", error=True)
add("rm", "directory without recursive", RM + "rm d", error=True)
add("rm", "recursive", RM + "rm -r d; ls")
add("rm", "recursive capital", RM + "rm -R d; ls")
add("rm", "recursive long", RM + "rm --recursive d f; ls")
add("rm", "dir option empty", RM + "rm -d empty; ls")
add("rm", "dir option nonempty", RM + "rm -d d", error=True)
add("rm", "verbose", RM + "rm -v f g", error=True)
add("rm", "verbose recursive", RM + "rm -rv d | sort", error=True)
add("rm", "interactive no", RM + "echo n | rm -i f; ls", error=True)
add("rm", "interactive yes", RM + "echo y | rm -i f 2>/dev/null; ls")
add("rm", "interactive once", RM + "echo n | rm -I f g; ls")
add("rm", "interactive recursive once", RM + "echo n | rm -rI d; ls", error=True)
add("rm", "interactive never", RM + "rm --interactive=never f; ls")
add("rm", "dot refused", RM + "rm -r .", error=True)
add("rm", "dot dot refused", RM + "rm -rf d/..", error=True)
add("rm", "missing operand", "rm", error=True)
add("rm", "symlink not target", RM + "ln -s f l; rm l; cat f; ls")
add("rm", "trailing slash on file", RM + "rm f/", error=True)
add("rm", "one file system", RM + "rm -r --one-file-system d; ls")
add("rm", "double dash", RM + "printf x >-n; rm -- -n; ls")
add("rm", "continues after failure", RM + "rm nosuch f; ls", error=True)
add("rm", "unknown option", "rm --bogus", error=True)
add("rmdir", "empty", RM + "rmdir empty; ls")
add("rmdir", "parents", RM + "rm d/e/h; rmdir -p d/e; ls")
add("rmdir", "parents stops at nonempty", RM + "rm d/e/h; rmdir -p x/../d/e 2>/dev/null; echo \"status=$?\"; ls")
add("rmdir", "verbose", RM + "rmdir -v empty", error=True)
add("rmdir", "verbose parents", RM + "rm d/e/h; rmdir -pv d/e", error=True)
add("rmdir", "ignore nonempty", RM + "rmdir --ignore-fail-on-non-empty d; echo \"status=$?\"")
add("rmdir", "nonempty", RM + "rmdir d", error=True)
add("rmdir", "not a directory", RM + "rmdir f", error=True)
add("rmdir", "missing", RM + "rmdir nosuch", error=True)
add("rmdir", "missing operand", "rmdir", error=True)
add("rmdir", "trailing slash", RM + "rmdir empty/; ls")
add("rmdir", "dot", RM + "rmdir .", error=True)

DATE_DEBUG = (
    "GNU date --debug prints the trace of its own date parser (parse-datetime: the starting "
    "value, epoch seconds, final times, output format); uutils parses dates with the "
    "parse_datetime crate, which keeps no such trace, so date --debug reports only the parts it "
    "knows. Matching it means instrumenting that crate"
)
EXPECTED = {
    "opt date: debug": (
        0,
        b"2024-01-01\nstatus=0\ndate: input string: 2024-01-01 12:00\n"
        b"date: parsed date part: (Y-M-D) 2024-01-01\ndate: parsed time part: 12:00:00\n"
        b"date: input timezone: system default\n",
        b"",
        DATE_DEBUG,
    ),
}
