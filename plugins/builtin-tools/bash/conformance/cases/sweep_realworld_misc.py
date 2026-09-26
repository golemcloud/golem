"""Real-world usage: tldr-pages examples for printf, seq, expr, factor, numfmt, date, env, sleep,
yes, xargs, tee, jq, sh and bash.

Source: tldr-pages (https://github.com/tldr-pages/tldr, CC BY 4.0), pages/common and pages/linux
at commit 8cf22035e60b167ebf224c1c527b27e0b5dca24d; see ../NOTICE. Each example runs with its
placeholders bound, then over the argument shapes its command meets: edge numbers, empty input,
missing operands, bad values. Clocks are pinned (`date -d`, TZ=UTC), sleeps are short, and random
output (shuf, mktemp) is only checked for its shape.
"""

TIER = "sweep"

W = "mkdir -p /tmp/w && cd /tmp/w"
CASES = []


def add(cmd, slug, body, setup=W, error=False):
    tags = ("cmd." + cmd,) + (("error",) if error else ())
    CASES.append(("real tldr " + cmd + ": " + slug, setup + "\n" + body, tags))


# -- printf ---------------------------------------------------------------------------------------
add("printf", "text message", 'printf "%s\\n" "Hello world"')
for _label, _args in (
    ("several arguments reuse the format", '"a" "b c" "d"'),
    ("no argument", ""),
    ("an empty argument", '""'),
    ("an argument with a backslash", "'a\\nb'"),
    ("a multibyte argument", "'héllo 日本'"),
):
    add("printf", "text message with " + _label, 'printf "%s\\n" ' + _args)
for _n in ("42", "7", "-5", "1234", "0x1F", "010", "x"):
    add("printf", "integer in bold blue " + _n, 'printf "\\e[1;34m%.3d\\e[0m\\n" ' + _n)
for _n in ("123.4", "0.005", "-1", "1e3", "2.675", "abc", "1,5"):
    add("printf", "float with the euro sign " + _n, 'printf "\\u20AC %.2f\\n" ' + _n)
add("printf", "environment variables", 'VAR1=one VAR2="two words"\nprintf "var1: %s\\tvar2: %s\\n" "$VAR1" "$VAR2"')
add("printf", "environment variables unset", 'printf "var1: %s\\tvar2: %s\\n" "$VAR1" "$VAR2"')
add("printf", "environment variables one missing", 'VAR1=one\nprintf "var1: %s\\tvar2: %s\\n" "$VAR1"')
add("printf", "store in a variable", 'printf -v myvar "This is %s = %d\\n" "a year" 2016; printf "[%s]" "$myvar"')
add("printf", "store in a variable with a bad number", 'printf -v myvar "This is %s = %d\\n" "a year" 20x16; echo "status=$?"; printf "[%s]" "$myvar"', error=True)
add("printf", "store in an array element", 'printf -v arr[2] "%s-%s" a b; declare -p arr')
add("printf", "store in an invalid name", 'printf -v 1bad "%s" x; echo "status=$?"', error=True)
for _args in ("0xFF 0377 100000", "255 255 1", "-1 8 0.000123", "0x7fffffffffffffff 0 1e300", "'A' '\"B' -0"):
    add("printf", "hex octal scientific " + _args, 'printf "hex=%x octal=%o scientific=%e\\n" ' + _args)
add("printf", "width and precision from arguments", 'printf "[%*s][%-*s][%.*f]\\n" 5 ab 4 c 2 3.14159')
add("printf", "shell-quoted and escaped", 'printf "%q %b\\n" "a b\'c" "x\\ty"')
add("printf", "percent literal and character", 'printf "100%% %c\\n" hello')

# -- seq ------------------------------------------------------------------------------------------
for _args in ("10", "10 20", "5 3 20", "0 0.5 2", "5 -1 1", "5 1", "-3 -1", "1 0.1 1.3", "1e2 1e2", "3 x"):
    add("seq", "sequence " + _args, "seq " + _args, error=_args == "3 x")
