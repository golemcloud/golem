"""Option sweep, part D: grep, sed and jq, option by option, with their error paths.

grep's diagnostics are compared as GNU words them (curly quotes folded to ASCII). sed's and jq's
wording differs from GNU sed and jq by documented design (EXPECTED_STDERR_REASON in sed.py and
jq.py), so their error cases compare the status and stdout only.
"""

TIER = "sweep"
CASES = []


def err(script):
    return "{ " + script + "; } 2>/tmp/err; echo \"status=$?\"; sed \"s/[\u2018\u2019]/'/g\" /tmp/err"


def add(cmd, label, script, error=False, quiet_error=False):
    tags = [f"cmd.{cmd}"]
    if error or quiet_error:
        tags += [f"cmd.{cmd}.error", "error"]
    if error:
        script = err(script)
    if quiet_error:
        script = "{ " + script + "; } 2>/dev/null; echo \"status=$?\""
    CASES.append((f"opt {cmd}: {label}", script, tags))


def place(opts, operand):
    """Put OPERAND after the command's own words: before the first unquoted `;` or `|`."""
    quote = None
    for i, ch in enumerate(opts):
        if quote:
            if ch == quote:
                quote = None
        elif ch in "'\"":
            quote = ch
        elif ch in ";|":
            return opts[:i].rstrip() + " " + operand + " " + opts[i:]
    return opts + " " + operand


# --- grep ----------------------------------------------------------------------------------------
G = (r"printf 'apple pie\nBanana split\ncherry tart\napple\npineapple\n\nfoo.bar\nfooXbar\nab ab ab\nend\n' >/tmp/g; ")
for o in ["apple", "-i banana", "-y banana", "--no-ignore-case -i BANANA", "-v apple", "-w apple", "-x apple", "-c apple", "-cv apple", "-n apple",
          "-b apple", "-bo ab", "-o ab", "-on 'a[bp]'", "-m 1 apple", "-m 2 -c apple", "-m 0 apple", "-H apple", "-h apple", "-l apple", "-L apple",
          "-q apple", "-s apple", "-e apple -e cherry", "--regexp=split", "-F foo.bar", "-G 'foo.bar'", "-E 'app(le)+$'", "-E 'a{2}'", "'a\\{2\\}'",
          "-E 'ch|pi'", "'ch\\|pi'", "-P 'ap(?=ple)'", "-P '\\bpie\\b'", "-P '\\d'", "-x -v ''", "-e ''", "-c ''", "-A 1 cherry", "-B 1 cherry",
          "-C 1 cherry", "-1 cherry", "-A 1 apple", "-A 1 --group-separator=XX apple", "-A 1 --no-group-separator apple", "-n -A 1 cherry",
          "-b -B 1 cherry", "-T -n apple", "-T -H apple", "-Z -l apple", "-HZ apple", "--label=L -H apple", "-wo ab", "-xc ''", "-ic APPLE",
          "-E '^$' -n", "-n '^'", "-o '^.'", "-o 'x*'", "-E -o '(ab ){2}'", "-w 'ab ab'", "--line-buffered end", "--color=never apple",
          "--color=always apple", "--colour=always -o pie", "-o -i A", "-E 'e$' -c", "'\\<ap'", "'e\\>'", "'\\bab\\b' -c", "-E '\\w+ \\w+$'",
          "'[[:upper:]]'", "'[[:digit:][:punct:]]'", "-E '[^a-z ]'", "-e apple -v -e end", "-f /tmp/pat", "-f /tmp/pat -f /tmp/pat2",
          "-f /tmp/empty", "-F -f /tmp/pat", "-x -F apple", "-w -F ab", "--max-count=1 -o ab", "-ob 'ab'", "-c -m 1 ''", "-lc apple",
          "-L -v apple", "-qv apple", "--null-data apple"]:
    add("grep", f"option {o}", G + "printf 'cherry\\nend\\n' >/tmp/pat; printf 'split\\n' >/tmp/pat2; : >/tmp/empty; grep " + o + " /tmp/g; echo \"status=$?\"")
