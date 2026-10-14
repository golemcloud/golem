"""Option sweep, part E: find, xargs, diff, cmp and patch, option by option, with error paths.

Trees are built inside each script. Output that GNU leaves in directory order is sorted, and
time predicates use times `touch -d` set far in the past, so no case depends on the clock.
Error paths fold GNU's curly quotes into ASCII ones, as in part A.
"""

TIER = "sweep"
CASES = []


def err(script):
    return "{ " + script + "; } 2>/tmp/err; echo \"status=$?\"; sed \"s/[‘’]/'/g\" /tmp/err"


def add(cmd, label, script, error=False):
    tags = [f"cmd.{cmd}"]
    if error:
        tags += [f"cmd.{cmd}.error", "error"]
        script = err(script)
    CASES.append((f"opt {cmd}: {label}", script, tags))


# --- find ----------------------------------------------------------------------------------------
TREE = ("mkdir -p /tmp/t/a/b /tmp/t/c /tmp/t/.h /tmp/t/empty; cd /tmp/t; printf 12345 >a/one.txt; printf '' >a/b/two.TXT; "
        "printf 123 >c/three.log; printf 'x' >.h/four; printf '%02000d' 0 >big; ln -s a/one.txt lnk; ln -s nosuch dangling; "
        "ln -s a dirlink; touch -d 2000-01-01 a/one.txt; touch -d 2010-01-01 c/three.log; ")
S = " | sort"
for e in ["", "-name '*.txt'", "-iname '*.txt'", "-name '*.[tT][xX][tT]'", "-path '*/b/*'", "-ipath '*/B/*'", "-wholename './a/*'", "-regex '.*/t[a-z]*\\.log'",
          "-iregex '.*TXT'", "-regextype posix-extended -regex '.*/(one|two)\\..*'", "-regextype egrep -regex '.*o{2}.*'", "-type f", "-type d", "-type l",
          "-type f,l", "-xtype l", "-xtype f", "-xtype d", "-empty", "-empty -type f", "-type f -size 0", "-type f -size +1k", "-type f -size -2", "-type f -size 5c", "-type f -size +4c -size -6c",
          "-type f -size 1k", "-type f -size -1M", "-type f -size +0", "-maxdepth 1", "-mindepth 2", "-maxdepth 1 -mindepth 1", "-maxdepth 0", "-depth", "-d", "-name a -prune -o -print",
          "-path ./a -prune -o -type f -print", "-not -name '*.txt'", "! -type d", "-type f -a -name '*o*'", "-type f -o -type l", "\\( -name one.txt -o -name four \\)",
          "-name '*.txt' , -name '*.log'", "-true", "-false", "-name one.txt -quit", "-print -quit", "-type f -print0 | sort -z | tr '\\0' ' '; echo",
          "-newer c/three.log", "! -newer c/three.log -type f", "-mtime +1000", "-mtime -1000 -type f", "-mmin +100000", "-mtime +8000 -mtime -9000",
          "-newermt 2005-01-01 -type f", "! -newermt 2005-01-01 -type f", "-anewer c/three.log -type f", "-lname 'a*'", "-ilname 'A*'", "-follow -type f",
          "-L . -type f", "-H lnk", "-L lnk", "-P lnk -type l", "-xdev -type f", "-mount -name four", "-noleaf -name four", "-ignore_readdir_race -name four",
          "-nowarn -name four", "-warn -name four", "-daystart -mtime +1000", "-name '[ab]*'", "-name '\\*'", "-name 'o*' -o -name 't*' -type f",
          "-type d -empty", "-type d -name '.*'", "-name '.*'", "-path './.h*'"]:
    add("find", f"expression {e or 'none'}", TREE + "find . " + e + S)