for _flag in ("-s", "--separator"):
    add("seq", "separator " + _flag, "seq " + _flag + ' " " 5 3 20')
add("seq", "empty separator", "seq -s '' 1 5")
for _flag in ("-f", "--format"):
    add("seq", "format " + _flag, "seq " + _flag + ' "%04g" 5 3 20')
add("seq", "format with a float", 'seq -f "%.2f" 0 0.25 1')
add("seq", "format without a directive", 'seq -f "x" 1 2', error=True)
for _flag in ("-w", "--equal-width"):
    add("seq", "equal width " + _flag, "seq " + _flag + " 5 3 20")
add("seq", "equal width with negatives", "seq -w -5 5 10")
add("seq", "equal width with decimals", "seq -w 0.5 0.5 2")
add("seq", "zero increment", "seq 1 0 3", error=True)
add("seq", "no operands", "seq", error=True)

# -- expr -----------------------------------------------------------------------------------------
for _s in ("string", "", "héllo", "a b"):
    add("expr", "length of '" + _s + "'", 'expr length "' + _s + '"; echo "status=$?"')
for _args in ("2 3", "1 100", "0 2", "5 0", "3 x"):
    add("expr", "substr " + _args, 'expr substr "string" ' + _args + '; echo "status=$?"')
for _p in ("st.*", "str", "x", "\\(s.r\\)", "s*", "\\(ab\\)*"):
    add("expr", "match '" + _p + "'", "expr match \"string\" '" + _p + "'; echo \"status=$?\"")
for _c in ("ri", "g", "xyz", ""):
    add("expr", "index of '" + _c + "'", 'expr index "string" "' + _c + '"; echo "status=$?"')
for _op in ("+", "-", "\\*", "/", "%"):
    for _a, _b in (("7", "3"), ("-7", "2"), ("7", "0")):
        add("expr", "arithmetic " + _a + " " + _op + " " + _b, "expr " + _a + " " + _op + " " + _b + '; echo "status=$?"')
add("expr", "arithmetic overflow", "expr 9223372036854775807 + 1; echo \"status=$?\"")
add("expr", "arithmetic on a non-integer", "expr 1.5 + 1; echo \"status=$?\"")
for _a, _b in (("5", "7"), ("0", "7"), ('""', "0"), ("0", "0")):
    add("expr", "or " + _a + " " + _b, "expr " + _a + " \\| " + _b + '; echo "status=$?"')
    add("expr", "and " + _a + " " + _b, "expr " + _a + " \\& " + _b + '; echo "status=$?"')
add("expr", "comparison of strings and numbers", "expr 10 \\< 9; expr a \\< b; expr 10 = 10.0; echo \"status=$?\"")
add("expr", "colon match", "expr 'abc123' : '[a-z]*'; expr 'abc123' : '[a-z]*\\([0-9]*\\)'")
add("expr", "parentheses", "expr \\( 2 + 3 \\) \\* 4")
add("expr", "missing argument", "expr 1 +; echo \"status=$?\"")

# -- factor ---------------------------------------------------------------------------------------
for _n in ("84", "1", "0", "97", "2305843009213693951", "18446744073709551615", "4294967297", "-6", "12 13 14", "abc", "08"):
    add("factor", "factorize " + _n, "factor " + _n + '; echo "status=$?"')
add("factor", "from stdin", "echo 84 | factor")
add("factor", "several from stdin", "printf '6\\n  15 21\\n\\n' | factor")
add("factor", "empty stdin", "printf '' | factor; echo \"status=$?\"")

# -- numfmt ---------------------------------------------------------------------------------------
for _v in ("1.5K", "1K", "1.5M", "999", "1.5", "1.5Ki", "abc"):
    add("numfmt", "from si " + _v, "numfmt --from si " + _v + '; echo "status=$?"')