add("grep", "stdin label", "echo hit | grep --label=stdin -H hit")
add("grep", "stdin dash with file", G + "echo apple | grep -c apple - /tmp/g")
add("grep", "two files", G + "cp /tmp/g /tmp/h; grep -c apple /tmp/g /tmp/h")
add("grep", "two files no filename", G + "cp /tmp/g /tmp/h; grep -h end /tmp/g /tmp/h")
add("grep", "recursive", "mkdir -p /tmp/r/a/b; echo hit >/tmp/r/a/f; echo hit >/tmp/r/a/b/g; echo no >/tmp/r/h; grep -r hit /tmp/r | sort")
add("grep", "recursive no operand", "mkdir -p /tmp/r/a; echo hit >/tmp/r/a/f; cd /tmp/r; grep -r hit")
add("grep", "recursive include", "mkdir -p /tmp/r; echo hit >/tmp/r/a.txt; echo hit >/tmp/r/b.log; grep -r --include='*.txt' hit /tmp/r")
add("grep", "recursive exclude", "mkdir -p /tmp/r; echo hit >/tmp/r/a.txt; echo hit >/tmp/r/b.log; grep -r --exclude='*.txt' hit /tmp/r")
add("grep", "recursive exclude dir", "mkdir -p /tmp/r/skip /tmp/r/keep; echo hit >/tmp/r/skip/f; echo hit >/tmp/r/keep/f; grep -r --exclude-dir=skip hit /tmp/r")
add("grep", "exclude from", "mkdir -p /tmp/r; echo hit >/tmp/r/a.txt; echo hit >/tmp/r/b.log; echo '*.log' >/tmp/x; grep -r --exclude-from=/tmp/x hit /tmp/r")
add("grep", "recursive files with matches", "mkdir -p /tmp/r/d; echo hit >/tmp/r/d/f; echo no >/tmp/r/g; grep -rl hit /tmp/r")
add("grep", "recursive count", "mkdir -p /tmp/r/d; echo hit >/tmp/r/d/f; echo no >/tmp/r/g; grep -rc hit /tmp/r | sort")
add("grep", "dereference recursive", "mkdir -p /tmp/r/real; echo hit >/tmp/r/real/f; cd /tmp/r; ln -s real link; grep -R hit . | sort")
add("grep", "recursive skips symlinks", "mkdir -p /tmp/r/real; echo hit >/tmp/r/real/f; cd /tmp/r; ln -s real link; grep -r hit . | sort")
add("grep", "directory operand", "mkdir /tmp/d; grep x /tmp/d", error=True)
add("grep", "directories skip", "mkdir /tmp/d; grep -d skip x /tmp/d; echo \"status=$?\"")
add("grep", "directories read", "mkdir /tmp/d; grep --directories=read x /tmp/d", error=True)
add("grep", "directories recurse", "mkdir /tmp/d; echo x >/tmp/d/f; grep -d recurse x /tmp/d")
add("grep", "devices skip", "grep -D skip x /dev/null; echo \"status=$?\"")
add("grep", "binary file matches", "printf 'a\\000b\\nhit\\n' >/tmp/b; grep hit /tmp/b")
add("grep", "binary text", "printf 'a\\000b\\nhit\\n' >/tmp/b; grep -a hit /tmp/b")
add("grep", "binary without match", "printf 'a\\000b\\nhit\\n' >/tmp/b; grep -I hit /tmp/b; echo \"status=$?\"")
add("grep", "binary files text", "printf 'a\\000hit\\n' >/tmp/b; grep --binary-files=text hit /tmp/b | od -An -c")
add("grep", "binary files binary", "printf 'a\\000hit\\n' >/tmp/b; grep --binary-files=binary -c hit /tmp/b")
add("grep", "binary count", "printf 'a\\000hit\\nhit\\n' >/tmp/b; grep -c hit /tmp/b")
add("grep", "binary only matching", "printf 'hit\\000\\n' | grep -o hit")
add("grep", "invalid utf8 is binary", "printf 'caf\\351 hit\\n' | grep hit")
add("grep", "null data", "printf 'a\\nb\\0c\\0' | grep -z b | od -An -c")
add("grep", "null data anchors", "printf 'one\\ntwo\\0three\\0' | grep -z '^two' | od -An -c")
add("grep", "crlf binary option", "printf 'a\\r\\n' | grep -U 'a.$' | od -An -c")
add("grep", "multiple patterns newline", G + "grep \"$(printf 'cherry\\nend')\" /tmp/g")
add("grep", "backreference", "printf 'abab\\nabcd\\n' | grep '\\(ab\\)\\1'")
add("grep", "backreference ere", "printf 'xx\\nxy\\n' | grep -E '(x)\\1'")
add("grep", "perl lookbehind", "printf 'price: 42\\n' | grep -oP '(?<=price: )\\d+'")
add("grep", "perl keep", "printf 'key=val\\n' | grep -oP 'key=\\Kval'")
add("grep", "perl non greedy", "printf '<a><b>\\n' | grep -oP '<.+?>'")
add("grep", "perl case insensitive flag", "printf 'ABC\\n' | grep -P '(?i)abc'")
add("grep", "perl unicode class", "printf '\u00e9t\u00e9\\n' | grep -oP '\\p{L}+'")
add("grep", "ignore case multibyte", "printf '\u00c9T\u00c9\\n' | grep -i '\u00e9t\u00e9'")
add("grep", "dot matches multibyte", "printf '\u00e9\\n' | grep -x '.'")
add("grep", "bracket range", "printf 'b\\nB\\n' | grep '[a-c]'")
add("grep", "equivalence class", "printf 'a\\n' | grep '[[=a=]]'")
add("grep", "interval ere", "printf 'aaa\\n' | grep -oE 'a{1,2}'")
add("grep", "star at start bre", "printf '*a\\n' | grep '*a'")
add("grep", "plus in bre literal", "printf 'a+\\n' | grep 'a+'")
add("grep", "question in bre", "printf 'ab\\n' | grep 'ab\\?c\\?'")
add("grep", "anchor in middle bre", "printf 'a^b\\n' | grep 'a^b'")
add("grep", "empty alternation", "printf 'x\\n' | grep -E 'a|' -c")
add("grep", "context overlapping", "seq 1 10 | grep -C 1 -e 3 -e 5")
add("grep", "context with max count", "seq 1 10 | grep -m 1 -A 2 '[0-9]'")
add("grep", "after context at end", "seq 1 3 | grep -A 5 2")
add("grep", "context separators with filename", "seq 1 10 >/tmp/n; grep -H -n -C 1 -e 3 -e 8 /tmp/n")
add("grep", "byte offset only matching multiple", "printf 'xaxa\\n' | grep -bo a")
add("grep", "invert with context", "seq 1 5 | grep -v -A 1 '[2-4]'")
add("grep", "count with invert empty", "printf '' | grep -vc x")
add("grep", "missing file", "grep x /tmp/nosuch", error=True)
add("grep", "missing file silent", "grep -s x /tmp/nosuch; echo \"status=$?\"")
add("grep", "missing file quiet match", G + "grep -q apple /tmp/nosuch /tmp/g; echo \"status=$?\"")
add("grep", "missing file with match", G + "grep apple /tmp/nosuch /tmp/g", error=True)
add("grep", "invalid regex bracket", "echo x | grep '['", error=True)
add("grep", "invalid regex paren", "echo x | grep -E '('", error=True)
add("grep", "unmatched paren bre", "echo x | grep '\\('", error=True)
add("grep", "invalid interval", "echo x | grep -E 'a{2,1}'", error=True)
add("grep", "invalid back reference", "echo x | grep '\\(a\\)\\2'", error=True)
add("grep", "invalid class", "echo x | grep '[[:bogus:]]'", error=True)
add("grep", "trailing backslash", "echo x | grep 'a\\'", error=True)
add("grep", "invalid perl", "echo x | grep -P '(?<'", error=True)
add("grep", "conflicting matchers", "echo x | grep -E -F x", error=True)
add("grep", "perl with several patterns", "echo x | grep -P -e a -e x", error=True)
add("grep", "invalid context", "echo x | grep -A x x", error=True)
add("grep", "invalid max count", "echo x | grep -m x x", error=True)
add("grep", "invalid binary files", "echo x | grep --binary-files=bogus x", error=True)
add("grep", "invalid directories", "echo x | grep -d bogus x", error=True)
add("grep", "missing pattern", "grep", error=True)
add("grep", "missing pattern file", "grep -f /tmp/nosuch x", error=True)
add("grep", "unknown option", "grep --bogus x", error=True)
add("grep", "unknown short option", "grep -j x", error=True)
add("grep", "ambiguous long option", "grep --no x", error=True)
add("grep", "stray backslash warning", "echo 'a:' | grep 'a\\:'", error=True)
add("grep", "character class syntax warning", "echo ':' | grep '[:space:]'", error=True)
add("grep", "star after anchor ere", "printf 'a\\n' | grep -E '^*a'", error=True)
add("grep", "negative max count", "seq 1 3 | grep -m -1 '[0-9]'")
add("grep", "long option abbreviation", G + "grep --ignore apple /tmp/g")
add("grep", "option after operand", G + "grep apple /tmp/g -c")
add("grep", "double dash pattern", "printf -- '-x\\n' | grep -- -x")
add("grep", "pattern with newline and fixed", "printf 'a\\nb\\n' | grep -F \"$(printf 'x\\nb')\"")
add("grep", "word with underscore", "printf 'foo_bar\\nfoo bar\\n' | grep -w foo")
add("grep", "line regexp with alternation", "printf 'ab\\nabc\\n' | grep -xE 'ab|abc'")
add("grep", "only matching with invert", "printf 'a\\nb\\n' | grep -ov a")
add("grep", "files without match status", "printf 'x\\n' >/tmp/a; grep -L y /tmp/a; echo \"status=$?\"")

