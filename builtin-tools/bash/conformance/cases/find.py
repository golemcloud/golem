"""find conformance: GNU findutils 4.10 in the oracle is the reference for status, stdout and stderr.

`-newerXt` dates go through uutils' `parse_datetime`, so GNU's grammar (relative dates, month
names, compact dates, offsets, `@EPOCH`) is compared here too.

GNU walks directories in readdir order, so every case listing more than one sibling sorts its output.
"""

TREE = (
    "mkdir -p /tmp/f/src/lib /tmp/f/docs /tmp/f/empty && cd /tmp/f && "
    "printf 'fn main() {}\\n' > src/main.rs && "
    "printf 'pub fn x() {}\\n' > src/lib/mod.rs && "
    ": > src/blank.txt && "
    "printf '# Title\\n' > docs/README.md && "
    "printf 'notes\\n' > docs/notes.txt && "
    "printf '%1500s' x > big.bin && "
    "printf 'upper\\n' > Upper.TXT"
)
CHAIN = "mkdir -p /tmp/d/a/b/c && : > /tmp/d/a/b/c/leaf"
AGES = (
    "mkdir -p /tmp/age && cd /tmp/age && "
    "touch -d '2020-01-01 00:00:00' old && touch -d '2021-06-01 12:00:00' mid && "
    # Minutes old rather than brand new: a Docker VM clock slightly behind the host makes a new
    # file look like it is from the future, and GNU's -mtime 0 then misses it.
    "touch -d '5 minutes ago' new"
)
STAMP = "mkdir -p /tmp/s && touch -d '2020-03-04 05:06:07' /tmp/s/stamp"