for _v in ("1500", "999", "1000", "1001", "1234567", "0", "-1500", "1.5"):
    add("numfmt", "to si " + _v, "numfmt --to si " + _v)
for _v in ("1.5K", "1K", "3G"):
    add("numfmt", "from iec " + _v, "numfmt --from iec " + _v)
for _v in ("1.5Ki", "2Mi", "1.5K", "1.5"):
    add("numfmt", "from auto " + _v, "numfmt --from auto " + _v + '; echo "status=$?"')
add("numfmt", "field with a header to iec", "printf 'name size\\na 1048576\\nb 2048\\nc 1536\\n' | numfmt --header=1 --field 2 --to iec")
add("numfmt", "field with a header to iec from a listing", "printf 'h1 h2 h3 h4 SIZE name\\n- 1 u g 4096 d\\n- 1 u g 123456789 f\\n' | numfmt --header=1 --field 5 --to iec")
add("numfmt", "left aligned padded format", "printf '2048\\ta\\n1536000\\tb\\n12\\tc\\n' | numfmt --to iec --format \"%-5f\"")
add("numfmt", "padding", "numfmt --to iec --padding 8 1048576 1")
add("numfmt", "suffix", "numfmt --to si --suffix B 1500")
add("numfmt", "invalid field", "echo 1 2 | numfmt --field 3 --to si; echo \"status=$?\"")

# -- date -----------------------------------------------------------------------------------------
for _ts in ("1473305798", "0", "-1", "2147483648", "1700000000.5"):
    add("date", "locale format of @" + _ts, "TZ=UTC date -d @" + _ts + " +%c")
    add("date", "iso 8601 in utc of @" + _ts, "date -u +%Y-%m-%dT%H:%M:%SZ -d @" + _ts)
    add("date", "default format of @" + _ts, "TZ=UTC date -d @" + _ts)
    add("date", "rfc 3339 seconds of @" + _ts, "TZ=UTC date --rfc-3339 seconds -d @" + _ts)
add("date", "utc (long)", "date --utc +%Y-%m-%dT%H:%M:%SZ --date @1473305798")
add("date", "unix timestamp of a date", 'date -d "2018-09-01 00:00" +%s -u')
add("date", "unix timestamp of a date (long)", 'date --date "2018-09-01 00:00" +%s --utc')
for _d in ("2018-09-01", "2018-09-01T12:34:56Z", "2018-09-01 12:34:56 +0200", "Sep 1 2018", "1 Sep 2018 10:00", "2018-02-29", "2020-02-29 +1 year", "2024-01-31 +1 month", "2018-09-01 next day", "garbage"):
    add("date", "unix timestamp of '" + _d + "'", 'date -u -d "' + _d + '" +%s; echo "status=$?"')
for _p in ("date", "ns"):
    add("date", "rfc 3339 " + _p, "TZ=UTC date --rfc-3339 " + _p + " -d @1473305798")
add("date", "rfc 3339 with a bad precision", "date --rfc-3339 minutes -d @0", error=True)
for _d in ("2021-01-03", "2021-01-04", "2020-12-31", "2026-06-15"):
    add("date", "iso week number of " + _d, "date -u -d " + _d + " +%V")
add("date", "iso 8601 option", "TZ=UTC date -I -d @1473305798; TZ=UTC date --iso-8601=seconds -d @1473305798")
add("date", "rfc email option", "TZ=UTC date -R -d @1473305798")
add("date", "many format directives", "TZ=UTC date -d @1473305798 '+%a %A %b %B %d %e %H %I %j %m %M %p %S %u %w %y %Y %Z %z %%'")
add("date", "padding modifiers", "TZ=UTC date -d @1473305798 '+%-d %_m %05Y %^a %#b'")
# A POSIX TZ string: the oracle image has no zone database, so an IANA name there falls back to UTC.
add("date", "another time zone", "TZ=EST5EDT,M3.2.0,M11.1.0 date -d @1473305798 '+%F %T %Z'")
add("date", "a fixed offset time zone", "TZ=UTC-3 date -d @1473305798 '+%F %T %z'")
add("date", "file modification time", "touch -d '2020-05-06 07:08:09' f && TZ=UTC date -r f '+%F %T'")