for fmt in ["%p\\n", "%f\\n", "%h\\n", "%P\\n", "%s %p\\n", "%d %p\\n", "%y %p\\n", "%Y %p\\n", "%l\\n", "%H|%P\\n", "%-10f|\\n", "%10s|\\n",
            "%TY-%Tm-%Td\\n", "%T@\\n", "%t\\n", "%A@\\n", "%TF %TT\\n", "%%\\n", "%p\\t%s\\0", "\\101\\n", "[%q]\\n", "%p",
            "\\c%p"]:
    add("find", f"printf {fmt}", TREE + f"find . -name '*.txt' -printf '{fmt}' | od -c | head -n 4")
add("find", "several starting points", TREE + "find a c -type f" + S)
add("find", "starting point is file", TREE + "find a/one.txt")
add("find", "trailing slash start", TREE + "find a/ -type f")
add("find", "absolute start", TREE + "find /tmp/t/c")
add("find", "no start defaults to dot", TREE + "find -name four")
add("find", "sorted siblings", TREE + "find c a -maxdepth 1")
add("find", "exec semicolon", TREE + "find . -name '*.txt' -exec echo got {} \\;" + S)
add("find", "exec plus", TREE + "find . -type f -name '*.txt' -exec echo {} + | tr ' ' '\\n'" + S)
add("find", "exec plus with trailing", TREE + "find . -name one.txt -exec echo {} end \\;")
add("find", "exec status as test", TREE + "find . -type f -exec grep -q 1 {} \\; -print" + S)
add("find", "exec embedded braces", TREE + "find . -name four -exec echo x{}y \\;")
add("find", "execdir", TREE + "find . -name one.txt -execdir pwd \\;")
add("find", "execdir plus", TREE + "find . -name '*.txt' -execdir echo {} + " + S)
add("find", "exec sh", TREE + "find . -name '*.log' -exec sh -c 'wc -c <\"$1\"' _ {} \\;")
add("find", "exec function", TREE + "f() { echo fn; }; find . -name four -exec f {} \\;", error=True)
add("find", "exec missing command", TREE + "find . -name four -exec nosuchcmd {} \\;", error=True)
add("find", "exec plus nonzero", TREE + "find . -name four -exec false {} +; echo \"status=$?\"")
add("find", "exec missing terminator", TREE + "find . -exec echo {}", error=True)
add("find", "exec plus not last", TREE + "find . -exec echo {} x +", error=True)
add("find", "delete", TREE + "find . -name '*.log' -delete; find . -type f" + S)
add("find", "delete directory tree", TREE + "find a -delete; ls")
add("find", "delete nonempty with prune", TREE + "find . -name c -delete", error=True)
add("find", "fprint", TREE + "find . -name '*.txt' -fprint /tmp/out; sort /tmp/out")
add("find", "fprint0", TREE + "find . -name four -fprint0 /tmp/out; od -An -c /tmp/out")
add("find", "fprintf", TREE + "find . -name four -fprintf /tmp/out '%f!\\n'; cat /tmp/out")
add("find", "print0", TREE + "find c -print0 | od -An -c")
add("find", "missing start", TREE + "find nosuch", error=True)
add("find", "missing start continues", TREE + "find nosuch c", error=True)
add("find", "unknown predicate", TREE + "find . -bogus", error=True)
add("find", "missing argument", TREE + "find . -name", error=True)
add("find", "invalid type", TREE + "find . -type q", error=True)
add("find", "invalid size", TREE + "find . -size x", error=True)
add("find", "invalid maxdepth", TREE + "find . -maxdepth -1", error=True)
add("find", "invalid mtime", TREE + "find . -mtime x", error=True)
add("find", "unbalanced parenthesis", TREE + "find . \\( -name a", error=True)
add("find", "stray closing parenthesis", TREE + "find . -name a \\)", error=True)
add("find", "invalid regex", TREE + "find . -regex '['", error=True)
add("find", "invalid regextype", TREE + "find . -regextype bogus -regex x", error=True)
add("find", "newer missing reference", TREE + "find . -newer nosuch", error=True)
add("find", "option after test warns", TREE + "find . -name four -maxdepth 1", error=True)
add("find", "path with trailing slash warns", TREE + "find . -name 'a/'", error=True)
add("find", "dangling symlink with L", TREE + "find -L . -name dangling", error=True)
add("find", "loop with L", "mkdir -p /tmp/l/d; cd /tmp/l/d; ln -s .. up; cd /tmp/l; find -L . | sort", error=True)
add("find", "operator without operand", TREE + "find . -o -name x", error=True)
add("find", "comma list prints both", TREE + "find c -name '*.log' -printf 'A %f\\n' , -printf 'B %f\\n'")
add("find", "prune with depth", TREE + "find . -depth -name a -prune" + S)
add("find", "quit status", TREE + "find . -quit; echo \"status=$?\"")
add("find", "dot dot start", TREE + "cd a; find .. -maxdepth 1 -name c")
add("find", "name with slash never matches", TREE + "find . -name 'a/one.txt'", error=True)
add("find", "empty name", TREE + "find . -name ''")
add("find", "size units", TREE + "find . -type f -size -1b" + S)
add("find", "newerXY variants", TREE + "find . -newerat 2005-01-01 -type f" + S)
add("find", "newer reference symlink", TREE + "find . -type f -newer lnk" + S)