CASES = [
    ("find: whole tree", f"{TREE}; find . | sort"),
    ("find: name glob", f"{TREE}; find . -name '*.txt' | sort"),
    ("find: iname", f"{TREE}; find . -iname '*.txt' | sort"),
    ("find: name bracket and escape", f"{TREE}; find . -name '[!a-m]*.rs' | sort; find . -name '\\*' | sort"),
    ("find: type f and d", f"{TREE}; find . -type d | sort; find . -type f | sort"),
    ("find: type list", f"{TREE}; find src -type d,f | sort"),
    ("find: depth limits", f"{TREE}; find . -mindepth 1 -maxdepth 1 | sort; find . -mindepth 3 | sort"),
    ("find: maxdepth zero with several starts", f"{TREE}; find . src docs/ -maxdepth 0"),
    ("find: default start", f"{TREE}; find -maxdepth 1 -name src"),
    ("find: trailing slash start", f"{TREE}; find /tmp/f/src/ -maxdepth 1 -name '*.rs'"),
    ("find: path glob", f"{TREE}; find . -path './src/*' -name '*.rs' | sort"),
    ("find: ipath and wholename", f"{TREE}; find . -ipath '*DOCS*' | sort; find . -wholename './docs/notes.txt'"),
    ("find: or with parentheses", f"{TREE}; find . \\( -name '*.md' -o -name '*.rs' \\) -type f | sort"),
    ("find: negation", f"{TREE}; find . -type f ! -name '*.rs' | sort; find src -not -type d | sort"),
    ("find: and binds tighter than or", f"{TREE}; find . -name '*.md' -o -name '*.rs' -print | sort"),
    ("find: explicit and", f"{TREE}; find . -type f -a -name 'm*' -and -path '*src*' | sort"),
    ("find: comma operator", f"{TREE}; find . -maxdepth 1 -name src -printf 'A %p\\n' , -name docs -printf 'B %p\\n' | sort"),
    ("find: true and false", f"{TREE}; find . -maxdepth 0 -false -o -true; find . -maxdepth 0 -false"),
    ("find: prune", f"{TREE}; find . -name src -prune -o -type f -print | sort"),
    ("find: prune is ignored with depth", f"{TREE}; find . -depth -name lib -prune | sort"),
    ("find: preorder", f"{CHAIN}; find /tmp/d"),
    ("find: depth order", f"{CHAIN}; find /tmp/d -depth; find /tmp/d -d -mindepth 2"),
    ("find: quit", f"{CHAIN}; find /tmp/d -name b -print -quit; find /tmp/d -quit; echo status=$?"),
    ("find: empty", f"{TREE}; find . -empty | sort"),
    ("find: size units round up", f"{TREE}; find . -type f -size +1k; find . -type f -size 2k; find . -type f -size 3; find . -type f -size 1500c"),
    ("find: size below one unit", f"{TREE}; find . -type f -size -1M | sort; find . -type f -size -2 | sort"),
    ("find: newer", f"{AGES}; find . -type f -newer mid | sort; find . -type f -newer old | sort"),
    ("find: mtime and mmin", f"{AGES}; find . -type f -mtime +30 | sort; find . -type f -mtime -1; find . -type f -mmin -60; find . -type f -mtime 0"),
    ("find: newermt", f"{AGES}; find . -type f -newermt '2020-06-01' | sort; find . -type f ! -newermt '2021-06-01 12:00:01' | sort"),
    # GNU's date grammar (parse_datetime): relative dates, month names, compact dates, offsets.
    ("find: newermt relative dates", f"{AGES}; find . -type f -newermt yesterday; find . -type f -newermt '1 year ago'; find . -type f -newermt now; find . -type f -newermt 'next monday'; echo end"),
    ("find: newermt other date forms", f"{AGES}; find . -type f -newermt 'Jun 1 2020' | sort; find . -type f -newermt 20200601 | sort; find . -type f -newermt '2020-06-01 12:00:00 +0200' | sort; find . -type f -newermt @1590969600 | sort"),
    ("find: newermt unreadable date", "find . -newermt xyz; echo status=$?"),
    ("find: regex default dialect", f"{TREE}; find . -regex '.*/[a-z]+\\.rs' | sort; find . -regex '.*\\.\\(md\\|txt\\)' | sort"),
    ("find: regex literal operators", f"{TREE}; find . -regex './src' ; find . -regex 'src'; find . -regex '.*a{{2}}'"),
    ("find: regextype posix-extended", f"{TREE}; find . -regextype posix-extended -regex '.*\\.(md|txt)' | sort"),
    ("find: iregex", f"{TREE}; find . -iregex '.*readme.*'"),
    ("find: print0", f"{TREE}; find . -name '*.rs' -print0 | sort -z | tr '\\0' '\\n'"),
    ("find: printf path directives", f"{TREE}; find . -maxdepth 1 -printf '%p|%f|%h|%P|%H|%d|%y\\n' | sort; find src/ -maxdepth 0 -printf '%p|%f|%P\\n'"),
    ("find: printf widths", f"{TREE}; find . -type f -printf '[%5s] [%-10f] [%.3f]\\n' | sort"),
    ("find: printf time", f"{STAMP}; find /tmp/s/stamp -printf '%t|%TY-%Tm-%Td %TH:%TM:%TS|%T@|%Tc|%TD|%Tj|%Ta %Tb %Te|%Ts|%T+\\n'"),
    ("find: printf escapes", f"{TREE}; find . -maxdepth 0 -printf 'a\\tb\\\\\\101\\n'; find . -maxdepth 0 -printf 'x\\cy\\n'; echo"),
    ("find: printf unknown directive warns", f"{TREE}; find . -maxdepth 0 -printf '%z|\\q\\n'; echo status=$?"),
    ("find: printf percent at end", f"{TREE}; find . -maxdepth 0 -printf 'x%'; echo status=$?"),
    ("find: fprint", f"{TREE}; find . -name '*.rs' -fprint /tmp/out.txt; sort /tmp/out.txt; find . -maxdepth 0 -fprint /dev/stdout"),
    ("find: exec per file", f"{TREE}; find . -name '*.rs' -exec echo found {{}} \\; | sort"),
    ("find: exec batch", f"{TREE}; find src -name '*.rs' -exec echo {{}} + | tr ' ' '\\n' | sort"),
    ("find: exec status is the test", f"{TREE}; find . -type f -exec grep -q main {{}} \\; -print"),
    ("find: exec substitutes inside words", f"{TREE}; find . -name main.rs -exec echo {{}}.bak \\;"),
    ("find: exec awkward names", "mkdir -p /tmp/q && : > '/tmp/q/a b' && : > \"/tmp/q/it's\" && : > '/tmp/q/$x'; find /tmp/q -type f -exec echo {} \\; | sort"),
    ("find: execdir", f"{TREE}; find . -name '*.rs' -execdir echo {{}} \\; | sort; find src -name '*.rs' -execdir echo {{}} + | sort"),
    ("find: exec command not found", f"{TREE}; find . -maxdepth 0 -exec nosuchcmd {{}} \\;; echo status=$?; find . -maxdepth 0 -exec nosuchcmd {{}} +; echo status=$?"),
    ("find: exec failure in batch", f"{TREE}; find . -maxdepth 0 -exec false {{}} +; echo status=$?; find . -maxdepth 0 -exec false {{}} \\; ; echo status=$?"),
    ("find: delete", f"{TREE}; find . -name '*.txt' -delete; find . -type f | sort"),
    ("find: delete nonempty directory", f"{TREE}; find . -name docs -delete; echo status=$?"),
    ("find: missing start point", f"{TREE}; find /tmp/nope . -maxdepth 0; echo status=$?; find ''; echo status=$?"),
    ("find: unknown predicate", "find . -foo; echo status=$?"),
    ("find: paths must precede expression", "find -type f /tmp; echo status=$?; find -print /tmp; echo status=$?"),
    ("find: missing argument", "find . -name; echo status=$?; find . -exec echo; echo status=$?"),
    ("find: operator errors", "find . -o; echo $?; find . -name x -o; echo $?; find . !; echo $?; find . \\( \\); echo $?; find . -name x \\); echo $?; find . \\( -name x; echo $?"),
    ("find: type errors", "find . -type x; echo $?; find . -type ff; echo $?; find . -type f,; echo $?"),
    ("find: numeric argument errors", "find . -size 1q; echo $?; find . -mtime x; echo $?; find . -maxdepth -1; echo $?"),
    ("find: regex errors", "find . -regex 'a\\(b'; echo $?; find . -regextype nope -regex x; echo $?"),
    ("find: exec plus errors", "find . -exec echo {} {} +; echo $?; find . -exec echo {}x +; echo $?"),
    ("find: delete with prune", "find . -name x -prune -o -delete; echo $?"),
    ("find: head of a large tree", "mkdir -p /tmp/big && for i in {1..300}; do : > /tmp/big/f$i; done; find /tmp/big -type f | head -n 1 | wc -l; find /tmp/big | head -n 3 | wc -l"),
    # Both are refused before find ever runs, so the tree they'd walk doesn't matter to our own
    # output -- but it matters to the oracle side `stale` checks: unscoped, `.` here is whatever
    # ambient directory the harness runs cases in, which reaches /proc, so a real `find` walking
    # it leaks PIDs and the host's own file list into a "stable" golden. Scoped to TREE instead.
    ("find: refuses perm", f"{TREE}; find . -perm 644; echo status=$?"),
    ("find: refuses owner printf", f"{TREE}; find . -printf '%u\\n'; echo status=$?"),
]

EXPECTED_REASON = "WASI has no permission bits or owners, so these predicates are refused"
EXPECTED = {
    "find: refuses perm": (0, b"status=2\n", b"find: -perm is unsupported in bash-tool\n"),
    "find: refuses owner printf": (0, b"status=2\n", b"find: -printf %u is unsupported in bash-tool\n"),
}

EXPECTED_STDERR = {}