# -- env ------------------------------------------------------------------------------------------
add("env", "show the environment's golem variables", "env | grep '^GOLEM_' | sort")
add("env", "run a program", "env echo hello")
add("env", "run a missing program", "env nosuchprogram", error=True)
add("env", "clear the environment", "env -i env; echo \"status=$?\"")
add("env", "clear the environment (long)", "X=1 env --ignore-environment bash -c 'echo \"[${X-unset}]\"'")
add("env", "clear the environment and set one variable", "env -i Y=2 env")
add("env", "unset a variable", "X=1 env -u X bash -c 'echo \"[${X-unset}]\"'")
add("env", "unset a variable (long)", "export X=1; env --unset X bash -c 'echo \"[${X-unset}]\"'")
add("env", "set a variable", "env GREETING=hi bash -c 'echo $GREETING'")
add("env", "set several variables", "env A=1 B=2 C=3 bash -c 'echo $A$B$C'")
add("env", "set a variable with an equals sign in its value", "env V=a=b bash -c 'echo $V'")
add("env", "run under a different name", "env -a custom bash -c 'echo $0'")
add("env", "run under a different name (long)", "env --argv0 custom bash -c 'echo $0'")
add("env", "change directory", "mkdir -p /tmp/w/sub && env -C /tmp/w/sub pwd")
add("env", "null-terminated output", "env -i A=1 B=2 env -0 | od -c")

# -- sleep ----------------------------------------------------------------------------------------
for _t in ("0.1", "0", ".05", "1e-2", "0.001m", "0.00001h", "0.000001d", "0.05s", "0.01 0.02"):
    add("sleep", "delay " + _t, "sleep " + _t + " && echo done")
for _t in ("x", "-1", "1q", ""):
    add("sleep", "invalid delay '" + _t + "'", "sleep " + _t + '; echo "status=$?"', error=True)
add("sleep", "then a command", "sleep 0.1 && echo 'after delay'")

# -- yes ------------------------------------------------------------------------------------------
add("yes", "repeat y", "yes | head -n 3")
add("yes", "repeat a value", "yes value | head -n 2")
add("yes", "repeat several words", "yes a b | head -n 2")
add("yes", "repeat an empty line", "yes '' | head -n 3 | od -c")
add("yes", "accept prompts of a reader", "yes | { read a; read b; echo \"$a$b\"; }")
add("yes", "bytes", "yes abc | head -c 10")
add("yes", "status in a pipeline", "yes | head -n 1; echo \"${PIPESTATUS[@]}\"")

# -- xargs ----------------------------------------------------------------------------------------
XIN = ["printf 'a b\\nc\\n'", "printf ''", "printf '\"x y\" z\\n'", "printf 'one\\n\\n  two  \\n'", "printf \"it's\\n\"", "printf 'a\\\\ b c\\n'"]
XLABEL = ["words", "empty input", "quoted input", "blank lines and spaces", "an unmatched quote", "an escaped space"]
for _i, _src in enumerate(XIN):
    add("xargs", "use input as arguments with " + XLABEL[_i], _src + " | xargs echo; echo \"status=$?\"")
    add("xargs", "one argument per command with " + XLABEL[_i], _src + " | xargs -n 1 echo; echo \"status=$?\"")
    add("xargs", "placeholder with " + XLABEL[_i], _src + " | xargs -I _ echo '[_]' extra; echo \"status=$?\"")