# --- sed -----------------------------------------------------------------------------------------
SD = r"printf 'one\ntwo\nthree\nfour\nfive\nsix\n' >/tmp/s; "
for o in ["-n p", "-n 2p", "'2,4d'", "'$d'", "-n '$p'", "'1~2d'", "'0~3d'", "'2,+2d'", "'2,~4d'", "'0,/o/d'", "'1,/o/d'", "'/two/,/four/d'",
          "'/two/,+1d'", "'/t/!d'", "'2!d'", "-n '/^t/p'", "-n '\\%^f%p'", "-n '\\,o,p'", "-n '/O/Ip'", "'s/o/0/'", "'s/o/0/g'", "'s/e/E/2'",
          "'s/e/E/2g'", "-n 's/o/0/p'", "'s/O/0/I'", "'s/O/0/i'", "'s/\\(t\\)\\(w\\)/\\2\\1/'", "-E 's/(t)(w)/\\2\\1/'", "-r 's/(o+)/[\\1]/'",
          "'s/o/[&]/'", "'s/o/\\&/'", "'s/.*/\\U&/'", "'s/.*/\\u&/'", "'s/\\w\\+/\\L\\u&/'", "'s/e/\\n/'", "'s/e/\\t/' ", "'s|o|/|'",
          "'s/x*/-/g'", "'s/o/0/w /tmp/w' -n; cat /tmp/w", "'y/abcdefghijklmnopqrstuvwxyz/ABCDEFGHIJKLMNOPQRSTUVWXYZ/'", "'y/o\\n/0_/'",
          "'2a\\appended'", "'2a appended'", "'2i\\inserted'", "'2i inserted'", "'2c\\changed'", "'2,3c\\changed'", "'$a end'", "'1i\\\n  lead'",
          "-n '2{p;p}'", "'2q'", "'2Q'", "'2q5'; echo \"q=$?\"", "'2Q7'; echo \"q=$?\"", "-n '$='", "'='", "-n 'l'", "-n 'l 5'", "-l 4 -n l",
          "'N;P;D'", "'$!N;s/\\n/+/'", "'N;N;s/\\n/,/g'", "'1h;2,$H;$!d;x;s/\\n/ /g'", "'1!G;h;$!d'", "'n;d'", "-n 'n;p'", "'x;G'",
          "'/two/{n;d}'", "':a;N;$!ba;s/\\n/ /g'", "'s/o/0/;ta;s/$/!/;:a'", "'s/o/0/;Tb;s/$/!/;:b'", "'bx;s/o/0/;:x'", "-n '/f/{s/f/F/;p}'",
          "'/two/r /tmp/r'", "'/two/R /tmp/r'", "'2w /tmp/o' -n; cat /tmp/o", "'1~3W /tmp/o' -n; cat /tmp/o", "'3z'", "-n F", "-s -n '$p' /tmp/s",
          "-n -s 1p /tmp/s", "--posix 's/o/0/'", "-u 2q", "--unbuffered -n 3p", "--expression=2d -e 3d", "-e 1d -e '$d'", "--quiet 2p",
          "--silent 3p", "--regexp-extended 's/(e)$/[\\1]/'", "-z 's/\\n/,/g'", "--null-data 's/^/>/'", "--sandbox 's/o/0/'", "--debug 2q",
          "'s/[[:digit:]]*/N/'", "'s/\\bt/T/g'", "'s/o\\+/O/'", "-E 's/o{2}/OO/'", "'s/e\\?$/!/'", "'/^$/d'", "'s/^/  /;s/ *$//'", "-n '/two/='",
          "'2!{s/^/-/}'", "'1d;$d'", "'2,4!d'", "-n '4,2p'", "'0~0d'", "'s/./X/3'", "'s/\\(.\\)\\(.\\)/\\2\\1/g'", "-E 's/(.)(.)?/<\\2>/g'",
          "'s/n/\\x41/'", "'s/n/\\o101/'", "'s/n/\\d65/'", "'s/n/\\cA/' | od -An -c", "-n '/o/{/n/p}'"]:
    add("sed", f"script {o}", SD + "printf 'R1\\nR2\\n' >/tmp/r; sed " + place(o, "/tmp/s"))
