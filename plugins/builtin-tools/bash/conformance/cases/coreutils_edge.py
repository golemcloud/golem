"""Coreutils edge cases: arithmetic, format directives, the locale, huge widths and non-regular inputs.

Every case name starts with "coreutils: ".
"""

CASES = [
    # wc column widths.
    ("coreutils: wc single file no padding", "printf 'a\\nb\\nc\\n' > f; wc -l f; wc -l f | cut -d' ' -f1"),
    ("coreutils: wc single file width from its own counts, not byte size", "for i in 1 2 3 4 5; do printf '%01000d\\n' 0; done > f; wc -l f; wc -l f | cut -d' ' -f1"),
    ("coreutils: wc redirected stdin sizes from its own counts", "for i in 1 2 3 4 5; do printf '%01000d\\n' 0; done > f; wc < f; wc -l < f; wc -lc < f"),
    ("coreutils: wc true pipe still widens to 7", "for i in 1 2 3 4 5; do printf '%01000d\\n' 0; done | wc; for i in 1 2 3 4 5; do printf '%01000d\\n' 0; done | wc -l"),
    ("coreutils: wc multi-file widens by summed byte size", "printf 'a\\nb\\nc\\n' > f; printf 'd\\ne\\n' > g; wc -l f g"),
    ("coreutils: wc mixed file and stdin widens like GNU", "printf 'a\\nb\\nc\\n' > f; printf 'x\\n' | wc -l - f"),
    ("coreutils: wc files0-from zero-length entry does not widen the other rows", "cd /tmp; echo hi > a.txt; printf 'a.txt\\0\\0' > list0; wc --files0-from=list0; echo s=$?"),

    # seq arithmetic.
    ("coreutils: seq with an infinite end streams until head stops it", "seq 1 inf | head -n 2; echo s=$?; seq 5 inf | head -n 2"),
    ("coreutils: seq with a huge end streams", "seq 1 1e20 | head -n 2"),
    ("coreutils: seq large integer increment does not overshoot", "seq 1 10000000000 30000000000"),
    ("coreutils: seq format with no directive is an error", "seq -f x 1 2; echo s=$?"),
    ("coreutils: seq rejects nan", "seq nan 1 3; echo s=$?; seq 1 nan 3; echo s=$?"),
    ("coreutils: seq long-form separator and format take a space-separated argument", "seq --separator , 1 3; seq --format %g 1 3"),

    # find -mtime/-mmin boundaries. Not the exact N*86400s boundary itself here: that
    # instant is inherently racy against real wall-clock time (GNU exhibits the same raciness
    # there -- confirmed by hand against the oracle), so the corpus sticks to the two sides
    # that are always deterministic.
    ("coreutils: find -mtime excludes a file well under N days old", "cd /tmp && touch -d '1 hour ago' fresh; find . -maxdepth 1 -name fresh -mtime -2; find . -maxdepth 1 -name fresh -mtime 2; find . -maxdepth 1 -name fresh -mtime +1"),
    ("coreutils: find -mtime matches a file well over N days old", "cd /tmp && touch -d '3 days ago' old; find . -maxdepth 1 -name old -mtime -2; find . -maxdepth 1 -name old -mtime 2; find . -maxdepth 1 -name old -mtime +1"),

    # which resolves every in-process command (this crate's own and Brush's builtins
    # that stand in for a real program), and ./ paths against the shell's cwd.
    ("coreutils: which finds this crate's own commands and Brush builtins that are real programs", "which sed; echo s=$?; which cat ls; echo s=$?; which echo; echo s=$?; which cd; echo s=$?"),
    ("coreutils: which with no operands is an error", "which; echo s=$?"),

    # xargs and find -exec must not run shell-language builtins.
    ("coreutils: xargs does not run shell builtins", "echo /tmp | xargs cd; echo \"s=$? pwd=$(pwd)\"; echo X=1 | xargs export; echo \"s=$? X=${X-unset}\""),
    ("coreutils: find -exec does not run shell builtins", "cd /tmp && find /tmp -maxdepth 0 -exec cd {} \\; ; echo \"find s=$? pwd=$(pwd)\""),
    ("coreutils: xargs and find -exec still run this crate's own commands", "echo /tmp | xargs ls -d; cd /tmp && find /tmp -maxdepth 0 -exec echo {} \\;"),
    ("coreutils: xargs and find -exec still run echo (a real GNU program, not a shell builtin)", "printf 'a b\\nc\\n' | xargs echo; echo s=$?; cd /tmp && find /tmp -maxdepth 0 -exec echo hi {} \\;"),

    # cp onto a symlink of the same file.
    ("coreutils: cp refuses a symlink of the same file instead of emptying it", "cd /tmp; printf important-data > f; ln -s f l; cp f l; echo s=$?; cat f"),
    ("coreutils: cp refuses two symlinks to the same file", "cd /tmp; printf data > t; ln -s t a; ln -s t b; cp a b; echo s=$?; cat t"),
    ("coreutils: cp still copies a genuinely different file onto a symlink", "cd /tmp; printf one > a; printf two > b; ln -s b l; cp a l; echo s=$?; cat l"),

    # base64/tr/truncate/factor input validation.
    ("coreutils: base64 -D is not a GNU option", "printf aGk= | base64 -D; echo s=$?"),
    ("coreutils: base64 -d rejects a stray CR", "printf 'aGVsbG8K\\r\\n' | base64 -d; echo s=$?"),
    ("coreutils: tr rejects an equivalence class in string2 when translating", "printf abc | tr abc '[=x=]'; echo s=$?"),
    ("coreutils: truncate rejects a hex size", "cd /tmp; truncate -s 0x10 f 2>&1; echo s=$?"),
    ("coreutils: factor rejects a padded operand", "factor ' 15 '; echo s=$?"),

    # stat directives and unknown options.
    ("coreutils: stat rejects an unknown short option", "cd /tmp; printf abc > f; stat -c %n -@ f; echo s=$?"),
    ("coreutils: stat rejects an unknown long option", "cd /tmp; printf abc > f; stat --bogus f; echo s=$?"),
    ("coreutils: stat --printf has no trailing newline of its own", "cd /tmp; printf 12345 > f; stat --printf '%s\\n' f; echo s=$?"),
    ("coreutils: stat %F says regular empty file for a zero-byte file", "cd /tmp; touch e; printf abc > f; stat -c %F e; stat -c %F f"),
    ("coreutils: stat %N quotes the name and shows a symlink's target", "cd /tmp; printf abc > f; ln -s f l; stat -c %N f; stat -c %N l"),
    ("coreutils: stat %a/%A/%U/%G/%u/%g report this sandbox's fixed defaults", "cd /tmp; printf abc > f; stat -c '%a|%A|%U:%G|%u:%g' f; mkdir d; stat -c '%a|%A' d"),

    # the shell defaults to a UTF-8 locale, matching the rest of this crate.
    ("coreutils: cut -c splits on characters, not UTF-8 bytes", "printf 'caf\\303\\251 ok\\n' | cut -c1-4"),
    ("coreutils: expr length/substr count characters", "expr length héllo; expr substr héllo 1 3"),
    ("coreutils: fold -b still splits on bytes even in a UTF-8 locale", "printf 'aaaaaaaaaaaaa\\n' | fold -b -w5"),
    ("coreutils: md5sum -c prints a non-ASCII name as characters", "cd /tmp; printf hi > f; md5sum f > sums; touch é; md5sum é >> sums 2>/dev/null; md5sum -c sums 2>&1 | tail -1"),

    # stat -f/-t (statvfs-style filesystem stats, and the raw terse dump of real
    # device/inode numbers) ask for data this sandbox genuinely does not have -- there is no
    # real filesystem behind it to report block/inode counts or a filesystem type for, and no
    # real device/inode numbers to print. Refused rather than fabricated.
    ("coreutils: stat -f is refused", "cd /tmp; printf abc > f; stat -f f; echo s=$?"),
    ("coreutils: stat -t is refused", "cd /tmp; printf abc > f; stat -t f; echo s=$?"),

    # sort's WASI ext_sort path concatenated every input file's raw bytes into one
    # buffer before splitting into lines, so an unterminated last line was silently joined to
    # the next file's first bytes instead of ending at that file's own EOF.
    ("coreutils: sort keeps each file's unterminated last line separate", "printf b > /tmp/sx; printf a > /tmp/sy; sort /tmp/sx /tmp/sy"),
    # an empty path fell into the relative-path branch of canonicalize and resolved to
    # the shell's own cwd instead of erroring, as GNU's realpath(3) does for every mode.
    ("coreutils: readlink -f of an empty path fails", "readlink -f ''; echo s=$?"),

    # mv's plain rename path printed a raw io::Error ('No such file or directory (os
    # error N)') instead of GNU's own 'cannot move A to B: reason' when the destination's
    # parent directory does not exist.
    ("coreutils: mv into a missing directory names both paths", "printf x > /tmp/mvsrc; mv /tmp/mvsrc /tmp/nosuchdir/mvdst; echo s=$?"),
    # hard-linking a missing source said 'failed to create hard link A => B' (uutils'
    # own wording); GNU stats the source first and says 'failed to access' instead.
    ("coreutils: ln of a missing source says failed to access", "ln /tmp/nosuch-src /tmp/lndst; echo s=$?"),

    # an absurd -printf field width/precision multiplied into `usize` without an
    # overflow guard, and unconditionally became that many bytes of padding; both are now
    # rejected before either can happen.
    ("coreutils: find -printf refuses an oversized field width", "touch /tmp/fpw; find /tmp/fpw -printf '%999999999999p\\n'; echo s=$?"),
    # numfmt --padding turns directly into a String::with_capacity of that many
    # bytes; capped at the shell's shared 16 MiB in-memory limit instead of risking an OOM.
    ("coreutils: numfmt refuses an oversized --padding", "numfmt --padding=99999999 5; echo s=$?"),

    # cmp -b was entirely unimplemented ('invalid option'), and cmp -l's byte-offset
    # column had no width formatting at all -- GNU right-justifies it to the digit count of the
    # number of bytes actually compared, so the same single-digit offset is padded or not
    # depending on how long the compared region is.
    ("coreutils: cmp -l pads the offset column to the compared length", "printf 'abcdef\\nline2\\n' >/tmp/cl1; printf 'abXdef\\nline2\\n' >/tmp/cl2; cmp -l /tmp/cl1 /tmp/cl2"),
    ("coreutils: cmp -b reports GNU's byte/char wording", "printf '\\007x' >/tmp/cb1; printf '\\001y' >/tmp/cb2; cmp -b /tmp/cb1 /tmp/cb2"),
    ("coreutils: cmp -lb names every control and high-bit byte", "printf '\\000\\001\\011\\040\\101\\176\\177\\200\\233\\310\\377' >/tmp/cbv1; printf '\\377\\376\\366\\337\\226\\126\\017\\016\\015\\014\\013' >/tmp/cbv2; cmp -lb /tmp/cbv1 /tmp/cbv2"),

    # od -t f4/f8 always rendered a float in plain decimal notation, never scientific,
    # because the code compared Rust's Display (fixed) against Debug (also fixed -- Debug only
    # adds a trailing ".0"), so it never actually considered a scientific candidate.
    ("coreutils: od -t f4 uses scientific notation for a large float", "printf 'o, W' | od -An -t f4"),
    ("coreutils: od -t f8 uses scientific notation for a large float", "printf '\\000\\000\\064\\046\\365\\153\\014\\103' | od -An -t f8"),

    # nl -v took a negative NUMBER as two separate short options ('v' then an unknown
    # '2'), and nl -s padded an unnumbered line for a 1-byte separator regardless of the real
    # separator's length.
    ("coreutils: nl -v accepts a negative starting number", "nl -v -2 /dev/null; echo s=$?"),
    ("coreutils: nl -s pads an unnumbered line to the real separator width", "printf 'a\\n\\nb\\n' | nl -s ': ' | od -c | head -n 3"),

    # numfmt's unselected-field passthrough wrote the original inter-field whitespace
    # verbatim, but GNU replaces only its first character with a canonical space (leaving any
    # further whitespace in the run alone) -- a tab separator showed up as a tab instead of a
    # space next to a --format-padded number.
    ("coreutils: numfmt normalizes a tab separator to a space, not verbatim", "printf '2048\\ta\\n' | numfmt --to iec --format '%-5f'"),
    ("coreutils: numfmt preserves a run of spaces beyond the first", "printf '2048   a   b\\n' | numfmt --to iec"),

    # join's default whitespace field-splitter always emitted a trailing field for
    # whatever was left after the last separator match, even when a run of trailing whitespace
    # already consumed the rest of the line -- producing a phantom empty last field whose
    # separator showed up as an extra space in the joined output.
    ("coreutils: join drops a phantom trailing field from trailing whitespace", "printf '1 a  \\n' >/tmp/jtw1; printf '1 b\\n' >/tmp/jtw2; join /tmp/jtw1 /tmp/jtw2"),

    # a byte that is not valid UTF-8 in an argument used to be refused outright by
    # utilities that read it as a Rust String, where GNU accepts any byte.
    ("coreutils: sort -t accepts a non-UTF-8 byte separator", "x=$'\\xff'; printf 'a1\\nb2\\n' | sort -t \"$x\" -k2; echo s=$?"),
    ("coreutils: expr accepts a non-UTF-8 byte in its argument", "x=$'\\xff'; expr \"a$x\" : 'a.'; echo s=$?"),
    ("coreutils: numfmt --suffix accepts a non-UTF-8 byte", "x=$'\\xff'; numfmt --suffix=\"$x\" 5; echo s=$?"),

    # rmdir failed an ordinary directory operand with a trailing slash (WASI's rmdir
    # doesn't tolerate one the way a native POSIX rmdir(2) does); GNU just removes it.
    ("coreutils: rmdir accepts a trailing slash", "mkdir -p /tmp/rmdts; rmdir /tmp/rmdts/; echo s=$?"),

    # fmt's goal-width (GNU's own 93% of -w) was computed by truncating instead of
    # rounding, so the Knuth-Plass line-fill's cost function picked the wrong word count for
    # most explicit -w values -- both the double-space-after-sentence overflow and the
    # one-word-underfill reports traced back to this single bug.
    ("coreutils: fmt fills to GNU's actual goal width, not one word short", "seq 60 | paste -s -d ' ' | fmt -w 75"),
    ("coreutils: fmt's sentence spacing doesn't overflow the requested width", "printf 'The quick brown fox jumps over the lazy dog. It barked.  Then it ran away quickly. The end.\\n' | fmt -w 40"),

    # dd. The byte content and the exit status are the same as GNU's for every form
    # that does not depend on wall-clock throughput (which `status=none` sidesteps entirely).
    ("coreutils: dd if/of/skip/count with status=none matches byte-for-byte", "cd /tmp; printf '0123456789' > din; dd if=din of=dout bs=1 skip=2 count=3 status=none; cat dout; echo; rm -f din dout"),
    ("coreutils: dd conv=notrunc seek writes into the middle of an existing file", "cd /tmp; printf 'AAAAAAAAAA' > dout2; printf 'XX' | dd of=dout2 bs=1 seek=3 conv=notrunc status=none; cat dout2; echo; rm -f dout2"),

    # du. Block-usage numbers are filesystem-dependent (see the README's Commands
    # section) and not this corpus's job to pin; what every implementation owes a script is the
    # right set of paths and the right shape, so these strip the byte-count column and check
    # only that.
    ("coreutils: du -a lists every path in the tree, not just directories", "cd /tmp; mkdir -p dutree/sub; printf a > dutree/f1; printf bb > dutree/sub/f2; du -a dutree | cut -f2 | sort; rm -rf dutree"),
    ("coreutils: du -c prints a grand total line after the named operands", "cd /tmp; mkdir -p da db; printf a > da/f; printf bb > db/f; du -c -s da db | cut -f2; rm -rf da db"),

    # chmod, nproc, hostname, uname. Each deliberately diverges from the oracle (see the
    # EXPECTED fixtures below) -- WASI has no permission bits, real hostname, CPU count or
    # kernel for these to report, so they answer this sandbox's own fixed identity instead.
    ("coreutils: chmod refuses canonically instead of pretending to succeed", "cd /tmp; touch c; chmod 644 c; echo s=$?; rm -f c"),
    ("coreutils: nproc reports this sandbox's one logical core", "nproc; echo s=$?"),
    ("coreutils: hostname answers from GOLEM_WORKER_NAME", "hostname; echo s=$?"),
    ("coreutils: uname -a reports this sandbox's fixed, synthetic fields", "uname -a; echo s=$?"),

    # cp: a process substitution's read end is a pipe, not a regular file, and
    # `fs::copy`'s generic (non-unix, non-windows) fallback refuses anything that isn't one --
    # `std`'s own wording, not this crate's. Streaming the bytes directly instead of relying on
    # `fs::copy` fixes this the same way `cat`/`sh -c` already read `/dev/fd/N` for a pipe.
    ("coreutils: cp -p copies from a /dev/fd/N pipe instead of refusing it", "exec 3< <(printf 'hello\\n'); cp -p /dev/fd/3 /tmp/cpfd.$$; cat /tmp/cpfd.$$; echo s=$?; rm -f /tmp/cpfd.$$"),

    # file: JSON detection, matching libmagic's own test -- a top-level object or
    # array that parses as valid JSON in full (trailing garbage after it does not count, and
    # neither does a bare scalar like `42` or `true`, which real `file` also reports as plain
    # text). HTML, XML and CSV are still not detected (README's Commands section).
    ("coreutils: file recognizes a JSON object and array", "cd /tmp; printf '{\"a\":1}' > j1.json; file j1.json; printf '[1,2,3]' > j2.json; file j2.json; rm -f j1.json j2.json"),
    ("coreutils: file recognizes JSON with leading whitespace and nesting", "cd /tmp; printf '  \\n  {\"a\":{\"b\":2}}\\n' > j3.json; file j3.json; rm -f j3.json"),
    ("coreutils: file does not recognize invalid JSON", "cd /tmp; printf '{not json' > j4.json; file j4.json; rm -f j4.json"),
    ("coreutils: file does not recognize a bare JSON scalar", "cd /tmp; printf '42' > jn.json; file jn.json; printf 'true' > jt.json; file jt.json; rm -f jn.json jt.json"),
    ("coreutils: file does not recognize JSON with trailing garbage", "cd /tmp; printf '{\"a\":1} extra' > jx.json; file jx.json; rm -f jx.json"),
    ("coreutils: file's --mime-type and -i for JSON", "cd /tmp; printf '{\"a\":1}\\n' > j5.json; file --mime-type j5.json; file -i j5.json; rm -f j5.json"),
    ("coreutils: file recognizes a large JSON array", "cd /tmp; { printf '['; seq 1 100000 | tr '\\n' ','; printf '0]'; } > jbig.json; file jbig.json; rm -f jbig.json"),

    # install, the tool-side forms (this crate, not the uutils fork -- see the README's
    # Commands section for why).
    ("coreutils: install copies a single file", "cd /tmp; printf hello > isrc; install isrc idest; cat idest; echo s=$?; rm -f isrc idest"),
    ("coreutils: install with no operands is an error", "install; echo s=$?"),
    ("coreutils: install refuses a directory source", "cd /tmp; mkdir -p idsrc; install idsrc iddst; echo s=$?; rm -rf idsrc iddst"),
    ("coreutils: install -d creates a directory tree", "cd /tmp; install -d itree/sub; ls itree; echo s=$?; rm -rf itree"),
    ("coreutils: install -D creates the destination's parents and copies", "cd /tmp; printf hi > idsrc2; install -D idsrc2 ia/ib/idest2; cat ia/ib/idest2; echo s=$?; rm -rf idsrc2 ia"),
    ("coreutils: install -t copies several sources into a target directory", "cd /tmp; mkdir -p itdir; printf a > ita; printf b > itb; install -t itdir ita itb; ls itdir; echo s=$?; rm -rf itdir ita itb"),
    ("coreutils: install -v reports what it did", "cd /tmp; printf x > ivsrc; install -v ivsrc ivdest; echo s=$?; rm -f ivsrc ivdest; install -v -d ivdir; echo s=$?; rm -rf ivdir"),
    ("coreutils: install -b backs up an existing destination", "cd /tmp; printf old > ibdest; printf new > ibsrc; install -b ibsrc ibdest; cat ibdest; cat ibdest~; echo s=$?; rm -f ibsrc ibdest ibdest~"),
    ("coreutils: install -m/-o/-g are accepted and ignored", "cd /tmp; printf x > imsrc; install -m 755 -o nobody -g nogroup imsrc imdest; cat imdest; echo s=$?; rm -f imsrc imdest"),

    # ls -C/-x: GNU's own calculate_columns, replacing the Grid crate's, which both
    # miscounted how many columns fit and always used tab-filled padding instead of GNU's own
    # "only when it saves a byte" rule.
    ("coreutils: ls -C wraps to one column when nothing wider fits", "cd /tmp; mkdir -p lsw; cd lsw; touch aaa bb ccccccc d ee fffffffffff; ls -C -w 20"),
    ("coreutils: ls -C columns and their tab-stop fill at a middling width", "cd /tmp; mkdir -p lsw2; cd lsw2; touch aaa bb ccccccc d ee fffffffffff g hhhhh; ls -C -w 40"),
    ("coreutils: ls -x fills left-to-right instead of top-to-bottom", "cd /tmp; mkdir -p lsw3; cd lsw3; touch aaa bb ccccccc d ee fffffffffff g hhhhh; ls -x -w 40"),
    ("coreutils: ls -C at a width wide enough for one line uses plain spaces, not tabs", "cd /tmp; mkdir -p lsw4; cd lsw4; touch aaa bb ccccccc d ee fffffffffff g hhhhh; ls -C -w 80"),
]