add("xargs", "chained commands in sh -c", "printf 'a\\nb\\n' | xargs sh -c \"echo start && echo \\$0 \\$@ | tr a-z A-Z\"")
add("xargs", "chained commands in sh -c with a placeholder name", "printf 'a\\nb\\n' | xargs sh -c 'echo \"$0|$*\"' _")
add("xargs", "one argument per command (long)", "printf 'a b c\\n' | xargs --max-args 1 echo")
add("xargs", "two arguments per command", "printf 'a b c\\n' | xargs -n 2 echo")
add("xargs", "parallel", "printf '1 2 3 4\\n' | xargs -P 10 -n 1 echo | sort")
add("xargs", "parallel (long)", "printf '1 2 3\\n' | xargs --max-procs 2 --max-args 1 echo | sort")
add("xargs", "parallel with failures", "printf '1 2 3\\n' | xargs -P 3 -n 1 sh -c 'exit $0'; echo \"status=$?\"")
add("xargs", "placeholder several times", "printf 'a\\nb\\n' | xargs -I {} echo {}-{}")
add("xargs", "read arguments from a file", W + "\nprintf 'f1\\nf2\\n' >list && xargs -a list echo")
add("xargs", "read arguments from a file (long)", W + "\nprintf 'f1\\nf2\\n' >list && xargs --arg-file list echo got")
add("xargs", "read arguments from a missing file", "xargs -a nosuchlist echo", error=True)
add("xargs", "null-delimited input", "printf 'a b\\0c\\0' | xargs -0 -n 1 echo")
add("xargs", "delimiter", "printf 'a,b,c' | xargs -d , -n 1 echo")
add("xargs", "no run if empty", "printf '' | xargs -r echo ran; echo \"status=$?\"")
add("xargs", "a command that fails", "printf 'a\\n' | xargs false; echo \"status=$?\"")
add("xargs", "a command that is missing", "printf 'a\\n' | xargs nosuchcmd; echo \"status=$?\"")
add("xargs", "a command that exits 255", "printf 'a\\nb\\n' | xargs -n 1 sh -c 'exit 255'; echo \"status=$?\"")
add("xargs", "default command", "printf 'a b\\n' | xargs")
add("xargs", "max chars", "printf 'aa bb cc dd\\n' | xargs -s 12 echo")
add("xargs", "lines at a time", "printf 'a b\\nc\\nd e\\n' | xargs -L 2 echo")
add("xargs", "trace commands", "printf 'a b\\n' | xargs -t echo")

# -- tee ------------------------------------------------------------------------------------------
add("tee", "copy stdin to a file and stdout", W + '\necho "example" | tee f && cat f')
add("tee", "copy stdin to several files", W + '\necho "example" | tee f g >/dev/null && cat f g')
add("tee", "overwrite an existing file", W + '\necho old >f && echo "example" | tee f && cat f')
add("tee", "append", W + '\necho old >f && echo "example" | tee -a f && cat f')
add("tee", "append (long)", W + '\necho old >f && echo "example" | tee --append f >/dev/null && cat f')
add("tee", "into a missing directory", W + '\necho "example" | tee nodir/f; echo "status=$?"', error=True)
add("tee", "into a directory", W + '\nmkdir d && echo "example" | tee d; echo "status=$?"', error=True)
add("tee", "to stderr and another program", 'echo "example" | tee /dev/stderr | xargs printf "[%s]"')
add("tee", "to process substitutions", W + '\necho "example" | tee >(xargs mkdir) >(wc -c) | sort; sleep 0.2; ls -d example')
add("tee", "empty input", W + "\nprintf '' | tee f; wc -c f")