add("sed", "in place", SD + "sed -i 's/o/0/' /tmp/s; cat /tmp/s")
add("sed", "in place suffix", SD + "sed -i.bak '1d' /tmp/s; head -n 1 /tmp/s /tmp/s.bak")
add("sed", "in place long suffix", SD + "sed --in-place=.orig 2d /tmp/s; ls /tmp/s*")
add("sed", "in place two files separate", SD + "cp /tmp/s /tmp/t; sed -i '$d' /tmp/s /tmp/t; wc -l /tmp/s /tmp/t")
add("sed", "in place missing file", SD + "sed -i 1d /tmp/nosuch /tmp/s; echo \"status=$?\"; wc -l </tmp/s")
add("sed", "in place with q", SD + "sed -i 3q /tmp/s; cat /tmp/s")
add("sed", "in place w to stdout", SD + "sed -i 's/one/ONE/w /dev/stdout' /tmp/s; head -n 1 /tmp/s")
add("sed", "follow symlinks", "cd /tmp; printf 'a\\n' >t; ln -s t l; sed -i --follow-symlinks 's/a/b/' l; cat t; readlink l")
add("sed", "in place on symlink replaces link", "cd /tmp; printf 'a\\n' >t; ln -s t l; sed -i 's/a/b/' l; cat t; [ -L l ] && echo link || cat l")
add("sed", "script file", SD + "printf '2d\\ns/o/0/\\n' >/tmp/sc; sed -f /tmp/sc /tmp/s")
add("sed", "script file with comments", SD + "printf '# comment\\n1d # trailing\\n' >/tmp/sc; sed -f /tmp/sc /tmp/s")
add("sed", "script file n first line", SD + "printf '#n\\n2p\\n' >/tmp/sc; sed -f /tmp/sc /tmp/s")
add("sed", "script file and expression", SD + "printf '1d\\n' >/tmp/sc; sed -f /tmp/sc -e 2d /tmp/s")
add("sed", "script from stdin file", SD + "echo 3p | sed -n -f - /tmp/s")
add("sed", "multiple files continuous", SD + "sed -n '$=' /tmp/s /tmp/s")
add("sed", "stdin dash", SD + "cat /tmp/s | sed 2q -")
add("sed", "no trailing newline kept", "printf 'a\\nb' | sed 's/b/c/' | od -An -c")
add("sed", "append after last line without newline", "printf 'a' | sed 'a x' | od -An -c")
add("sed", "empty regex reuses last", SD + "sed -n '/o/{s//0/p}' /tmp/s")
add("sed", "newline in character class", "printf 'a\\nb\\n' | sed 'N;s/[\\n]/+/'")
add("sed", "case conversion stops", "echo hello world | sed 's/\\(hello\\) \\(world\\)/\\U\\1\\E \\2/'")
add("sed", "special replacement newline", "echo ab | sed 's/a/&\\\n/'")
add("sed", "grouped commands semicolon", SD + "sed -n '2{p};4{p}' /tmp/s")
add("sed", "a with leading whitespace kept", "echo x | sed 'a\\   indented'")
add("sed", "c on range with negation", SD + "sed '2,5!c X' /tmp/s")
add("sed", "D restarts without reading", "printf 'a\\nb\\nc\\n' | sed '$!N;P;D'")
add("sed", "long script loop count", "seq 1 5 | sed -n ':a;$!{N;ba};s/\\n/+/gp'")
add("sed", "T resets", "printf 'x\\ny\\n' | sed 's/x/X/;T;s/$/ changed/'")
add("sed", "missing script", "sed", quiet_error=True)
add("sed", "unknown command", "echo x | sed k", quiet_error=True)
add("sed", "unterminated s", "echo x | sed 's/a/b'", quiet_error=True)
add("sed", "unknown s option", "echo x | sed 's/a/b/q'", quiet_error=True)
add("sed", "unmatched brace", "echo x | sed '{p'", quiet_error=True)
add("sed", "unexpected brace", "echo x | sed '}'", quiet_error=True)
add("sed", "extra characters after command", "echo x | sed 'dp'", quiet_error=True)
add("sed", "missing file", "sed p /tmp/nosuch", quiet_error=True)
add("sed", "missing file continues", SD + "sed -n 1p /tmp/nosuch /tmp/s", quiet_error=True)
add("sed", "undefined label", "echo x | sed 'bnope'", quiet_error=True)
add("sed", "invalid regex", "echo x | sed 's/\\(/x/'", quiet_error=True)
add("sed", "invalid back reference", "echo x | sed 's/x/\\1/'", quiet_error=True)
add("sed", "y lengths differ", "echo x | sed 'y/ab/c/'", quiet_error=True)
add("sed", "address zero", "echo x | sed '0p'", quiet_error=True)
add("sed", "zero address with non regex", "echo x | sed '0,2p'", quiet_error=True)
add("sed", "command takes one address", "echo x | sed '1,2='", quiet_error=False)
add("sed", "q with two addresses", "echo x | sed '1,2q'", quiet_error=True)
add("sed", "missing script file", "sed -f /tmp/nosuch", quiet_error=True)
add("sed", "unknown option", "sed --bogus p", quiet_error=True)
add("sed", "invalid line length", "echo x | sed -l x -n l", quiet_error=True)
add("sed", "sandbox rejects w", "echo x | sed --sandbox 'w /tmp/o'", quiet_error=True)
add("sed", "sandbox rejects r", "echo x | sed --sandbox 'r /tmp/o'", quiet_error=True)
add("sed", "posix rejects extension", "echo x | sed --posix 's/x/\\U&/'", quiet_error=False)
add("sed", "in place without file", "echo x | sed -i p", quiet_error=True)
add("sed", "r missing file ignored", "echo x | sed 'r /tmp/nosuch'; echo \"status=$?\"")
add("sed", "w to directory", "mkdir /tmp/d; echo x | sed 'w /tmp/d'", quiet_error=True)
add("sed", "comments and spaces", "echo x | sed -e '  # c' -e ' s/x/y/ ; # tail'")
add("sed", "empty script", SD + "sed '' /tmp/s | wc -l")
add("sed", "unbalanced parenthesis ere", "echo x | sed -E 's/(/x/'", quiet_error=True)
add("sed", "invalid interval", "echo x | sed 's/x\\{2,1\\}/y/'", quiet_error=True)
add("sed", "exit code from q on stdin end", "printf '' | sed 'q5'; echo \"status=$?\"")