# Cosmetic, understood, and out of proportion to fix for this finding (see each case's own
# comment above for why); everything else in this corpus lands byte-exact.
EXPECTED_REASON = "documented cosmetic gap; see the case's own comment"
EXPECTED = {
    # stat -f/-t ask for real filesystem/device data (block counts, filesystem type, device
    # and inode numbers) that does not exist in this sandbox.
    "coreutils: stat -f is refused": (
        0,
        b"s=2\n",
        b"stat: -f is unsupported in bash-tool: no real filesystem statistics in the sandbox\n",
    ),
    "coreutils: stat -t is refused": (
        0,
        b"s=2\n",
        b"stat: -t is unsupported in bash-tool: no real device/inode data in the sandbox\n",
    ),
    # A bare `which` refuses with this crate's own usage line, not BusyBox's fictional
    # version-banner text (matching that exactly would be absurd -- it names a program that
    # is not this one). The exit status (1, matching GNU/BusyBox) is what actually matters.
    "coreutils: which with no operands is an error": (0, b"s=1\n", b"which: missing operand\n"),
    # Real GNU find actually tries to write that many bytes of padding and hits glibc's own
    # write(2) limit partway through (EOVERFLOW, "Value too large for data type"); real GNU
    # numfmt actually allocates the ~95 MB of spaces this needs and succeeds. Both are genuine,
    # unbounded-by-GNU-itself allocations this sandbox is not going to reproduce; refusing
    # outright is the documented cap (README's Commands section) instead.
    "coreutils: find -printf refuses an oversized field width": (
        0,
        b"s=2\n",
        b"find: -printf with a field width this large is unsupported in bash-tool\n",
    ),
    "coreutils: numfmt refuses an oversized --padding": (
        0,
        b"s=2\n",
        b"numfmt: --padding above 16777216 is unsupported in bash-tool\n",
    ),
    # GNU appends the raw byte verbatim ("5\xff\n"); numfmt's suffix handling is string-based
    # throughout (grouping, zero-padding, stripping it back off a --from value), so this fix
    # only stops the previous outright refusal -- the byte itself still goes through a lossy
    # UTF-8 conversion (U+FFFD) rather than round-tripping exactly.
    "coreutils: numfmt --suffix accepts a non-UTF-8 byte": (
        0,
        b"5\xef\xbf\xbd\ns=0\n",
        b"",
    ),
    # these all deliberately diverge from the oracle -- WASI has no permission bits for
    # `chmod` to change, and no real hostname/CPU-count/kernel to report, so each answers this
    # sandbox's own fixed, documented identity instead (see the README's Commands section).
    # `GOLEM_WORKER_NAME` is pinned to "conformance-agent" for every case in this file (see
    # conformance.py's fixed environment), so these are as reproducible as any other case here.
    "coreutils: chmod refuses canonically instead of pretending to succeed": (
        0,
        b"s=2\n",
        b"chmod: changing file modes is unsupported in bash-tool\n",
    ),
    "coreutils: nproc reports this sandbox's one logical core": (0, b"1\ns=0\n", b""),
    "coreutils: hostname answers from GOLEM_WORKER_NAME": (
        0,
        b"conformance-agent\ns=0\n",
        b"",
    ),
    "coreutils: uname -a reports this sandbox's fixed, synthetic fields": (
        0,
        b"Linux conformance-agent 6.1.0 #1 SMP wasm32 GNU/Linux\ns=0\n",
        b"",
    ),
    # `install -o`/`-g` are accepted and ignored here (no owners on WASI to change; see the
    # README's Commands section), so unlike real GNU running as a non-root oracle user, this
    # never attempts a chown and so never hits its "Operation not permitted". The chosen names
    # ("nobody"/"nogroup") only matter to the oracle's own failure text, not to this behavior.
    "coreutils: install -m/-o/-g are accepted and ignored": (0, b"xs=0\n", b""),
}
