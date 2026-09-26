"""Option sweep, part C: seq, shuf, sleep, sort, split, stat, tac, tail, tee, touch, tr, truncate,
tsort, uniq, unlink, wc, yes, rev, which, file and printf (the builtin, `command printf` and
`env printf`).

Nondeterministic commands run only in deterministic forms: shuf with one candidate, `-n 0` or a
fixed --random-source; sleep 0; stat on times that `touch -d` set. Error paths fold GNU's curly
quotes into ASCII ones, as in part A.
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


# --- seq -----------------------------------------------------------------------------------------
for a in ["5", "3 7", "7 3", "1 3 10", "10 -3 1", "0.5 0.5 2", "1 0.1 1.3", "-2 2", "-- -3 -1", "1e1 12", "0x10 0x12", "1 1", "2 1 1",
          "1.50 3", "1.0 0.25 2", "-1 -0.5 0", "9 1 11", "5 -1 5", "1 10000000000 30000000000", "0 0.000001 0.000003", "18446744073709551615 18446744073709551617"]:
    add("seq", f"operands {a}", f"seq {a}")
for o in ["-w 8 11", "-w -- -5 5", "-w 1 0.5 2", "-s , 1 4", "-s '' 1 3", "-s '\\n\\t' 1 3", "--separator=: 1 3", "--equal-width 98 101",
          "-f %03g 8 11", "-f '%.2f' 1 0.5 2", "-f '%e' 1 2", "-f 'x%gy' 1 2", "-f '%%%g' 1 2", "--format=%5.1f 1 2", "-f %G 1e10 1e10",
          "-f %a 1 1", "-s, -w 9 11", "-f '%-5g|' 1 2"]:
    add("seq", f"option {o}", f"seq {o}")
add("seq", "zero increment", "seq 1 0 3", error=True)
add("seq", "invalid number", "seq a", error=True)
add("seq", "missing operand", "seq", error=True)
add("seq", "extra operand", "seq 1 2 3 4", error=True)
add("seq", "format without directive", "seq -f abc 1 2", error=True)
add("seq", "format integer directive", "seq -f %d 1 2", error=True)
add("seq", "format two directives", "seq -f '%g %g' 1 2", error=True)
add("seq", "equal width with format", "seq -w -f %g 1 2", error=True)
add("seq", "nan", "seq nan", error=True)
add("seq", "inf first", "seq inf 1; echo \"status=$?\"")
add("seq", "head of infinite", "seq 1 inf | head -n 3")
add("seq", "unknown option", "seq -q 1", error=True)
add("seq", "negative increment ascending", "seq 1 -1 3; echo \"status=$?\"")

# --- shuf ----------------------------------------------------------------------------------------
add("shuf", "count zero", "printf 'a\\nb\\n' | shuf -n 0; echo \"status=$?\"")
add("shuf", "single echo", "shuf -e only")
add("shuf", "single range", "shuf -i 5-5")
add("shuf", "repeat single", "shuf -r -n 3 -e x")
add("shuf", "repeat range single", "shuf -r -i 7-7 --head-count=2")
add("shuf", "empty input", "shuf </dev/null; echo \"status=$?\"")
add("shuf", "single line file", "printf 'one\\n' >/tmp/f; shuf /tmp/f")
add("shuf", "zero terminated single", "shuf -z -e a | od -An -c")
add("shuf", "output file", "shuf -e z -o /tmp/out; cat /tmp/out")
add("shuf", "sorted output is a permutation", "seq 1 20 | shuf | sort -n | tr '\\n' ' '; echo")
add("shuf", "count bound", "seq 1 20 | shuf -n 5 | wc -l")
add("shuf", "range permutation", "shuf -i 1-9 | sort | tr -d '\\n'; echo")
add("shuf", "random source", "printf '%0200d' 0 >/tmp/r; seq 1 5 | shuf --random-source=/tmp/r")
add("shuf", "random source range", "printf '%0200d' 7 >/tmp/r; shuf -i 1-6 --random-source=/tmp/r")
add("shuf", "invalid range", "shuf -i 5-1", error=True)
add("shuf", "range not numeric", "shuf -i a-b", error=True)
add("shuf", "invalid count", "shuf -n x -e a", error=True)
add("shuf", "echo and range", "shuf -e a -i 1-2", error=True)
add("shuf", "extra operand", "shuf /tmp/a /tmp/b", error=True)
add("shuf", "missing file", "shuf /tmp/nosuch", error=True)
add("shuf", "repeat without input", "shuf -r -n 2 </dev/null", error=True)
add("shuf", "random source missing", "shuf --random-source=/tmp/nosuch -e a", error=True)
add("shuf", "negative count", "shuf -n -1 -e a", error=True)
add("shuf", "count larger than input", "shuf -n 5 -e a")

# --- sleep ---------------------------------------------------------------------------------------
for a in ["0", "0.01", "0s", "0m", "0h", "0d", "0 0", ".001", "1e-3", "0x0", "0.0s 0.0m"]:
    add("sleep", f"interval {a}", f"sleep {a}; echo \"status=$?\"")
add("sleep", "invalid suffix", "sleep 1x", error=True)
add("sleep", "negative", "sleep -1", error=True)
add("sleep", "missing operand", "sleep", error=True)
add("sleep", "invalid among valid", "sleep 0 x", error=True)
add("sleep", "empty operand", "sleep ''", error=True)
add("sleep", "unknown option", "sleep --bogus", error=True)
add("sleep", "double dash", "sleep -- 0; echo \"status=$?\"")

# --- sort ----------------------------------------------------------------------------------------
S1 = r"printf 'banana\nApple\ncherry\napple\n10\n9\n 2\n-1\n' >/tmp/s; "
for o in ["", "-b", "-d", "-f", "-g", "-i", "-M", "-h", "-n", "-r", "-V", "-s", "-u", "-fu", "-nr", "-bn", "-df", "-rf", "--sort=numeric",
          "--sort=general-numeric", "--sort=human-numeric", "--sort=month", "--sort=version", "--ignore-case --unique", "-n -u",
          "--reverse --numeric-sort", "-c", "-C", "--check=quiet", "--check=diagnose-first", "--debug", "-S 1K", "--parallel=1", "--batch-size=2",
          "-T /tmp"]:
    add("sort", f"option {o or 'none'}", S1 + f"sort {o} /tmp/s; echo \"status=$?\"")
KS = r"printf 'b 2 x\na 10 y\nc 2 a\na 1 z\n' >/tmp/k; "
for k in ["-k2", "-k2n", "-k2,2n", "-k2,2n -k1,1", "-k2,2nr -k3", "-k1.2", "-k3,3 -k1,1r", "-k2n,2 -s", "-k 1,1 -u", "-k2,2 -t ' '",
          "-k1,1 -k2,2n -r", "-k2.1,2.1", "-k3b", "-k2,2g", "-k2h,2"]:
    add("sort", f"key {k}", KS + f"sort {k} /tmp/k")
add("sort", "field separator", "printf 'x:3\\ny:1\\nz:2\\n' | sort -t: -k2n")
add("sort", "field separator long", "printf 'x,3\\ny,1\\n' | sort --field-separator=, --key=2")
add("sort", "separator tab", "printf 'a\\t2\\nb\\t1\\n' | sort -t \"$(printf '\\t')\" -k2")
add("sort", "separator nul", "printf 'a\\0002\\nb\\0001\\n' | sort -t '\\0' -k2 | od -An -c")
add("sort", "human numbers", "printf '1K\\n2M\\n512\\n1G\\n3k\\n' | sort -h")
add("sort", "general numeric", "printf '1e3\\n-inf\\nnan\\n2.5\\n0x10\\n' | sort -g")
add("sort", "month names", "printf 'mar\\nJan\\n  feb\\nxyz\\nDEC\\n' | sort -M")
add("sort", "version sort", "printf 'a1.10\\na1.9\\na1.9a\\na1\\n' | sort -V")
add("sort", "dictionary order", "printf 'a-c\\nab\\na c\\n' | sort -d")
add("sort", "ignore nonprinting", "printf 'b\\na\\001c\\na\\n' | sort -i | od -An -c")
add("sort", "unique with keys", "printf 'a 1\\na 2\\nb 1\\n' | sort -u -k1,1")
add("sort", "stable", "printf 'b 1\\na 2\\nb 0\\na 1\\n' | sort -s -k1,1")
add("sort", "last resort comparison", "printf 'b 1\\na 2\\nb 0\\na 1\\n' | sort -k1,1")
add("sort", "zero terminated", "printf 'b\\0a\\0c\\0' | sort -z | od -An -c")
add("sort", "output file", "printf 'b\\na\\n' >/tmp/f; sort -o /tmp/f /tmp/f; cat /tmp/f")
add("sort", "output long", "printf 'b\\na\\n' | sort --output=/tmp/o; cat /tmp/o")
add("sort", "merge", "printf 'a\\nc\\n' >/tmp/1; printf 'b\\nd\\n' >/tmp/2; sort -m /tmp/1 /tmp/2")
add("sort", "merge unsorted input", "printf 'c\\na\\n' >/tmp/1; printf 'b\\n' >/tmp/2; sort -m /tmp/1 /tmp/2")
add("sort", "several files", "printf 'c\\n' >/tmp/1; printf 'a\\nb\\n' >/tmp/2; sort /tmp/1 /tmp/2")
add("sort", "files0 from", "printf 'c\\n' >/tmp/1; printf 'a\\n' >/tmp/2; printf '/tmp/1\\0/tmp/2\\0' >/tmp/l; sort --files0-from=/tmp/l")
add("sort", "check disorder message", "printf 'a\\nc\\nb\\n' | sort -c", error=True)
add("sort", "check unique", "printf 'a\\na\\n' | sort -cu", error=True)
add("sort", "check with file name", "printf 'b\\na\\n' >/tmp/f; sort -c /tmp/f", error=True)
add("sort", "invalid key", "sort -k 0 /dev/null", error=True)
add("sort", "invalid key field char", "sort -k 1.x /dev/null", error=True)
add("sort", "incompatible options", "sort -n -g /dev/null", error=True)
add("sort", "multi-char separator", "sort -t ab /dev/null", error=True)
add("sort", "missing file", "sort /tmp/nosuch", error=True)
add("sort", "check two files", "sort -c /dev/null /dev/null", error=True)
add("sort", "invalid buffer size", "sort -S x /dev/null", error=True)
add("sort", "invalid sort word", "sort --sort=bogus /dev/null", error=True)
add("sort", "random with source", "printf '%0100d' 0 >/tmp/r; printf 'a\\nb\\na\\n' | sort -R --random-source=/tmp/r | sort | uniq -c")
add("sort", "reverse unique keeps first", "printf 'A\\na\\nb\\n' | sort -f -u -r")
add("sort", "numeric with thousands", "printf '1,000\\n999\\n' | sort -n")
add("sort", "numeric decimals and signs", "printf '+1\\n-0.5\\n.5\\n-.5\\n0\\n' | sort -n")
add("sort", "blank lines", "printf 'b\\n\\na\\n\\n' | sort | od -An -c")
add("sort", "no trailing newline", "printf 'b\\na' | sort")
add("sort", "unknown option", "sort --bogus", error=True)
add("sort", "key with leading blanks", "printf 'x   b\\nx a\\n' | sort -k2")
add("sort", "key with b modifier", "printf 'x   b\\nx a\\n' | sort -k2b")
add("sort", "key end before start field", "printf 'a b\\nb a\\n' | sort -k2,1")
add("sort", "compress program refused or ignored", "printf 'b\\na\\n' | sort --compress-program=cat")

# --- split ---------------------------------------------------------------------------------------
SP = "mkdir /tmp/p; cd /tmp/p; seq 1 10 >in; "
LS = "; for f in x*; do printf '%s:' \"$f\"; tr '\\n' ' ' <\"$f\"; echo; done"
for o in ["-l 3", "-l3", "--lines=4", "-b 5", "-b 1K", "--bytes=7", "-C 6", "--line-bytes=10", "-n 3", "-n 2/3", "-n l/3", "-n l/2/3",
          "-n r/3", "-n r/2/3", "-a 3 -l 5", "-d -l 4", "--numeric-suffixes=5 -l 4", "-x -l 3", "--hex-suffixes=10 -l 4",
          "--additional-suffix=.txt -l 5", "-e -n 15", "-n 15", "-t , -l 2", "-u -n r/2", "-d -a 1 -l 5", "--verbose -l 5", "-l 100"]:
    add("split", f"option {o}", SP + f"split {o} in" + LS)
add("split", "prefix", SP + "split -l 5 in part-; ls")
add("split", "stdin", "mkdir /tmp/p; cd /tmp/p; seq 1 4 | split -l 2; ls")
add("split", "stdin dash with prefix", "mkdir /tmp/p; cd /tmp/p; seq 1 4 | split -l 2 - pre; ls")
add("split", "separator chunks", "mkdir /tmp/p; cd /tmp/p; printf 'a,b,c,d' | split -t , -l 2" + LS)
add("split", "nul separator", "mkdir /tmp/p; cd /tmp/p; printf 'a\\0b\\0c\\0' | split -t '\\0' -l 2; od -An -c xaa")
add("split", "chunk to stdout", SP + "split -n 2/2 in")
add("split", "round robin to stdout", SP + "split -n r/1/3 in")
add("split", "suffixes exhausted", SP + "split -a 1 -l 1 -b 1 in", error=True)
add("split", "suffixes too short", "mkdir /tmp/p; cd /tmp/p; seq 1 30 | split -a 1 -l 1", error=True)
add("split", "invalid lines", SP + "split -l 0 in", error=True)
add("split", "invalid bytes", SP + "split -b x in", error=True)
add("split", "invalid chunks", SP + "split -n 0 in", error=True)
add("split", "chunk out of range", SP + "split -n 4/3 in", error=True)
add("split", "two modes", SP + "split -l 2 -b 3 in", error=True)
add("split", "missing file", SP + "split nosuch", error=True)
add("split", "extra operand", SP + "split in a b", error=True)
add("split", "multi-char separator", SP + "split -t ab in", error=True)
add("split", "suffix with slash", SP + "split --additional-suffix=a/b in", error=True)
add("split", "filter", SP + "split -l 5 --filter='cat >$FILE.f' in; ls")
add("split", "empty input", "mkdir /tmp/p; cd /tmp/p; split </dev/null; ls; echo \"status=$?\"")

# --- stat ----------------------------------------------------------------------------------------
ST = ("mkdir -p /tmp/st/d; cd /tmp/st; printf 12345 >f; printf '' >e; ln -s f l; "
      "touch -d '2001-02-03 04:05:06.5 UTC' f; ")
for fmt in ["%n", "%N", "%s", "%F", "%Y", "%y", "%X", "%x", "%n:%s:%F", "%%", "%h", "[%10n]", "[%-10s]", "%.3Y", "%.9y", "%Hd", "%Lr", "%q"]:
    add("stat", f"format {fmt}", ST + f"TZ=UTC stat -c '{fmt}' f")
add("stat", "format directory type", ST + "stat -c %F d")
add("stat", "format empty file type", ST + "stat -c %F e")
add("stat", "format symlink", ST + "stat -c '%F %N' l")
add("stat", "dereference", ST + "stat -L -c '%F %s' l")
add("stat", "dereference long", ST + "stat --dereference -c '%F' l")
add("stat", "printf escapes", ST + "stat --printf='%n\\t%s\\n' f")
add("stat", "printf no newline", ST + "stat --printf='%s' f; echo")
add("stat", "format adds newline", ST + "stat -c '%s\\t' f | od -An -c")
add("stat", "several files", ST + "stat -c '%n %s' f e")
add("stat", "format long option", ST + "stat --format=%s f")
add("stat", "missing file", "stat /tmp/nosuch", error=True)
add("stat", "missing continues", ST + "stat -c %n f nosuch e", error=True)
add("stat", "missing operand", "stat", error=True)
add("stat", "unknown directive", ST + "stat -c '%j' f")
add("stat", "stdin dash", ST + "stat -c %F - <f")
add("stat", "unknown option", "stat --bogus", error=True)

# --- tac -----------------------------------------------------------------------------------------
add("tac", "file", "printf 'a\\nb\\nc\\n' >/tmp/f; tac /tmp/f")
add("tac", "no trailing newline", "printf 'a\\nb' | tac | od -An -c")
add("tac", "two files", "printf '1\\n2\\n' >/tmp/a; printf '3\\n4\\n' >/tmp/b; tac /tmp/a /tmp/b")
add("tac", "stdin dash", "printf '1\\n2\\n' | tac -")
add("tac", "empty lines", "printf '\\n\\na\\n' | tac | od -An -c")
add("tac", "missing file", "tac /tmp/nosuch", error=True)
add("tac", "separator", "printf 'a,b,c,' | tac -s ,", error=True)
add("tac", "before", "printf 'a\\nb\\n' | tac -b", error=True)
add("tac", "empty input", "tac </dev/null; echo \"status=$?\"")

# --- tail ----------------------------------------------------------------------------------------
TL = "seq 1 20 >/tmp/n; seq 21 23 >/tmp/m; "
for o in ["", "-n 3", "-n +18", "-n 0", "-n +0", "-c 6", "-c +50", "-c 0", "-3", "+18", "-2c", "--lines=2", "--bytes=3", "-n 1K", "-q", "-v",
          "-n 2 -v", "-n -2", "-c -4", "--silent", "--quiet", "-n1", "-c 1b"]:
    add("tail", f"option {o or 'none'}", TL + f"tail {o} /tmp/n")
add("tail", "two files", TL + "tail -n 1 /tmp/n /tmp/m")
add("tail", "two files quiet", TL + "tail -q -n 1 /tmp/n /tmp/m")
add("tail", "stdin", "seq 1 5 | tail -n 2")
add("tail", "stdin plus", "seq 1 5 | tail -n +4")
add("tail", "zero terminated", "printf 'a\\0b\\0c\\0' | tail -z -n 2 | od -An -c")
add("tail", "no trailing newline", "printf 'a\\nb\\nc' | tail -n 2 | od -An -c")
add("tail", "missing file", TL + "tail /tmp/nosuch /tmp/m", error=True)
add("tail", "invalid count", "tail -n x /tmp/n", error=True)
add("tail", "directory", "mkdir /tmp/d; tail /tmp/d", error=True)
add("tail", "retry without follow", TL + "tail --retry -n 1 /tmp/m", error=True)
add("tail", "sleep without follow", TL + "tail -s 1 -n 1 /tmp/m", error=True)
add("tail", "pid without follow", TL + "tail --pid=1 -n 1 /tmp/m", error=True)
add("tail", "huge count", TL + "tail -n 99999999999999999999 /tmp/m")
add("tail", "unknown option", "tail --bogus", error=True)
add("tail", "long lines", "printf '%0300d\\n%0300d\\n' 1 2 | tail -n 1 | wc -c")

# --- tee -----------------------------------------------------------------------------------------
add("tee", "files", "echo hi | tee /tmp/a /tmp/b; cat /tmp/a /tmp/b")
add("tee", "append", "echo 1 >/tmp/a; echo 2 | tee -a /tmp/a >/dev/null; cat /tmp/a")
add("tee", "append long", "echo 1 >/tmp/a; echo 2 | tee --append /tmp/a; cat /tmp/a")
add("tee", "ignore interrupts", "echo x | tee -i /tmp/a; cat /tmp/a")
add("tee", "no files", "echo x | tee")
add("tee", "dash is a file", "cd /tmp; echo x | tee -; ls")
add("tee", "missing directory continues", "echo x | tee /tmp/nosuch/a /tmp/b; echo \"status=$?\"; cat /tmp/b", error=True)
add("tee", "p option", "echo x | tee -p /tmp/a; cat /tmp/a")
add("tee", "output error warn", "echo x | tee --output-error=warn /tmp/a; cat /tmp/a")
add("tee", "output error exit", "echo x | tee --output-error=exit /tmp/a; cat /tmp/a")
add("tee", "output error invalid", "echo x | tee --output-error=bogus", error=True)
add("tee", "directory operand", "mkdir /tmp/d; echo x | tee /tmp/d", error=True)
add("tee", "unknown option", "tee --bogus </dev/null", error=True)
add("tee", "binary data", "printf '\\000\\377' | tee /tmp/a | od -An -tx1; od -An -tx1 /tmp/a")

# --- touch ---------------------------------------------------------------------------------------
Y = "; TZ=UTC stat -c '%y' /tmp/f"
for d in ["2001-02-03", "2001-02-03 04:05:06", "2001-02-03T04:05:06Z", "@1000000000", "2001-02-03 04:05:06.123456789 UTC", "Feb 3 2001 UTC",
          "2001-02-03 +1 day", "2001-02-03 04:05 +0200"]:
    add("touch", f"date {d}", f"TZ=UTC touch -d '{d}' /tmp/f" + Y)
for t in ["200102030405", "0102030405", "02030405", "200102030405.06", "02030405.59", "197001010000.00"]:
    add("touch", f"stamp {t}", f"TZ=UTC touch -t {t} /tmp/f" + Y)
add("touch", "creates", "touch /tmp/f; [ -f /tmp/f ] && wc -c </tmp/f")
add("touch", "no create", "touch -c /tmp/f; echo \"status=$?\"; ls /tmp")
add("touch", "no create long", "touch --no-create /tmp/f; ls /tmp")
add("touch", "reference", "touch -d @1000000000 /tmp/r; touch -r /tmp/r /tmp/f" + Y)
add("touch", "reference long", "touch -d @1000000000 /tmp/r; touch --reference=/tmp/r /tmp/f" + Y)
add("touch", "reference with date adjustment", "touch -d @1000000000 /tmp/r; touch -r /tmp/r -d '+1 hour' /tmp/f" + Y)
add("touch", "modification only", "touch -d @1000000000 /tmp/f; touch -m -d @2000000000 /tmp/f; TZ=UTC stat -c '%Y %X' /tmp/f")
add("touch", "access only", "touch -d @1000000000 /tmp/f; touch -a -d @2000000000 /tmp/f; TZ=UTC stat -c '%Y %X' /tmp/f")
add("touch", "time word", "touch -d @1000000000 /tmp/f; touch --time=mtime -d @3 /tmp/f; stat -c '%Y %X' /tmp/f")
add("touch", "time word access", "touch -d @1000000000 /tmp/f; touch --time=atime -d @3 /tmp/f; stat -c '%Y %X' /tmp/f")
add("touch", "f ignored", "touch -f /tmp/f; echo \"status=$?\"")
add("touch", "no dereference", "cd /tmp; touch -d @5 t; ln -s t l; touch -h -d @9 l; stat -c %Y t")
add("touch", "dereference by default", "cd /tmp; touch -d @5 t; ln -s t l; touch -d @9 l; stat -c %Y t")
add("touch", "several files", "touch -d @7 /tmp/a /tmp/b; stat -c '%n %Y' /tmp/a /tmp/b")
add("touch", "invalid date", "touch -d 'not a date' /tmp/f", error=True)
add("touch", "invalid stamp", "touch -t 20011 /tmp/f", error=True)
add("touch", "stamp bad seconds", "touch -t 200102030405.61 /tmp/f", error=True)
add("touch", "missing reference", "touch -r /tmp/nosuch /tmp/f", error=True)
add("touch", "missing operand", "touch", error=True)
add("touch", "missing directory", "touch /tmp/nosuch/f", error=True)
add("touch", "invalid time word", "touch --time=bogus /tmp/f", error=True)
add("touch", "date and stamp", "touch -d @1 -t 200101010000 /tmp/f", error=True)
add("touch", "obsolete stamp operand", "touch 01020304 /tmp/f; ls /tmp")
add("touch", "directory", "mkdir /tmp/d; touch -d @8 /tmp/d; stat -c %Y /tmp/d")

# --- tr ------------------------------------------------------------------------------------------
TR = "printf 'Hello, World! 123\\tTabs\\n  spaced  out  \\n' | tr "
for a in ["a-z A-Z", "'[:lower:]' '[:upper:]'", "'[:upper:]' '[:lower:]'", "-d '[:digit:]'", "-d '[:alpha:]'", "-d '[:alnum:]'", "-d '[:space:]'",
          "-d '[:blank:]'", "-d '[:punct:]'", "-d '[:cntrl:]'", "-d '[:print:]'", "-d '[:graph:]'", "-d '[:xdigit:]'", "-s ' '", "-s '[:space:]'",
          "-c '[:alnum:]' _", "-cd '[:alpha:]\\n'", "-cs '[:alpha:]' '\\n'", "-ds l ' '", "-t abcdefgh 12", "abcdef 12", "lo 'x*'",
          "lo '[x*]'", "'lo' '[x*1]y'", "a-e '[=a=]'", "'[=l=]' L", "'\\t' T", "'\\n' ' '", "' ' '\\012'", "'\\054' ';'", "-s 'l'",
          "-s lo LO", "o-l x", "'H-L' 'h-l'", "-d ', !'", "'[:upper:][:digit:]' 'X'", "-C '[:alpha:]' '.'", "--delete l", "--squeeze-repeats ' '",
          "--complement --delete 'a-z\\n'", "--truncate-set1 'lo' 'x'", "'\\\\' x", "'a\\-z' _", "-- -a _", "'[a*3]' x", "0-9 '[#*]'",
          "'[:lower:]' '[:lower:]'", "-d 'a-z' A-Z", "'\\101' x", "'\\x41' x"]:
    add("tr", f"arguments {a}", TR + a)
add("tr", "missing operand", "echo x | tr", error=True)
add("tr", "missing second set", "echo x | tr a", error=True)
add("tr", "extra operand", "echo x | tr a b c", error=True)
add("tr", "delete with two sets", "echo x | tr -d a b", error=True)
add("tr", "invalid class", "echo x | tr '[:bogus:]' x", error=True)
add("tr", "reversed range", "echo x | tr z-a x", error=True)
add("tr", "class in set2", "echo x | tr a '[:digit:]'", error=True)
add("tr", "upper to digit", "echo x | tr '[:upper:]' '[:digit:]'", error=True)
add("tr", "repeat in set1", "echo x | tr '[a*]' x", error=True)
add("tr", "two repeats in set2", "echo x | tr ab '[x*][y*]'", error=True)
add("tr", "empty set2", "echo x | tr a ''", error=True)
add("tr", "equivalence multiple chars", "echo x | tr '[=ab=]' x", error=True)
add("tr", "trailing backslash", "echo 'a\\' | tr '\\' x; echo \"status=$?\"")
add("tr", "multibyte input", "printf '\u00e9t\u00e9\\n' | tr t T")
add("tr", "multibyte set", "printf '\u00e9t\u00e9\\n' | tr '\u00e9' e | od -An -c")
add("tr", "file operand rejected", "printf 'x\\n' >/tmp/f; tr a b /tmp/f", error=True)
add("tr", "nul bytes", "printf 'a\\0b\\0' | tr '\\0' '\\n'")
add("tr", "high bytes", "printf '\\377\\200' | tr '\\377' 'x' | od -An -c")
add("tr", "set1 longer than set2", "echo abcd | tr abcd xy")
add("tr", "unknown option", "echo x | tr --bogus a b", error=True)

# --- truncate ------------------------------------------------------------------------------------
TC = "printf 'abcdefghij' >/tmp/f; "
for s in ["0", "4", "20", "+5", "-3", "<5", "<20", ">5", ">20", "/4", "%4", "1K", "1KB", "2k", "0x10"]:
    add("truncate", f"size {s}", TC + f"truncate -s '{s}' /tmp/f; wc -c </tmp/f")
add("truncate", "size long", TC + "truncate --size=3 /tmp/f; cat /tmp/f; echo")
add("truncate", "extends with zeros", TC + "truncate -s 12 /tmp/f; od -An -c /tmp/f")
add("truncate", "creates", "truncate -s 3 /tmp/new; wc -c </tmp/new")
add("truncate", "no create", "truncate -c -s 3 /tmp/new; echo \"status=$?\"; ls /tmp")
add("truncate", "reference", TC + "printf abc >/tmp/r; truncate -r /tmp/r /tmp/f; cat /tmp/f; echo")
add("truncate", "reference relative", TC + "printf abc >/tmp/r; truncate -r /tmp/r -s +2 /tmp/f; wc -c </tmp/f")
add("truncate", "several files", TC + "cp /tmp/f /tmp/g; truncate -s 2 /tmp/f /tmp/g; cat /tmp/f /tmp/g; echo")
add("truncate", "missing size", TC + "truncate /tmp/f", error=True)
add("truncate", "invalid size", TC + "truncate -s x /tmp/f", error=True)
add("truncate", "missing operand", "truncate -s 1", error=True)
add("truncate", "missing reference", TC + "truncate -r /tmp/nosuch /tmp/f", error=True)
add("truncate", "missing directory", "truncate -s 1 /tmp/nosuch/f", error=True)
add("truncate", "directory operand", "mkdir /tmp/d; truncate -s 1 /tmp/d", error=True)
add("truncate", "negative absolute", TC + "truncate -s -20 /tmp/f; wc -c </tmp/f")
add("truncate", "division by zero", TC + "truncate -s /0 /tmp/f", error=True)
add("truncate", "io blocks", TC + "truncate -o -s 0 /tmp/f; wc -c </tmp/f")

# --- tsort ---------------------------------------------------------------------------------------
add("tsort", "pairs", "printf 'a b\\nb c\\na d\\n' | tsort")
add("tsort", "self pair", "printf 'a a\\nb b\\n' | tsort")
add("tsort", "cycle", "printf 'a b\\nb a\\n' | tsort", error=True)
add("tsort", "cycle with others", "printf 'x y\\na b\\nb c\\nc a\\n' | tsort", error=True)
add("tsort", "odd tokens", "echo a b c | tsort", error=True)
add("tsort", "file operand", "printf '1 2\\n2 3\\n' >/tmp/t; tsort /tmp/t")
add("tsort", "missing file", "tsort /tmp/nosuch", error=True)
add("tsort", "extra operand", "tsort /tmp/a /tmp/b", error=True)
add("tsort", "tokens across lines", "printf 'a\\nb c\\nd\\n' | tsort")
add("tsort", "empty input", "tsort </dev/null; echo \"status=$?\"")
add("tsort", "tabs and spaces", "printf 'a\\t\\tb\\n  b   c\\n' | tsort")

# --- uniq ----------------------------------------------------------------------------------------
U = r"printf 'a 1\na 1\nA 1\nb 2\nb 3\nc x\nc x\nc x\n' >/tmp/u; "
for o in ["", "-c", "-d", "-D", "-u", "-i", "-ic", "-f 1", "-f1 -c", "-s 1", "-s 2 -c", "-w 1", "-w 1 -c", "-f 1 -w 1", "-dc", "-u -c",
          "--all-repeated=separate", "--all-repeated=prepend", "--all-repeated=none", "--group", "--group=prepend", "--group=append", "--group=both",
          "--group=separate -i", "--count --repeated", "--skip-fields=1 --unique", "--check-chars=1 -D", "-i -D", "-s 10", "-f 5"]:
    add("uniq", f"option {o or 'none'}", U + f"uniq {o} /tmp/u")
add("uniq", "output file", U + "uniq /tmp/u /tmp/o; cat /tmp/o")
add("uniq", "stdin dash", U + "uniq - </tmp/u")
add("uniq", "zero terminated", "printf 'a\\0a\\0b\\0' | uniq -z | od -An -c")
add("uniq", "no trailing newline", "printf 'a\\na' | uniq -c")
add("uniq", "group with count", U + "uniq --group -c /tmp/u", error=True)
add("uniq", "all repeated with count", U + "uniq -D -c /tmp/u", error=True)
add("uniq", "invalid skip", U + "uniq -f x /tmp/u", error=True)
add("uniq", "invalid group", U + "uniq --group=bogus /tmp/u", error=True)
add("uniq", "missing file", "uniq /tmp/nosuch", error=True)
add("uniq", "extra operand", U + "uniq /tmp/u /tmp/o /tmp/p", error=True)
add("uniq", "blank fields", "printf ' a\\n\\ta\\n' | uniq -f 1 -c")
add("uniq", "empty input", "uniq </dev/null; echo \"status=$?\"")
add("uniq", "obsolete skip", U + "uniq -1 /tmp/u", error=True)

# --- unlink --------------------------------------------------------------------------------------
add("unlink", "file", "printf x >/tmp/f; unlink /tmp/f; ls /tmp")
add("unlink", "symlink", "cd /tmp; printf x >f; ln -s f l; unlink l; ls")
add("unlink", "directory", "mkdir /tmp/d; unlink /tmp/d", error=True)
add("unlink", "missing operand", "unlink", error=True)
add("unlink", "extra operand", "unlink /tmp/a /tmp/b", error=True)
add("unlink", "missing file", "unlink /tmp/nosuch", error=True)

# --- wc ------------------------------------------------------------------------------------------
W = "printf 'one two\\nthree  four five\\n\\n\u00e9t\u00e9 \\tsix' >/tmp/w; printf 'a\\n' >/tmp/v; "
for o in ["", "-c", "-m", "-l", "-w", "-L", "-lw", "-cm", "-lwcmL", "--bytes", "--chars", "--lines", "--words", "--max-line-length"]:
    add("wc", f"option {o or 'none'}", W + f"wc {o} /tmp/w")
for t in ["auto", "always", "only", "never"]:
    add("wc", f"total {t}", W + f"wc --total={t} /tmp/w /tmp/v")
    add("wc", f"total {t} one file", W + f"wc -l --total={t} /tmp/w")
add("wc", "two files", W + "wc /tmp/w /tmp/v")
add("wc", "stdin", W + "wc </tmp/w")
add("wc", "stdin dash with file", W + "cat /tmp/v | wc -l - /tmp/w")
add("wc", "files0 from", W + "printf '/tmp/w\\0/tmp/v\\0' >/tmp/l; wc -l --files0-from=/tmp/l")
add("wc", "files0 from stdin", W + "printf '/tmp/v\\0' | wc -c --files0-from=-")
add("wc", "files0 with operand", W + "printf '/tmp/v\\0' >/tmp/l; wc --files0-from=/tmp/l /tmp/w", error=True)
add("wc", "missing file", W + "wc /tmp/nosuch /tmp/v", error=True)
add("wc", "directory", "mkdir /tmp/d; wc /tmp/d", error=True)
add("wc", "invalid total", "wc --total=bogus </dev/null", error=True)
add("wc", "invalid utf8", "printf 'a\\377b c\\n' | wc -mwc")
add("wc", "max line with tabs", "printf 'a\\tb\\n' | wc -L")
add("wc", "max line multibyte", "printf '\u00e9\u00e9\u00e9\\n\u4e2d\u6587\\n' | wc -L")
add("wc", "empty input", "wc </dev/null")
add("wc", "words across nonprinting", "printf 'a\\001b c\\n' | wc -w")
add("wc", "unknown option", "wc --bogus", error=True)
add("wc", "debug", "printf 'x\\n' | wc --debug -l", error=True)

# --- yes -----------------------------------------------------------------------------------------
add("yes", "default", "yes | head -n 2")
add("yes", "words", "yes a b | head -n 2")
add("yes", "empty word", "yes '' | head -n 2 | od -An -c")
add("yes", "help", "yes --help | head -n 1")
add("yes", "version status", "yes --version >/dev/null; echo \"status=$?\"")
add("yes", "double dash", "yes -- --help | head -n 1")
add("yes", "unknown option", "yes --bogus | head -n 1", error=True)
add("yes", "long line", "yes \"$(printf '%0100d' 0)\" | head -n 3 | wc -c")

# --- rev -----------------------------------------------------------------------------------------
add("rev", "stdin", "printf 'abc\\nxy\\n' | rev")
add("rev", "file", "printf 'hello\\n' >/tmp/f; rev /tmp/f")
add("rev", "two files", "printf 'ab\\n' >/tmp/a; printf 'cd\\n' >/tmp/b; rev /tmp/a /tmp/b")
add("rev", "no trailing newline", "printf 'abc' | rev; echo")
add("rev", "multibyte", "printf '\u00e9ta\\n' | rev")
add("rev", "empty lines", "printf '\\n\\nab\\n' | rev | od -An -c")
add("rev", "tabs", "printf 'a\\tb\\n' | rev | od -An -c")
add("rev", "stdin dash", "printf 'ab\\n' | rev -")

# --- which ---------------------------------------------------------------------------------------
add("which", "missing command", "which nosuchcommand; echo \"status=$?\"")

# --- file ----------------------------------------------------------------------------------------
FI = "mkdir -p /tmp/fi/d; cd /tmp/fi; "
for label, make in [("empty", "printf ''"), ("ascii text", "printf 'hello\\n'"), ("utf8 text", "printf '\u00e9t\u00e9\\n'"),
                    ("crlf text", "printf 'a\\r\\nb\\r\\n'"), ("no trailing newline", "printf 'abc'"), ("shell script", "printf '#!/bin/sh\\necho hi\\n'"),
                    ("bash script", "printf '#!/bin/bash\\necho hi\\n'"), ("python script", "printf '#!/usr/bin/env python3\\nprint(1)\\n'"),
                    ("json", "printf '{\"a\": [1, 2]}\\n'"), ("binary data", "printf '\\000\\001\\002\\377'"), ("gzip magic", "printf '\\037\\213\\010\\000\\000\\000\\000\\000\\000\\003'"),
                    ("png magic", "printf '\\211PNG\\r\\n\\032\\n'"), ("pdf magic", "printf '%%PDF-1.4\\n'"), ("zip magic", "printf 'PK\\003\\004'"),
                    ("elf magic", "printf '\\177ELF\\002\\001\\001'"), ("html", "printf '<!DOCTYPE html>\\n<html></html>\\n'"),
                    ("xml", "printf '<?xml version=\"1.0\"?>\\n<a/>\\n'"), ("long lines", "printf '%0400d\\n' 0"), ("latin1 text", "printf 'caf\\351\\n'"),
                    ("utf16 bom", "printf '\\377\\376a\\000'"), ("utf8 bom", "printf '\\357\\273\\277hi\\n'"), ("c source", "printf '#include <stdio.h>\\nint main(void) { return 0; }\\n'")]:
    add("file", label, FI + make + " >f; file f")
add("file", "directory", FI + "file d")
add("file", "symlink", FI + "printf x >t; ln -s t l; file l")
add("file", "symlink follow", FI + "printf 'x\\n' >t; ln -s t l; file -L l")
add("file", "dangling symlink", FI + "ln -s nosuch l; file l")
add("file", "brief", FI + "printf 'hi\\n' >f; file -b f")
add("file", "mime", FI + "printf 'hi\\n' >f; file -i f")
add("file", "mime type", FI + "printf 'hi\\n' >f; file --mime-type f")
add("file", "mime type brief", FI + "printf '{}\\n' >f; file -b --mime-type f")
add("file", "mime encoding", FI + "printf 'hi\\n' >f; file --mime-encoding f")
add("file", "missing file", FI + "file nosuch; echo \"status=$?\"")
add("file", "several", FI + "printf '' >e; printf 'x\\n' >t; file e t d")
add("file", "no operand", "file", error=True)
add("file", "stdin dash", "printf 'hello\\n' | file -")
add("file", "separator", FI + "printf 'x\\n' >t; file -F '|' t")
add("file", "no pad", FI + "printf 'x\\n' >t; printf '' >longername; file -N t longername")
add("file", "names from file", FI + "printf 'x\\n' >t; printf 't\\nd\\n' >list; file -f list")

# --- printf (builtin, command printf and env printf) ---------------------------------------------
# `hex float` uses 0: %a of any other value depends on the CPU's long double (0x8p-3 on x86-64,
# 0x1p+0 on arm64), so the oracle's answer would depend on the machine that recorded it.
PF = [("decimal", "'%d\\n' 42 -7"), ("integer i", "'%i\\n' 010 0x1f"), ("octal", "'%o\\n' 8"), ("unsigned", "'%u\\n' 3"),
      ("hex", "'%x %X\\n' 255 255"), ("alt hex", "'%#x %#o\\n' 255 8"), ("float", "'%f\\n' 3.14159"), ("float F", "'%F\\n' 2.5"),
      ("exponent", "'%e %E\\n' 12345.678 0.00012"), ("general", "'%g %G\\n' 0.0001 1e20"), ("hex float", "'%a\\n' 0"),
      ("char", "'%c|\\n' hello"), ("string", "'%s|\\n' a 'b c'"), ("width", "'[%5s][%-5s]\\n' ab cd"), ("precision string", "'[%.2s]\\n' abcdef"),
      ("star width", "'[%*d]\\n' 6 42"), ("star precision", "'[%.*f]\\n' 2 3.14159"), ("negative star width", "'[%*d]\\n' -6 42"),
      ("zero pad", "'%05d\\n' 42"), ("plus flag", "'%+d %+d\\n' 5 -5"), ("space flag", "'% d|\\n' 5"), ("precision int", "'%.3d\\n' 7"),
      ("percent", "'100%%\\n'"), ("reuse format", "'%s=%s\\n' a 1 b 2 c"), ("missing args", "'%s|%d|%f\\n'"), ("char constant", "'%d %d\\n' \"'A\" '\"z'"),
      ("b escapes", "'%b\\n' 'a\\tb\\\\n'"), ("b octal", "'%b\\n' '\\0101\\101'"), ("b stop", "'%b-%s\\n' 'x\\cy' z; echo"), ("q quoting", "'%q\\n' 'a b' \"it's\" ''"),
      ("escape sequences", "'\\a\\b\\f\\r\\v\\e' | od -An -c"), ("octal escape", "'\\101\\60\\n'"), ("hex escape", "'\\x41\\x4a\\n'"),
      ("unicode escape", "'\\u00e9\\U0001F600\\n'"), ("backslash c in format", "'a\\cb'; echo"), ("unknown escape", "'\\q\\n'"),
      ("time format epoch", "'%(%Y-%m-%d)T\\n' 0"), ("time format default", "'[%(%H)T]\\n' 3600"), ("invalid number", "'%d\\n' abc"),
      ("partial number", "'%d\\n' 12abc"), ("float invalid", "'%f\\n' x"), ("overflow", "'%d\\n' 99999999999999999999"),
      ("negative unsigned", "'%u\\n' -1"), ("invalid directive", "'%y\\n' 1"), ("no format", ""), ("dash dash", "-- '%s\\n' x"),
      ("format looks like option", "-x"), ("trailing percent", "'abc%'"), ("large precision", "'%.30f\\n' 1"), ("hex input float", "'%f\\n' 0x10"),
      ("inf and nan", "'%f %f\\n' inf nan"), ("string with width star missing", "'[%*s]\\n' 3"), ("length modifiers", "'%ld %hd %lld\\n' 1 2 3"),
      ("big hex", "'%x\\n' 18446744073709551615"), ("alternate g", "'%#g\\n' 1"), ("left zero", "'%-05d|\\n' 3"), ("c of number", "'%c\\n' 65"),
      ("b with percent", "'%b\\n' '100%'"), ("q with newline", "'%q\\n' \"$(printf 'a\\nb')\""), ("Q precision", "'%.3Q\\n' abcdef")]
for label, a in PF:
    add("printf", f"builtin {label}", f"printf {a}; echo \" status=$?\"")
for label, a in PF[:12]:
    add("printf", f"command {label}", f"command printf {a}; echo \" status=$?\"")
for label, a in [("decimal", "'%d\\n' 42"), ("string", "'%s|\\n' a 'b c'"), ("q quoting", "'%q\\n' 'a b'"), ("b escapes", "'%b\\n' 'a\\tb'"),
                 ("invalid number", "'%d\\n' abc"), ("hex escape", "'\\x41\\n'"), ("missing args", "'%s|%d\\n'"), ("no format", "")]:
    add("printf", f"env {label}", f"env printf {a}; echo \" status=$?\"")
add("printf", "v assigns", "printf -v x '%05d' 42; echo \"[$x]\"")
add("printf", "v array element", "printf -v a[2] '%s' hi; echo \"${a[2]}\"")
add("printf", "v reuse format", "printf -v x '%s,' a b c; echo \"$x\"")
add("printf", "v invalid name", "printf -v 1x '%s' a; echo \"status=$?\"")
add("printf", "v missing name", "printf -v; echo \"status=$?\"")
add("printf", "v with b stop", "printf -v x 'a%bz' 'b\\cc'; echo \"[$x]\"")
add("printf", "v keeps trailing newlines", "printf -v x 'a\\n\\n'; echo \"${#x}\"")
add("printf", "v in function local", "f() { local x; printf -v x %s in; echo \"$x\"; }; f; echo \"[${x-unset}]\"")
add("printf", "v nameref", "declare -n r=t; printf -v r %s via; echo \"$t\"")
add("printf", "v readonly", "readonly x=1; printf -v x %s 2; echo \"status=$? x=$x\"")
add("printf", "builtin redirect", "printf '%s\\n' a >/tmp/p; cat /tmp/p")
add("printf", "builtin in pipeline", "printf '%s\\n' c a b | sort")
add("printf", "unknown option", "printf -z x; echo \"status=$?\"")
add("printf", "command v rejected", "command printf -v x y; echo \"status=$? [${x-unset}]\"")