# --- jq ------------------------------------------------------------------------------------------
JS = "printf '{\"b\":2,\"a\":[1,\"x\",null],\"c\":{\"d\":true}}\\n' >/tmp/j; "
for o in [".", "-c .", "-r .c", "-r .a[1]", "-j '.a[]'", "-a '\"\u00e9\"'", "-S .", "-S -c .", "--sort-keys -c .", "--tab .", "--indent 1 .",
          "--indent 0 .", "--indent 7 .", "-C -c .", "-M -c .", "-e .c.d", "-e .x", "-e 'false'", "-e empty", "--exit-status .b", "-r '.a[]'",
          "--raw-output0 '.a[1]' | od -An -c", "-c 'keys'", "-c 'to_entries'", "'.a | length'", "-c 'paths'", "'.a[0] + .b'", "-c '[.[]|type]'",
          "--compact-output '.c'", "--raw-output '.a[1]'", "--join-output '.b, .b'; echo", "--ascii-output -c '{\"k\":\"\u00e9\u4e2d\"}'",
          "-c --seq .c | od -An -c", "-c 'del(.a)'", "-c '.a |= map(select(. != null))'", "-r '.a | @csv'", "-r '.a | @tsv'", "-r '@base64 \"\\(.b)\"'",
          "-r '.a | @json'", "-r '.a[1] | @sh'", "-r '.a[1] | @uri'", "-r '.a[1] | @html'", "-r '[.b] | @text'", "-c 'with_entries(.value |= tostring)'",
          "-c '[paths(type == \"number\")]'", "'.a[1] | ascii_upcase'", "-c 'getpath([\"c\",\"d\"])'", "'has(\"a\")'", "-c '[.[] | numbers]'", "'.a | index(\"x\")'",
          "-c '.a | map(tostring)'", "'.b | tostring | tonumber'", "-c 'env | has(\"GOLEM_AGENT_TYPE\")'", "-r '$ENV.GOLEM_AGENT_TYPE'", "'input_line_number'",
          "-c '[limit(2; .a[])]'", "-c 'first(.a[])'", "-c '.a | reverse'", "-c '[.a[] | strings]'", "'.a | any'", "'.a | all'", "-c '.a | sort'",
          "-c '.a | unique'", "-c 'reduce .a[] as $x (0; . + 1)'", "-c '[foreach .a[] as $x (0; . + 1)]'", "'.b as $v | $v * 10'", "-c 'splits(\"x\")?'",
          "-r '.a[1] | test(\"X\"; \"i\")'", "-c '.a[1] | [match(\"x\").offset]'", "-r '.a[1] | sub(\"x\"; \"y\")'", "-c 'tojson | fromjson'",
          "-c '.a | .[1:]'", "-c '.a | .[-1]'", "'.. | numbers'", "-c 'try error(\"boom\") catch .'", "-c '.b // \"none\"'", "-c '.z // \"none\"'",
          "'if .b > 1 then \"big\" else \"small\" end'", "-c 'label $out | foreach .a[] as $i (0; .+1; if . > 1 then ., break $out else . end)'",
          "-c 'def f(x): x * 2; f(.b)'", "-c '[range(3)]'", "-c '[range(1;10;3)]'", "'@text \"b=\\(.b)\"'", "-c 'input? // \"eof\"'", "-c '[inputs]'",
          "'$__prog_args' -c", "-c 'ltrimstr(\"x\")'", "'.a[1] | ascii_downcase | explode | implode'", "-c 'splits(\", \")' <<< '\"a, b\"'",
          "'now | type'", "-c 'getpath([\"a\",0])'", "'.a | join(\",\")' 2>&1", "-c 'to_entries | from_entries'", "-c '[.[] | objects]'",
          "'.c | length'", "'.a[1] | utf8bytelength'", "-c 'infinite, -infinite, nan | tostring'", "'1e1000'", "'-0'", "'.b / 3'", "'10 % 3'",
          "'[1,2] - [2]' -c", "'{} * {\"a\":1}' -c", "'\"abc\" * 2'", "'\"a,b\" / \",\"' -c", "'null + 1'", "-c 'min_by(.x)?' ", "'[3,1,2] | min, max'",
          "'@base64d' <<< '\"aGk=\"'", "-c 'tostream' ", "-c 'fromstream(tostream)'", "-c 'getpath([\"nope\",\"x\"])'", "'.a[1:2] | length'",
          "-c 'walk(if type == \"number\" then . + 1 else . end)'", "-c 'to_entries[0]'", "'splits(\"a\") | length' <<< '\"banana\"'",
          "'ascii' <<< 65", "'@json \"v=\\(.a)\"'", "'.b | @sh'", "-c 'limit(0; .a[])'", "-c 'isvalid(.a)'", "'getpath([\"a\",1]) | ltrimstr(\"x\")'",
          "'.c.d | not'", "'input_filename'", "'.b | tojson'", "-c 'splits(\"x\")'", "'ltrimstr(1)'", "-c 'debug' 2>&1",
          "-c 'stderr' 2>&1", "'halt_error' <<< '\"bye\\n\"'", "'\"x\" | halt_error(3)'; echo \"q=$?\"", "'halt'", "-c '$ENV | type'",
          "-n 'input' ", "-n '[inputs]' -c", "-n 'reduce inputs as $x (0; . + 1)'", "-rn '@text \"\\(1+1)\"'", "-cn '[1,[2]] | flatten'",
          "-n '\"\\u00e9\" | @uri' -r", "-n '[.[]?]' -c", "-n 'getpath([\"a\"])'", "-n 'splits(\"a\")' <<< '\"x\"'", "-n '\"abc\" | .[1:]'",
          "-n '[\"a\",\"b\"] | combinations' -c", "-n '[[1,2],[3,4]] | [combinations]' -c", "-n 'now | floor | type'", "-n '\"2015-03-05T23:51:47Z\" | fromdate'",
          "-n '0 | todate'", "-n '1425599507 | gmtime | mktime'", "-n '0 | strftime(\"%Y-%m-%dT%H:%M:%SZ\")'", "-n '\"10:15\" | strptime(\"%H:%M\") | mktime' 2>&1",
          "-n '[splits(\"a, b\"; \", \")]' -c", "-n '\"test\" | ascii_upcase'", "-n '[1,2,3] | IN(2)'", "-n '2 | IN(1,2)'", "-n '[1,2] | any(. > 1)'",
          "-n '{\"a\":1} | to_entries' -c", "-n '[1,null,2] | map(values)' -c", "-n '\"a\" | ascii_downcase | test(\"A\"; \"ix\")'",
          "-n '[1,2,3] | getpath([5])'", "-n '{\"a\":{\"b\":1}} | [leaf_paths]' -c", "-n '\"abc\" | sub(\"(?<x>b)\"; \"[\\(.x)]\")'",
          "-n '\"aXbXc\" | [splits(\"X\")]' -c", "-n '\"abc\" | gsub(\"\"; \"-\")'", "-n '[limit(3; repeat(1))]' -c", "-n '[1,2] | .[1:] = [9]' -c",
          "-n 'input_line_number'", "-n 'getpath([\"a\",\"b\"]) = 1' -c", "-n '{} | .a.b.c = 1' -c", "-n '[] | .[3] = 1' -c",
          "-n 'error' ; echo \"q=$?\"", "-n 'error(null)'; echo \"q=$?\"", "-n '{} | error'; echo \"q=$?\"", "-n '\"a\" | error'; echo \"q=$?\"",
          "-n '1 as [$a] | $a'", "-n '[1,[2,3]] as [$a,[$b]] | $a+$b'", "-n '{\"a\":1} as {a:$x} | $x'", "-n '[[1,2]] | .[] as [$a] ?// $a | $a' -c",
          "-n '[.[]?] | length'", "-n 'try (1/0) catch .'", "-n '1 / 0'; echo \"q=$?\"", "-n '[1] | implode'", "-n '\"\\ud83d\"' -a",
          "-n '@base32 \"hi\"' -r", "-n '\"NBUQ====\" | @base32d' -r", "-n 'ltrimstr(\"a\")'", "-n '1 | tojson | fromjson'",
          "-n '[1,2] | tostring'", "-n '{\"a\":[1,2]} | tostring'", "-n '\"1.50\" | tonumber'", "-n '\"x\" | tonumber'; echo \"q=$?\"", "-n '100000000000000000000'",
          "-n '1.000'", "-n '3.0'", "-n '[1.5,2.0,0.1]' -c", "-n '1e3'", "-n '0.00001'", "-n '123456789012'", "-n '-1e-7'", "-n '[.1 + .2]' -c",
          "-n 'pow(2;10)'", "-n 'log10' <<< 100", "-n '4 | sqrt'", "-n '2.5 | floor, ceil, round'", "-n '[-1.5 | fabs, trunc]' -c", "-n 'infinite | floor' 2>&1",
          "-n '\"\\(1,2)-\\(3,4)\"'", "-n '[\"b\",\"a\"] | sort_by(.)' -c", "-n '[{\"a\":2},{\"a\":1}] | group_by(.a)' -c", "-n '[{\"a\":2},{\"a\":1}] | unique_by(.a)' -c",
          "-n '[1,2,3] | add'", "-n '[] | add'", "-n '[[1],[2]] | add' -c", "-n '{\"a\":1,\"b\":2} | map_values(.+1)' -c", "-n '{\"a\":1} | keys_unsorted' -c",
          "-n '[3,1] | sort | first, last'", "-n '[1,2,3] | nth(1)'", "-n 'splits' 2>/dev/null; echo \"q=$?\"", "-n '[\"a\"] | inside([\"a\",\"b\"])'",
          "-n '\"foobar\" | contains(\"bar\")'", "-n '{\"a\":1} | contains({\"a\":1})'", "-n '\"abc\" | startswith(\"a\"), endswith(\"c\")'",
          "-n '\"a b\" | split(\" \")' -c", "-n '\"a1b2\" | [scan(\"[0-9]\")]' -c", "-n '\"abc\" | capture(\"(?<x>b)\")' -c", "-n '\"  x \" | trim, ltrim, rtrim' -c",
          "-n '[1,[2,[3]]] | flatten(1)' -c", "-n '{\"a\":null} | .a // 5'", "-n '[1,2] | index(2)'", "-n '\"abcb\" | rindex(\"b\")'", "-n '\"abcb\" | indices(\"b\")' -c",
          "-n '[3,2,1] | bsearch(2)'", "-n 'splits(\"\") ' <<< '\"ab\"' -c", "-n 'getpath([])' -c", "-n '$__prog_name' 2>&1; echo \"q=$?\"", "-n 'tojson' <<< 1",
          "-n 'env | type'", "-n 'builtins | length > 10'", "-n 'input_filename'", "-n '\"x\" | ascii'", "-n '{\"a\":1} | del(.a)' -c", "-n '[1,2,3] | del(.[0,2])' -c",
          "-n 'to_entries' <<< '{}' -c", "-n '[paths]' -c", "-n '{\"a\":1} | has(\"b\")'", "-n '[1] | has(0)'", "-n '\"abc\" | ascii_downcase | length'",
          "-n '@sh \"echo \\(\"a b\")\"' -r", "-n '[1,\"a\"] | @csv' -r", "-n '[\"a\\tb\"] | @tsv' -r", "-n 'splits(1)' 2>/dev/null; echo \"q=$?\"",
          "-n 'getpath(1)' 2>/dev/null; echo \"q=$?\"", "-n '{} | .[\"a\"] = 1' -c", "-n 'limit(-1; 1,2)' -c", "-n '[.[]?] | tojson'",
          "-n 'range(5) | select(. % 2 == 0)'", "-n 'until(. > 4; . + 3)' <<< 0", "-n '0 | until(. > 4; . + 3)'", "-n '[while(. < 4; . + 1)]' -c",
          "-n '1 | [recurse(if . < 3 then . + 1 else empty end)]' -c", "-n '{\"a\":[{\"b\":1}]} | [recurse] | length'", "-n '[splits(\"b\")] ' -c",
          "-n '\"a\" | test(\"A\"; \"i\")'", "-n '\"\\n\" | @json'", "-n 'ltrimstr(\"x\") | type'", "-n '{\"a\":1} + null' -c", "-n 'null | not'",
          "-n '[true, false, null, 0, \"\"] | map(if . then 1 else 0 end)' -c", "-n '\"\\u0000\"' ", "-n '\"a\" < \"b\", [] < {}, null < false'",
          "-n '{\"b\":1,\"a\":2} | tojson'", "-n '{\"b\":1,\"a\":2}' -S -c", "-n '[{\"b\":1,\"a\":2}] | .[0] | keys' -c", "-n '$x' --arg x 1",
          "-n '$x + 1' --argjson x 1", "-n '$named' -c --arg a 1 --argjson b 2", "-n '$ARGS' -c --args a b", "-n '$ARGS.positional' -c --jsonargs 1 '{\"a\":2}'",
          "-n '$f' -c --slurpfile f /tmp/j", "-n '$f | length' --rawfile f /tmp/j", "-n '[$a, $b]' -c --arg a x --arg b y", "-n '$__named' 2>/dev/null -c; echo \"q=$?\"",
          "-n '$ARGS.named' -c --arg k v --argjson n 3", "-rn '$ENV.GOLEM_AGENT_ID'", "-n 'input' --args a 2>/dev/null; echo \"q=$?\""]:
    words = o if "<<<" in o else place(o, "/tmp/j")
    if "2>" in o:
        add("jq", f"filter {o}", JS + "jq " + words + "; echo \"status=$?\"")
    else:
        # jaq words its errors differently from jq (jq.py's EXPECTED_STDERR_REASON): compare
        # whether there was a diagnostic, not its text.
        add("jq", f"filter {o}", JS + "{ jq " + words + "; } 2>/tmp/e; echo \"status=$?\"; [ -s /tmp/e ] && echo diagnostic")
