"""Every expansion: parameter operators, arithmetic, substitution, splitting and globbing."""

CASES = [
    ("expand: process substitution as operands", 'diff <(printf "a\\n") <(printf "b\\n"); echo status=$?; paste <(seq 2) <(seq 3 4); comm <(printf "a\\nb\\n") <(printf "a\\nc\\n")', ["expansion.process"]),
    ("expand: process substitution as input", 'while read l; do echo "got $l"; done < <(printf "x\\ny\\n"); wc -l < <(seq 3); grep -c a <(printf "a\\na\\nb\\n")', ["expansion.process"]),
    # Bash runs `>(list)` alongside the command; the sleeps let its output land in order.
    ("expand: process substitution as output", 'printf "t1\\nt2\\n" | tee >(wc -l) >/dev/null; sleep 0.2; echo hi > >(tr a-z A-Z); sleep 0.2; echo done', ["expansion.process"]),
    ("expand: process substitution path", 'echo <(true) | grep -c "^/dev/fd/"; f() { cat "$1"; }; f <(echo via-function)', ["expansion.process"]),
    # Parameter operators
    ("expand: default and assign", 'unset u; e=; echo "${u-a} ${u:-b} ${e-c} ${e:-d}"; echo "${u=x} $u"; echo "${e:=y} $e"'),
    ("expand: alternative", 'unset u; e=; s=set; echo "[${u+a}] [${u:+b}] [${e+c}] [${e:+d}] [${s:+e}]"'),
    ("expand: error operator", 'unset u; ( : "${u?custom message}" ); echo status=$?; ( : "${u:?}" ) 2>/dev/null; echo status=$?'),
    ("expand: error operator on empty", 'e=; ( echo "${e:?empty}" ); echo status=$?; echo "${e?not reached}" | wc -c'),
    ("expand: length", 's=héllo; a=(x yy zzz); echo ${#s} ${#a[@]} ${#a[1]} ${#undefined}'),
    ("expand: substring", 's=abcdefgh; echo ${s:2} ${s:2:3} ${s: -3} ${s: -3:2} ${s:0:-2} ${s:10}'),
    ("expand: substring of arrays and positionals", 'a=(a b c d e); echo ${a[@]:1:2} ${a[@]: -2}; set -- 1 2 3 4; echo ${@:2} ${@:0:2}'),
    ("expand: trim patterns", 'p=/usr/local/lib/libfoo.so.1; echo ${p#*/} ${p##*/} ${p%.*} ${p%%.*} ${p#nomatch}'),
    ("expand: trim with brackets and classes", 'v=abc123def; echo ${v%%[0-9]*} ${v#[[:alpha:]]} ${v##*[[:digit:]]}'),
    ("expand: replace forms", 'v=a-b-c-a; echo ${v/a/X} ${v//a/X} ${v/#a/X} ${v/%a/X} ${v//-} ${v/b}'),
    ("expand: replace with patterns and slashes", 'p=/a/b/c; echo ${p//\\//_} ${p//[ab]/Z} ${p/\\/a/root}; x="a b"; echo ${x// /_}'),
    ("expand: replace on arrays", 'a=(foo bar baz); echo ${a[@]/a/A} ${a[@]//a}; echo ${a[@]/#/-}'),
    ("expand: case modification", 'w="hello world"; echo ${w^} ${w^^} ${w,} ; u=HELLO; echo ${u,,} ${u,} ${w^^o}'),
    ("expand: transform operators", 'v="a b\'c"; echo ${v@Q}; e=\'x\\ty\'; echo ${e@E}; declare -i n=3; echo ${n@a}; a=(1 2); echo ${a@A}; echo ${v@U} ${v@L} ${v@u}'),
    ("expand: prefix names", 'abc1=1 abc2=2 abd=3; echo ${!abc*}; echo "${!abc@}"'),
    ("expand: array keys", 'a=(x y z); a[7]=q; echo ${!a[@]}; declare -A m=([one]=1); echo ${!m[@]}'),
    ("expand: indirection", 'target=value; ref=target; echo ${!ref}; arr=(a b); r2="arr[1]"; echo ${!r2}; unset nothing; n=nothing; echo "[${!n}]"'),
    ("expand: nested defaults", 'unset a b; echo ${a:-${b:-deep}}; b=mid; echo ${a:-${b:-deep}}; echo "${a:-"quoted default"}"'),
    ("expand: unset in arithmetic and strings", 'unset u; echo $((u + 1)) "[$u]"'),
    # Arithmetic
    ("expand: arithmetic operators", 'echo $((7/2)) $((7%3)) $((-7/2)) $((-7%3)) $((2**10)) $((1<<4)) $((255>>4)) $((5&3)) $((5|3)) $((5^3)) $((~5)) $((!0)) $((!5))'),
    ("expand: arithmetic precedence and ternary", 'echo $((1+2*3)) $(((1+2)*3)) $((2**3**2)) $((1?2:3)) $((0?2:3)) $((1,2,3)) $((3>2 && 2>1)) $((0||0))'),
    ("expand: arithmetic assignment operators", 'x=5; echo $((x+=2)) $((x-=1)) $((x*=3)) $((x/=2)) $((x%=4)) $((x<<=2)) $((x|=1)) $((x++)) $x $((++x)) $((x--)) $((--x))'),
    ("expand: arithmetic bases", 'echo $((0x1f)) $((017)) $((2#1010)) $((36#z)) $((16#ff)) $((64#_))'),
    ("expand: arithmetic with variables and strings", 'a=3 b=a; echo $((b+1)) $((a*a)); c="2+3"; echo $((c*2)); echo $(( a > 2 ))'),
    ("expand: arithmetic errors", 'echo $((1/0)); echo status=$?; echo $((2**-1)); echo status=$?; echo $((1+)); echo status=$?', ["error"]),
    ("expand: arithmetic overflow wraps", 'echo $((9223372036854775807+1)) $((-9223372036854775808-1)) $((2**63))'),
    ("expand: arithmetic command status", '((0)); echo $?; ((5)); echo $?; ((x=3)); echo $x; (( y = x * 2 )) && echo $y', ["compound.arith"]),
    ("expand: legacy arithmetic", 'echo $[1+2] $[3*4]'),
    # Command substitution
    ("expand: command substitution strips trailing newlines", 'v=$(printf "a\\n\\n\\n"); echo "[$v]"; w=$(printf "\\n\\na"); echo "[$w]"'),
    ("expand: backquotes and nesting", 'echo `echo a` $(echo $(echo b)) "$(echo "$(echo c d)")" `echo \\`echo e\\``'),
    ("expand: substitution reads a file", 'echo content >/tmp/f; v=$(< /tmp/f); echo "$v"; v=$(</tmp/nosuch); echo status=$?'),
    ("expand: substitution status", 'v=$(exit 3); echo $?; v=$(true; false); echo $?; x=$(echo ok) || echo no; echo $x'),
    ("expand: substitution in a subshell environment", 'x=1; y=$(x=2; echo $x); echo $x $y; $(cd /tmp); pwd'),
    # Word splitting and IFS
    ("expand: default splitting", 'v="  a  b\tc\n d "; set -- $v; echo $#; printf "[%s]" "$@"; echo'),
    ("expand: custom IFS", 'IFS=:; v="a::b:c:"; set -- $v; echo $#; printf "[%s]" "$@"; echo'),
    ("expand: IFS mixing whitespace and delimiters", 'IFS=" ,"; v=" a , b,,c "; set -- $v; echo $#; printf "[%s]" "$@"; echo'),
    ("expand: empty IFS disables splitting", 'IFS=; v="a b c"; set -- $v; echo $#'),
    ("expand: quoted at and star", 'set -- "a b" c; printf "[%s]" "$@"; echo; printf "[%s]" "$*"; echo; IFS=-; printf "[%s]" "$*"; echo; printf "[%s]" $*; echo'),
    ("expand: array star with IFS", 'a=(x y z); IFS=,; echo "${a[*]}"; unset IFS; echo "${a[*]}"'),
    ("expand: empty and unset words vanish", 'e=; set -- $e "" $e a; echo $#; f() { echo $#; }; f "$@"; f "${@}" ""'),
    ("expand: splitting results of substitution", 'set -- $(printf "a b\\nc"); echo $#; set -- "$(printf "a b\\nc")"; echo $#'),
    # Brace expansion
    ("expand: brace lists and sequences", 'echo {a,b,c} x{1..3}y {5..1} {a..e..2} {01..10..3} {-2..2}'),
    ("expand: brace nesting and invalid forms", 'echo {a,{b,c}d} {a} {a..} {1..3..0} "{a,b}" \\{a,b\\} {,x}'),
    ("expand: brace with variables is literal", 'n=3; echo {1..$n}; x=a,b; echo {$x}'),
    # Globbing
    ("expand: glob basics", 'cd /tmp; touch a1 a2 b1 .hidden; echo a*; echo ?1; echo [ab]1; echo [!a]1; echo *; echo nomatch*'),
    ("expand: glob ordering and dirs", 'cd /tmp; mkdir -p d1/sub d2; touch d1/x d2/y; echo */; echo d*/*; echo ./d?'),
    ("expand: quoted globs are literal", 'cd /tmp; touch star1; echo "star*" \'star*\' star\\*; v="star*"; echo $v "$v"'),
    ("expand: nullglob and failglob", 'cd /tmp; shopt -s nullglob; echo x nomatch* y; shopt -u nullglob; shopt -s failglob; echo nomatch*; echo status=$?', ["expansion.glob.options"]),
    ("expand: dotglob", 'cd /tmp; touch .dot vis; echo *; shopt -s dotglob; echo *', ["expansion.glob", "expansion.glob.options"]),
    ("expand: nocaseglob", 'cd /tmp; touch Upper lower; shopt -s nocaseglob; echo u*; echo L*', ["expansion.glob.options"]),
    ("expand: extglob patterns", 'shopt -s extglob; cd /tmp; touch a.txt b.log c.txt abc; echo *.@(txt|log); echo !(*.txt); echo +(a|b)c; echo ?(a)bc', ["expansion.glob.options"]),
    # `shopt -s extglob` affects parsing, so on the same line as the patterns it is too late.
    ("expand: extglob enabled on the same line", 'shopt -s extglob; echo @(a|b)', ["expansion.glob.options"]),
    ("expand: extglob off by default", 'shopt -q extglob; echo status=$?; v=aaab; echo ${v##+(a)}', ["expansion.glob.options"]),
    ("expand: extglob patterns on a later line", 'shopt -s extglob\ncd /tmp; touch a.txt b.log c.txt abc; echo *.@(txt|log); echo !(*.txt); echo +(a|b)c; echo ?(a)bc', ["expansion.glob.options"]),
    ("expand: extglob in conditions and case on a later line", 'shopt -s extglob\n[[ abcabc == +(abc) ]] && echo plus; case x.tar.gz in *.@(gz|bz2)) echo compressed;; esac; v=aaab; echo ${v##+(a)}', ["expansion.glob.options"]),
    ("expand: extglob in conditions and case", 'shopt -s extglob; [[ abcabc == +(abc) ]] && echo plus; case x.tar.gz in *.@(gz|bz2)) echo compressed;; esac; v=aaab; echo ${v##+(a)}', ["expansion.glob.options"]),
    ("expand: globstar", 'cd /tmp; mkdir -p g/a/b; touch g/1 g/a/2 g/a/b/3; shopt -s globstar; echo g/**; echo g/**/3', ["expansion.glob.options"]),
    ("expand: glob character classes", 'cd /tmp; touch A1 b2 _3; echo [[:upper:]]*; echo [[:digit:]]* ; echo [[:alpha:]][[:digit:]]'),
    ("expand: GLOBIGNORE", 'cd /tmp; touch keep drop.o; GLOBIGNORE="*.o"; echo *'),
    ("expand: glob after splitting", 'cd /tmp; touch p1 p2; v="p* q"; echo $v'),
    # Quoting
    ("expand: dollar quotes", "echo $'tab\\there' $'\\x41\\u00e9' $'it\\'s' $'\\101' $'a\\cA' | od -c | head -n 2", ["syntax.quote.ansi-c"]),
    ("expand: locale quotes", 'echo $"translated" "$x"text'),
    ("expand: nested quotes in substitution", 'echo "$(echo "inner \\"quoted\\"")"; echo "a${u:-"b c"}d"'),
    ("expand: backslash in double quotes", 'echo "\\$HOME \\` \\" \\\\ \\a \\n"'),
    ("expand: word joining", 'a=x; echo "$a"y\'$a\'"$a"; echo ${a}_suffix $a_suffix'),
    (
        "expand: brace sequences at the ends of 64-bit integers",
        "echo {-9223372036854775807..-9223372036854775808}; echo {9223372036854775806..9223372036854775807}; "
        "echo {9223372036854775805..-9223372036854775805}; echo {1..3..99999999999999999999}; "
        "echo {a..e..99999999999999999999}; echo {1..3..9223372036854775807}; echo {1..10..0}",
        ["expansion.brace"],
    ),
]

EXTGLOB_PARSE = 'extended glob syntax always parses here, since the whole script is parsed before it runs; bash parses each line before running it, so enabling extglob on the same line is a syntax error there'
EXPECTED = {
    'expand: extglob enabled on the same line': (
        0, b'@(a|b)\n', b'', EXTGLOB_PARSE,
    ),
    'expand: extglob in conditions and case': (
        0, b'plus\ncompressed\nb\n', b'', EXTGLOB_PARSE,
    ),
    'expand: extglob patterns': (
        0, b'a.txt b.log c.txt\nabc b.log\nabc\nabc\n', b'', EXTGLOB_PARSE,
    ),
}