# -- jq -------------------------------------------------------------------------------------------
JSON = [
    ("an object", '{"key1": "value1", "key2": {"nestedKey": 2}, "list": [1, 2, 3]}'),
    ("a compact object", '{"a":1,"b":[true,false,null]}'),
    ("unicode strings", '{"s": "caf\\u00e9 \\u65e5\\u672c", "e": "\\ud83d\\ude00", "raw": "日本"}'),
    ("control characters", '{"s": "tab\\there\\nnew \\u0001 \\u007f \\\\ \\"q\\""}'),
    ("integers", '[0, -0, 1, -1, 123456789012, 9007199254740993, 100000000000000000000]'),
    ("floats", '[1.0, 1.5, -2.25, 0.1, 1e3, 1E-3, 1.5e300, 3.0e0]'),
    ("huge numbers", '[1e1000, -1e1000, 1e-400]'),
    ("nested empties", '{"a": {}, "b": [], "c": [[]], "d": {"e": {}}}'),
    ("several documents", '1 "two" [3] {"four": 4}'),
    ("an empty input", ''),
    ("only whitespace", '  \n\t '),
    ("duplicate keys", '{"a": 1, "a": 2}'),
    ("a top-level string", '"just text"'),
    ("deep nesting", '[[[[[[[[[[1]]]]]]]]]]'),
]
for _label, _doc in JSON:
    _setup = W + "\nprintf '%s\\n' '" + _doc.replace("'", "'\\''") + "' >file.json"
    add("jq", "pretty print " + _label, _setup + "\njq '.' file.json")
    add("jq", "compact " + _label, _setup + "\njq -c . file.json")
_obj = W + "\nprintf '%s\\n' '{\"key1\": \"value1\", \"key2\": {\"nestedKey\": 2}}' >file.json"
_arr = W + "\nprintf '%s\\n' '[{\"key1\": \"value1\", \"key2\": \"value2\"}, {\"key1\": \"value1\", \"key2\": \"x\"}, {\"key1\": \"y\", \"key2\": \"value2\"}]' >file.json"
add("jq", "execute a script file", _obj + "\nprintf '.key2 | .nestedKey * 10\\n' >script.jq && cat file.json | jq -f script.jq")
add("jq", "execute a script file (long)", _obj + "\nprintf '{k: .key1}\\n' >script.jq && cat file.json | jq --from-file script.jq")
add("jq", "execute a missing script file", _obj + "\ncat file.json | jq -f nosuch.jq; echo \"status=$?\"", error=True)
add("jq", "pass named arguments", _obj + "\ncat file.json | jq --arg \"name1\" \"value1\" --arg \"name2\" \"value2\" '. + $ARGS.named'")
add("jq", "pass named arguments with json", _obj + "\ncat file.json | jq -c --argjson n 5 --arg s 5 '$ARGS.named'")
add("jq", "pass positional arguments", "jq -n -c '$ARGS' --args a b")
add("jq", "new object from several files", W + "\nprintf '{\"key1\": 1, \"key2\": {\"nestedKey\": \"a\"}}\\n' >multiple_json_file_1.json && printf '{\"key1\": 2, \"key2\": {}}\\n' >multiple_json_file_2.json && cat multiple_json_file_*.json | jq '{newKey1: .key1, newKey2: .key2.nestedKey}'")
add("jq", "specific array items", _arr + "\ncat file.json | jq '.[0], .[2]'")
add("jq", "array items out of range", _arr + "\ncat file.json | jq -c '.[5], .[-1]'")
add("jq", "all array values", _arr + "\ncat file.json | jq -c '.[]'")
add("jq", "all object values", _obj + "\ncat file.json | jq '.[]'")
add("jq", "two-condition filter", _arr + "\ncat file.json | jq '.[] | select((.key1==\"value1\") and .key2==\"value2\")'")
add("jq", "two-condition filter with or", _arr + "\ncat file.json | jq -c '.[] | select(.key1==\"y\" or .key2==\"x\")'")
add("jq", "add keys", _obj + "\ncat file.json | jq '. + {\"key1\": \"new\", \"key3\": \"value3\"}'")
add("jq", "remove keys with del", _obj + "\ncat file.json | jq -c 'del(.key1)'")
add("jq", "remove array elements", "echo '[1,2,3,2]' | jq -c '. - [2]'")
add("jq", "raw output", _arr + "\ncat file.json | jq -r '.[].key1'")
add("jq", "sort keys", "echo '{\"b\":1,\"a\":{\"d\":2,\"c\":3}}' | jq -S .")
add("jq", "tab indentation", "echo '{\"a\":[1]}' | jq --tab .")
add("jq", "indent 1", "echo '{\"a\":[1]}' | jq --indent 1 .")
add("jq", "null input", "jq -n '1 + 1'")
add("jq", "exit status", "echo 'null' | jq -e .; echo \"status=$?\"; echo 'false' | jq -e .; echo \"status=$?\"")
add("jq", "join output", "echo '[\"a\",\"b\"]' | jq -j '.[]'; echo")
add("jq", "ascii output", "echo '\"caf\\u00e9 \\ud83d\\ude00\"' | jq -a .")