# --- xargs ---------------------------------------------------------------------------------------
for label, cmd in [
    ("default echo", "printf 'a b\\nc\\n' | xargs"), ("command", "printf 'a b\\nc\\n' | xargs echo X"),
    ("max args", "seq 1 5 | xargs -n 2 echo"), ("max args long", "seq 1 5 | xargs --max-args=3 echo"),
    ("max lines", "printf 'a b\\nc\\nd e f\\n' | xargs -L 1 echo"), ("max lines two", "printf 'a\\nb\\nc\\n' | xargs -L 2 echo"),
    ("max lines trailing blank", "printf 'a \\nb\\nc\\n' | xargs -L 1 echo"), ("l option", "printf 'a\\nb\\n' | xargs -l echo"),
    ("replace", "printf 'a\\nb\\n' | xargs -I{} echo '<{}>'"), ("replace custom", "printf 'a\\nb\\n' | xargs -I % echo %-%"),
    ("replace long", "printf 'a\\nb\\n' | xargs --replace=X echo X."), ("i default", "printf 'a\\nb\\n' | xargs -i echo '[{}]'"),
    ("replace keeps spaces", "printf 'a  b\\n' | xargs -I{} echo '[{}]'"), ("replace strips leading blanks", "printf '  a b\\n' | xargs -I{} echo '[{}]'"),
    ("null", "printf 'a b\\0c\\0' | xargs -0 -n 1 echo"), ("null long", "printf 'x\\0y\\0' | xargs --null echo"),
    ("delimiter", "printf 'a,b,c' | xargs -d , -n 1 echo"), ("delimiter newline", "printf 'a b\\nc\\n' | xargs -d '\\n' -n 1 echo"),
    ("delimiter escape", "printf 'a\\tb' | xargs -d '\\t' echo"), ("delimiter octal", "printf 'a:b' | xargs -d '\\072' echo"),
    ("eof string", "printf 'a\\nSTOP\\nb\\n' | xargs -E STOP echo"), ("eof option", "printf 'a\\n_\\nb\\n' | xargs -e_ echo"),
    ("eof empty", "printf 'a\\n_\\nb\\n' | xargs -e echo"), ("no run if empty", "printf '' | xargs -r echo ran; echo \"status=$?\""),
    ("run once if empty", "printf '' | xargs echo ran"), ("verbose", "printf 'a\\n' | xargs -t echo 2>&1"),
    ("verbose quoting", "printf 'a b\\n' | xargs -0 -t echo 2>&1"), ("arg file", "printf 'x y\\n' >/tmp/a; xargs -a /tmp/a echo"),
    ("arg file long", "printf 'x\\n' >/tmp/a; xargs --arg-file=/tmp/a echo got"), ("max chars", "seq 1 10 | xargs -s 12 echo"),
    ("max chars exit", "seq 1 10 | xargs -x -s 8 -n 3 echo"), ("max procs", "seq 1 4 | xargs -P 2 -n 1 echo | sort"),
    ("quotes", "printf '\"a b\" c\\n' | xargs -n 1 echo"), ("single quotes", "printf \"'a b' c\\n\" | xargs -n 1 echo"),
    ("backslash", "printf 'a\\\\ b c\\n' | xargs -n 1 echo"), ("unmatched quote", "printf '\"a b\\n' | xargs echo"),
    ("exit 255 stops", "seq 1 3 | xargs -n 1 sh -c 'echo $0; exit 255'; echo \"status=$?\""),
    ("failure status 123", "seq 1 3 | xargs -n 1 sh -c 'exit 1'; echo \"status=$?\""),
    ("command not found", "echo a | xargs nosuchcmd; echo \"status=$?\""), ("function not visible", "f() { echo fn; }; echo a | xargs f; echo \"status=$?\""),
    ("builtin echo", "echo a | xargs echo -n; echo"), ("shell builtin cd", "echo /tmp | xargs cd; echo \"status=$?\""),
    ("exported variable", "export V=1; echo a | xargs sh -c 'echo $V $0'"), ("default command echo options", "printf -- '-n\\n' | xargs; echo"),
    ("stdin of command", "echo a | xargs sh -c 'cat; echo end'"), ("bash c with args", "printf 'a\\nb\\n' | xargs bash -c 'echo \"$@\"' _"),
    ("replace with max args", "printf 'a\\nb\\n' | xargs -I{} -n 1 echo {}"), ("zero input with replace", "printf '' | xargs -I{} echo {}; echo \"status=$?\""),
    ("empty lines", "printf '\\n\\na\\n\\n' | xargs echo"), ("long line", "printf '%0500d\\n' 0 | xargs echo | wc -c"),
]:
    add("xargs", label, cmd)