add("jq", "null input", "jq -n '1+1'")
add("jq", "null input long", "jq --null-input '[1,2]' -c")
add("jq", "raw input", "printf 'a\\nb\\n' | jq -R .")
add("jq", "raw input slurp", "printf 'a\\nb\\n' | jq -Rs .")
add("jq", "raw input no trailing newline", "printf 'a\\nb' | jq -R -c .")
add("jq", "raw input null input inputs", "printf 'a\\nb\\n' | jq -nR '[inputs]' -c")
add("jq", "slurp", "printf '1 2 3' | jq -s -c .")
add("jq", "slurp long", "printf '{\"a\":1}\\n{\"a\":2}\\n' | jq --slurp 'map(.a)' -c")
add("jq", "slurp empty", "printf '' | jq -s -c .")
add("jq", "slurp two files", "echo 1 >/tmp/a; echo 2 >/tmp/b; jq -s -c . /tmp/a /tmp/b")
add("jq", "several inputs", "printf '1 \"a\" [2]' | jq -c .")
add("jq", "two files", "echo 1 >/tmp/a; echo 2 >/tmp/b; jq . /tmp/a /tmp/b")
add("jq", "filter from file", "echo '.a' >/tmp/f; echo '{\"a\":5}' | jq -f /tmp/f")
add("jq", "filter from file long", "echo '.a + 1' >/tmp/f; echo '{\"a\":5}' | jq --from-file /tmp/f")
add("jq", "filter from file with input file", "echo '.a' >/tmp/f; echo '{\"a\":6}' >/tmp/i; jq -f /tmp/f /tmp/i")
add("jq", "filter file with comments", "printf '# c\\n.a # tail\\n' >/tmp/f; echo '{\"a\":7}' | jq -f /tmp/f")
add("jq", "exit status false", "echo false | jq -e .; echo \"status=$?\"")
add("jq", "exit status null", "echo null | jq -e .; echo \"status=$?\"")
add("jq", "exit status no output", "echo 1 | jq -e empty; echo \"status=$?\"")
add("jq", "exit status last output", "echo 1 | jq -e '., false'; echo \"status=$?\"")
add("jq", "seq input", "printf '\\0361\\n\\0362\\n' | jq --seq -c . | od -An -c")
add("jq", "tab and indent last wins", "echo '{\"a\":[1]}' | jq --tab --indent 3 .")
add("jq", "indent too large", "echo 1 | jq --indent 9 .", quiet_error=True)
add("jq", "indent invalid", "echo 1 | jq --indent x .", quiet_error=True)
add("jq", "arg missing value", "jq -n --arg x", quiet_error=True)
add("jq", "argjson invalid", "jq -n --argjson x '{' '$x'", quiet_error=True)
add("jq", "slurpfile missing", "jq -n --slurpfile x /tmp/nosuch '$x'", quiet_error=True)
add("jq", "rawfile missing", "jq -n --rawfile x /tmp/nosuch '$x'", quiet_error=True)
add("jq", "undefined variable", "jq -n '$nope'", quiet_error=True)
add("jq", "undefined function", "jq -n 'nope(1)'", quiet_error=True)
add("jq", "syntax error", "jq -n '1 +'", quiet_error=True)
add("jq", "invalid json input", "echo '{a:1}' | jq .", quiet_error=True)
add("jq", "invalid json after valid", "printf '1 2 x' | jq -c .", quiet_error=True)
add("jq", "truncated json", "printf '[1,2' | jq -c .", quiet_error=True)
add("jq", "missing file", "jq . /tmp/nosuch", quiet_error=True)
add("jq", "missing file continues", "echo 1 >/tmp/a; jq . /tmp/nosuch /tmp/a", quiet_error=True)
add("jq", "missing filter", "jq", quiet_error=True)
add("jq", "unknown option", "jq --bogus .", quiet_error=True)
add("jq", "runtime error status", "echo '{}' | jq '.a.b.c = (1 | .x)'", quiet_error=True)
add("jq", "runtime error continues inputs", "printf '1 \"a\" 2' | jq '. + 1'", quiet_error=True)
add("jq", "error with exit status", "echo 1 | jq -e 'error(\"x\")'", quiet_error=True)
add("jq", "import refused", "jq -n 'import \"x\" as x; 1'", quiet_error=True)
add("jq", "combined short options", "echo '{\"a\":\"x\"}' | jq -rc .a")
add("jq", "args then files", "jq -n -c '$ARGS.positional' --args a -- -b")
add("jq", "double dash filter", "jq -n -- '-1'")
add("jq", "unicode escapes output", "jq -n '\"\\u00e9\\ud83d\\ude00\"'")
add("jq", "control characters escaped", "jq -n '\"\\u0001\\u001f\\u007f\"'")
add("jq", "large integer precision", "echo '12345678901234567890' | jq .")
add("jq", "number formatting", "echo '[1.0, 1e2, 0.1, 1E-5, -0.0, 100000000000000000]' | jq -c .")
add("jq", "duplicate keys", "echo '{\"a\":1,\"a\":2}' | jq -c .")
add("jq", "deep nesting", "printf '%.0s[' $(seq 1 200) >/tmp/d; printf '%.0s]' $(seq 1 200) >>/tmp/d; jq -c 'getpath([range(199)|0])' /tmp/d")
add("jq", "bom input", "printf '\\357\\273\\2771' | jq .", quiet_error=True)
add("jq", "nan input", "echo 'NaN' | jq .", quiet_error=True)
add("jq", "empty input", "printf '' | jq .; echo \"status=$?\"")
add("jq", "whitespace input", "printf ' \\n ' | jq .; echo \"status=$?\"")