# -- sh and bash ----------------------------------------------------------------------------------
add("sh", "execute a command", 'sh -c "echo hello from sh"')
add("sh", "execute a command with arguments", "sh -c 'echo \"$0 $1 $#\"' name one two")
add("sh", "execute a command that fails", "sh -c 'exit 3'; echo \"status=$?\"")
add("sh", "execute a command with a syntax error", "sh -c 'if then'; echo \"status=$?\"", error=True)
add("sh", "execute a command with no command string", "sh -c; echo \"status=$?\"", error=True)
add("sh", "read commands from stdin", "echo 'echo from stdin' | sh -s")
add("sh", "read commands from stdin with arguments", "echo 'echo \"$1-$2\"' | sh -s a b")
add("sh", "read commands from stdin with no -s", "printf 'echo one\\necho two\\n' | sh")
add("bash", "execute commands", "bash -c \"echo 'bash is executed'\"")
add("bash", "execute commands from stdin", "echo \"echo 'bash is executed'\" | bash")
add("bash", "execute commands from stdin without rc files", "echo \"echo 'norc'\" | bash --norc")
add("bash", "execute commands from stdin that fail", "printf 'echo a\\nfalse\\n' | bash; echo \"status=$?\"")
add("bash", "execute commands and print them", "bash -x -c 'a=1; echo $a'")
add("bash", "execute commands and stop at the first error", "bash -e -c 'echo one; false; echo two'; echo \"status=$?\"")
add("bash", "execute commands with a syntax error", "bash -c 'echo ok; fi'; echo \"status=$?\"", error=True)
add("bash", "execute commands with an unset variable check", "bash -u -c 'echo $nope'; echo \"status=$?\"", error=True)
add("bash", "execute commands with pipefail", "bash -o pipefail -c 'false | true'; echo \"status=$?\"")
add("bash", "execute commands with an exit status", "bash -c 'exit 42'; echo \"status=$?\"")
add("bash", "execute commands with arguments", "bash -c 'printf \"<%s>\" \"$@\"; echo' b0 'x y' z")
add("bash", "nested execute commands", "bash -c \"bash -c 'echo nested \\$0' inner\"")
add("bash", "execute commands with a heredoc on stdin", "bash <<'EOF'\nx=5\necho \"x is $x\"\nEOF")


# Dropped after recording, with the reason for each group.
# GNU quotes the name with curly quotes under the oracle's UTF-8 locale and nothing else differs: the documented
# deliberate difference UTF8_QUOTES (env_facts.py, command_errors.py).
_DROPPED_Q = (
    "real tldr date: unix timestamp of '2018-02-29'",
    "real tldr date: unix timestamp of 'garbage'",
    'real tldr expr: missing argument',
    'real tldr factor: factorize abc',
    'real tldr numfmt: from si 1.5Ki',
    'real tldr numfmt: from si abc',
    "real tldr sleep: invalid delay '1q'",
    "real tldr sleep: invalid delay 'x'",
)
_DROPPED = set(_DROPPED_Q)
CASES = [case for case in CASES if case[0] not in _DROPPED]