add("xargs", "invalid max args", "echo a | xargs -n 0 echo", error=True)
add("xargs", "invalid max lines", "echo a | xargs -L x echo", error=True)
add("xargs", "invalid max chars", "echo a | xargs -s 0 echo", error=True)
# The ceiling GNU names is 128 KiB less what the environment takes, which differs between hosts.
add(
    "xargs",
    "max chars too large",
    "echo a | xargs -s 99999999999 echo 2>/tmp/e; st=$?; "
    "sed 's/should be <= [0-9]*/should be <= N/' /tmp/e >&2; (exit $st)",
    error=True,
)
add("xargs", "invalid delimiter", "echo a | xargs -d ab echo", error=True)
add("xargs", "missing arg file", "xargs -a /tmp/nosuch echo", error=True)
add("xargs", "unknown option", "xargs --bogus", error=True)
add("xargs", "exit when too long", "echo aaaaaaaaaa | xargs -x -s 5 echo", error=True)
add("xargs", "nul with quotes kept", "printf '\"a\"\\0' | xargs -0 echo")

# --- diff ----------------------------------------------------------------------------------------
D = (r"printf 'a\nb\nc\nd\ne\nf\ng\n' >/tmp/1; printf 'a\nB\nc\nd\ne\nf\ng\nh\n' >/tmp/2; "
     "touch -d '2001-02-03 04:05:06.5 UTC' /tmp/1; touch -d '2002-03-04 05:06:07 UTC' /tmp/2; ")
for o in ["", "--normal", "-q", "--brief", "-s", "-c", "-C 1", "--context=0", "-u", "-U 0", "-U1", "--unified=5", "-e", "--ed", "-n", "--rcs",
          "-i", "--ignore-case", "-t", "-T", "--tabsize=4 -t",
          "-u --label A --label B", "-c --label X --label Y", "-D SYM", "--ifdef=X", "-a", "--text", "-d", "--minimal", "--horizon-lines=1", "--speed-large-files",
          "--color=never", "-p -u", "-F '^[a-c]' -u", "--suppress-blank-empty -u", "-u --strip-trailing-cr",
          "--line-format='%L'", "--old-line-format='-%l\n' --new-line-format='+%l\n' --unchanged-line-format=' %l\n'", "--changed-group-format='[%<|%>]'",
          "--unchanged-group-format='' --changed-group-format='%dF-%dL %dn\n'", "-I '^[bB]$'", "--ignore-matching-lines='^h'", "-u -I h", "-l"]:
    add("diff", f"option {o or 'none'}", D + "diff " + o + " /tmp/1 /tmp/2; echo \"status=$?\"")
W = r"printf 'a b\n c\nx\t \nd\n' >/tmp/1; printf 'a  b\nc\nx\nd\n' >/tmp/2; "
for o in ["", "-b", "-w", "-Z", "-E", "--ignore-space-change", "--ignore-all-space", "--ignore-trailing-space", "--ignore-tab-expansion", "-bZ"]:
    add("diff", f"whitespace {o or 'none'}", W + "diff " + o + " /tmp/1 /tmp/2; echo \"status=$?\"")
add("diff", "identical", "echo a >/tmp/1; echo a >/tmp/2; diff /tmp/1 /tmp/2; echo \"status=$?\"")
add("diff", "report identical", "echo a >/tmp/1; echo a >/tmp/2; diff -s /tmp/1 /tmp/2")
add("diff", "no newline at end", "printf 'a' >/tmp/1; printf 'a\\n' >/tmp/2; diff /tmp/1 /tmp/2; diff -u /tmp/1 /tmp/2 | tail -n 3")
add("diff", "empty files", "printf '' >/tmp/1; printf 'x\\n' >/tmp/2; diff -u /tmp/1 /tmp/2 | tail -n 2")
add("diff", "stdin dash", "echo a >/tmp/1; echo b | diff /tmp/1 -")
add("diff", "binary files", "printf 'a\\000' >/tmp/1; printf 'b\\000' >/tmp/2; diff /tmp/1 /tmp/2")
add("diff", "binary files brief", "printf 'a\\000' >/tmp/1; printf 'b\\000' >/tmp/2; diff -q /tmp/1 /tmp/2")
add("diff", "file and directory", "mkdir /tmp/d; echo a >/tmp/d/f; echo b >/tmp/f; diff /tmp/f /tmp/d; echo \"status=$?\"")
add("diff", "directory and file", "mkdir /tmp/d; echo a >/tmp/d/f; echo b >/tmp/f; diff /tmp/d /tmp/f; echo \"status=$?\"")
DR = "mkdir -p /tmp/x/s /tmp/y/s /tmp/x/only; echo 1 >/tmp/x/a; echo 2 >/tmp/y/a; echo s >/tmp/x/s/f; echo t >/tmp/y/s/f; echo n >/tmp/y/new; echo same >/tmp/x/c; echo same >/tmp/y/c; find /tmp/x /tmp/y -exec touch -d '2001-01-01 UTC' {} +; "
for o in ["", "-r", "-q", "-rq", "-rN", "-ru", "-r --unidirectional-new-file", "-r -x s", "-r --exclude='*a'", "-r -S c", "-rs", "-r --no-dereference",
          "--from-file=/tmp/x/a /tmp/y/a /tmp/x/c", "--to-file=/tmp/y/a /tmp/x/a /tmp/x/c", "-r --ignore-file-name-case"]:
    add("diff", f"directories {o or 'none'}", DR + "diff " + o + " /tmp/x /tmp/y" * ("-file" not in o) + "; echo \"status=$?\"")
add("diff", "exclude from", DR + "echo s >/tmp/ex; diff -r -X /tmp/ex /tmp/x /tmp/y; echo \"status=$?\"")
add("diff", "missing file", "echo a >/tmp/1; diff /tmp/1 /tmp/nosuch", error=True)
add("diff", "missing file new-file", "echo a >/tmp/1; diff -N /tmp/1 /tmp/nosuch; echo \"status=$?\"")
add("diff", "missing operand", "diff /tmp/1", error=True)
add("diff", "extra operand", "diff a b c", error=True)
add("diff", "unknown option", "diff --bogus a b", error=True)
add("diff", "invalid context", "diff -C x /tmp/1 /tmp/2", error=True)
add("diff", "conflicting formats", "echo a >/tmp/1; echo b >/tmp/2; touch -d '2001-01-01 UTC' /tmp/1 /tmp/2; diff -u -c /tmp/1 /tmp/2", error=True)
add("diff", "invalid width", "diff -y -W x /tmp/1 /tmp/2", error=True)
add("diff", "invalid regex", "echo a >/tmp/1; echo b >/tmp/2; diff -I '[' /tmp/1 /tmp/2", error=True)
add("diff", "both stdin", "echo a | diff - -; echo \"status=$?\"")
add("diff", "large context", D + "diff -U 100 /tmp/1 /tmp/2 | tail -n +3")
add("diff", "unified hunk merge", "seq 1 20 >/tmp/1; seq 1 20 | sed 's/^5$/five/;s/^9$/nine/;s/^18$/x/' >/tmp/2; diff -u /tmp/1 /tmp/2 | tail -n +3")
add("diff", "context headers labels", D + "diff -c --label=old --label=new /tmp/1 /tmp/2 | head -n 3")
add("diff", "ifdef with changes", "printf 'a\\nb\\n' >/tmp/1; printf 'a\\nc\\n' >/tmp/2; diff -DNEW /tmp/1 /tmp/2")
add("diff", "ed script applies", D + "diff -e /tmp/1 /tmp/2 >/tmp/ed; cat /tmp/ed")
add("diff", "crlf", "printf 'a\\r\\n' >/tmp/1; printf 'a\\n' >/tmp/2; diff /tmp/1 /tmp/2 | od -An -c")

# --- cmp -----------------------------------------------------------------------------------------
C = r"printf 'abcdef\nline2\n' >/tmp/1; printf 'abXdef\nline2 more\n' >/tmp/2; cp /tmp/1 /tmp/3; "
for o in ["", "-b", "-l", "-lb", "-s", "--silent", "--quiet", "-n 2", "-n 3", "--bytes=10", "-i 3", "-i 3:4", "--ignore-initial=1", "-i 1k", "-l -n 5",
          "--print-bytes", "--verbose", "-z"]:
    add("cmp", f"option {o or 'none'}", C + "cmp " + o + " /tmp/1 /tmp/2; echo \"status=$?\"")
add("cmp", "identical", C + "cmp /tmp/1 /tmp/3; echo \"status=$?\"")
add("cmp", "skip operands", C + "cmp /tmp/1 /tmp/2 3 3; echo \"status=$?\"")
add("cmp", "skip operand suffix", C + "cmp /tmp/1 /tmp/2 0 1K; echo \"status=$?\"")
add("cmp", "eof on shorter", "printf 'ab' >/tmp/1; printf 'abc' >/tmp/2; cmp /tmp/1 /tmp/2", error=True)
add("cmp", "eof on empty", "printf '' >/tmp/1; printf 'a' >/tmp/2; cmp /tmp/1 /tmp/2", error=True)
add("cmp", "eof verbose", "printf 'ab' >/tmp/1; printf 'aXc' >/tmp/2; cmp -l /tmp/1 /tmp/2", error=True)
add("cmp", "stdin dash", C + "cat /tmp/1 | cmp - /tmp/3; echo \"status=$?\"")
add("cmp", "one operand reads stdin", C + "cmp /tmp/1 </tmp/2", error=True)
add("cmp", "missing file", C + "cmp /tmp/1 /tmp/nosuch", error=True)
add("cmp", "missing file silent", C + "cmp -s /tmp/1 /tmp/nosuch; echo \"status=$?\"")
add("cmp", "missing operand", "cmp", error=True)
add("cmp", "invalid skip", C + "cmp -i x /tmp/1 /tmp/2", error=True)
add("cmp", "invalid bytes", C + "cmp -n x /tmp/1 /tmp/2", error=True)
add("cmp", "unknown option", "cmp --bogus a b", error=True)
add("cmp", "directory", "mkdir /tmp/d; cmp /tmp/d /tmp/d", error=True)
add("cmp", "help", "cmp --help | head -n 1")
add("cmp", "print bytes high", "printf 'a\\377' >/tmp/1; printf 'a\\200' >/tmp/2; cmp -b /tmp/1 /tmp/2; echo \"status=$?\"")
add("cmp", "extra operand", C + "cmp /tmp/1 /tmp/2 1 2 3", error=True)
add("cmp", "silent and verbose", C + "cmp -s -l /tmp/1 /tmp/2", error=True)

# --- patch ---------------------------------------------------------------------------------------
PT = ("mkdir -p /tmp/p; cd /tmp/p; printf 'one\\ntwo\\nthree\\nfour\\nfive\\n' >f; printf 'one\\nTWO\\nthree\\nfour\\nfive\\nsix\\n' >g; "
      "touch -d '2001-01-01 UTC' f g; diff -u f g >u.diff; diff -c f g >c.diff; diff f g >n.diff; cp f orig; ")
for o in ["< u.diff", "-i u.diff", "--input=u.diff", "< c.diff", "-c < c.diff", "-u < u.diff", "f < n.diff", "-n f < n.diff", "--dry-run < u.diff",
          "-s < u.diff", "--silent < u.diff", "--verbose < u.diff", "-b < u.diff", "-b -z .old < u.diff", "--backup --suffix=.bk < u.diff",
          "-B pre_ < u.diff", "-o out < u.diff", "-o - < u.diff", "-r rej < u.diff", "-E < u.diff", "-t < u.diff", "-f < u.diff", "--posix < u.diff",
          "-l < u.diff", "-F 0 < u.diff", "--binary < u.diff", "-V numbered -b < u.diff", "--quoting-style=literal < u.diff", "-p0 < u.diff",
          "-p1 < u.diff", "-D FOO < u.diff", "--merge < u.diff", "-Z < u.diff", "--reject-format=unified < u.diff", "--read-only=ignore < u.diff",
          "--no-backup-if-mismatch < u.diff", "--backup-if-mismatch < u.diff", "-Y bk/ -b < u.diff", "-e f < n.diff", "-g 0 < u.diff"]:
    add("patch", f"option {o}", PT + "patch " + o + "; echo \"status=$?\"; cat f; ls")
add("patch", "reverse", PT + "patch <u.diff >/dev/null; patch -R <u.diff; cat f")
add("patch", "already applied detected", PT + "patch <u.diff >/dev/null; patch <u.diff; echo \"status=$?\"; ls")
add("patch", "forward ignores applied", PT + "patch <u.diff >/dev/null; patch -N <u.diff; echo \"status=$?\"; ls")
add("patch", "forward batch", PT + "patch <u.diff >/dev/null; patch -N -t <u.diff; echo \"status=$?\"")
add("patch", "offset", PT + "printf 'zero\\n' | cat - orig >f; patch <u.diff; cat f")
add("patch", "fuzz", PT + "printf 'one\\ntwo\\nthree\\nFOUR\\nfive\\n' >f; patch <u.diff; echo \"status=$?\"; cat f")
add("patch", "fuzz zero fails", PT + "printf 'one\\ntwo\\nthree\\nFOUR\\nfive\\n' >f; patch -F 0 <u.diff; echo \"status=$?\"; cat f.rej")
add("patch", "rejected hunk", PT + "printf 'x\\ny\\nz\\n' >f; patch <u.diff; echo \"status=$?\"; ls; cat f.rej")
add("patch", "strip components", "mkdir -p /tmp/p/a /tmp/p/b; cd /tmp/p; printf 'x\\n' >a/f; printf 'y\\n' >b/f; diff -u a/f b/f >d; printf 'x\\n' >f; mkdir -p c; printf 'x\\n' >c/f; patch -p1 -d c <d; cat c/f")
add("patch", "directory option", PT + "mkdir sub; cp orig sub/f; patch -d sub <u.diff; cat sub/f")
add("patch", "directory missing", PT + "patch -d nosuch <u.diff", error=True)
add("patch", "file operand and patch file", PT + "cp orig h; patch h u.diff; cat h")
add("patch", "creates new file", "mkdir -p /tmp/p; cd /tmp/p; printf 'new\\n' >n; diff -u /dev/null n >d; rm n; patch <d; cat n")
add("patch", "removes emptied file", "mkdir -p /tmp/p; cd /tmp/p; printf 'x\\n' >e; printf '' >e2; diff -u e e2 >d; patch -E e <d; ls")
add("patch", "git style headers", "mkdir -p /tmp/p; cd /tmp/p; printf 'a\\n' >f; printf -- '--- a/f\\n+++ b/f\\n@@ -1 +1 @@\\n-a\\n+b\\n' >d; patch -p1 <d; cat f")
add("patch", "missing target file", "mkdir -p /tmp/p; cd /tmp/p; printf -- '--- x\\n+++ x\\n@@ -1 +1 @@\\n-a\\n+b\\n' >d; patch -t <d", error=True)
add("patch", "garbage input", "mkdir -p /tmp/p; cd /tmp/p; echo garbage | patch", error=True)
add("patch", "empty input", "mkdir -p /tmp/p; cd /tmp/p; patch </dev/null", error=True)
add("patch", "malformed hunk", "mkdir -p /tmp/p; cd /tmp/p; printf 'a\\n' >f; printf -- '--- f\\n+++ f\\n@@ -1 +1 @@\\n' >d; patch <d", error=True)
add("patch", "missing patch file", "patch -i /tmp/nosuch", error=True)
add("patch", "unknown option", "patch --bogus </dev/null", error=True)
add("patch", "invalid strip", PT + "patch -p x <u.diff", error=True)
add("patch", "two hunks", "mkdir -p /tmp/p; cd /tmp/p; seq 1 20 >f; seq 1 20 | sed 's/^2$/two/;s/^19$/nineteen/' >g; diff -u f g >d; patch <d; sed -n '2p;19p' f")
add("patch", "no newline marker", "mkdir -p /tmp/p; cd /tmp/p; printf 'a' >f; printf 'b' >g; diff -u f g >d; patch <d; od -An -c f")
add("patch", "multiple files in one patch", "mkdir -p /tmp/p/o /tmp/p/n; cd /tmp/p; echo 1 >o/a; echo 2 >n/a; echo 3 >o/b; echo 4 >n/b; diff -ru o n >d; cd o; patch -p1 <../d; cat a b")
add("patch", "dry run leaves file", PT + "patch --dry-run <u.diff >/dev/null; cmp f orig && echo unchanged")
add("patch", "reverse unapplied", PT + "patch -R -f <u.diff; echo \"status=$?\"", error=True)
add("patch", "binary refused", "mkdir -p /tmp/p; cd /tmp/p; printf 'a\\000' >f; printf 'b\\000' >g; diff -a -u f g >d; patch <d; echo \"status=$?\"")
add("patch", "context with offset", PT + "printf 'zero\\n' | cat - orig >f; patch -c <c.diff; cat f")
add("patch", "output to stdout keeps file", PT + "patch -o - f <u.diff >/tmp/o; cmp f orig && cat /tmp/o")
