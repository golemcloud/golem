"""Brush's parser, tokenizer and expansion syntax, and how jq parses its programs and input.

Each case is recorded from Bash 5.
"""

CASES = [
    # expansions after a here-document operator on the same line.
    ("parse: braced redirect target after a here-document",
     'f=/tmp/out.txt; cat <<EOF > "${f}"\nhello\nEOF\ncat /tmp/out.txt'),
    ("parse: append to a braced target after a here-document",
     'f=/tmp/out.txt; echo first > $f; cat <<EOF >> "${f}"\nsecond\nEOF\ncat "$f"'),
    ("parse: expansions after a here-document on the same line",
     'v=abc; cat <<EOF; echo ${v#a} $((1+2)) "${v%c}" $(echo x) $[2*3]\nbody\nEOF\necho end'),
    ("parse: two here-documents with expansions between them",
     'a=1; cat <<A "${b:-/dev/null}" - <<B; echo "${a}"\nfirst\nA\nsecond\nB'),
    ("parse: multi-line substitution on the here-document line",
     'cat <<EOF; x=$(\necho hi\n); echo "x=$x"\nbody\nEOF\necho end'),
    ("parse: here-document inside a substitution on a here-document line",
     'cat <<EOF; y=$(cat <<IN\ninner\nIN\n); echo "y=$y"\nouter\nEOF'),
    ("parse: here-document tag spelled with a parameter",
     'cat <<${x}\nbody\n${x}\necho end'),
    # bytes that are not UTF-8 are kept, not replaced or re-encoded, and never abort.
    ("parse: substitution of Latin-1 text",
     "printf 'caf\\xe9\\n' > /tmp/l.txt\ncontent=$(cat /tmp/l.txt)\necho \"len=${#content}\"\n"
     "printf %s \"$content\" | od -An -tx1\necho \"$content\" | od -An -tx1\necho after"),
    ("parse: substitution of a lone byte",
     "x=$(printf 'a\\377b'); echo \"len=${#x}\"; echo \"$x\" | od -An -tx1; echo after"),
    ("parse: backquotes and a file substitution",
     "printf 'x\\376\\n' > /tmp/b; a=`cat /tmp/b`; b=$(</tmp/b); printf '%s|' \"$a\" \"$b\" | od -An -tx1"),
    ("parse: ANSI-C hex and octal bytes",
     "x=$'a\\xffb'; printf %s \"$x\" | od -An -tx1; echo \"$x\" | od -An -tx1; y=$'\\351t\\351'; echo \"${#y}\"; echo \"$y\" | od -An -tx1"),
    ("parse: echo -e bytes",
     "echo -e '\\xff' | od -An -tx1; echo -e 'a\\0351b\\c' | od -An -tx1; x=$'\\xfe'; echo -e \"[$x\\x41]\" | od -An -tx1"),
    ("parse: @E transform bytes",
     "x='\\xff\\x41'; y=${x@E}; printf %s \"$y\" | od -An -tx1; echo ${#y}"),
    ("parse: read keeps an invalid byte",
     "printf 'a\\377b\\n' | { IFS= read -r l; printf %s \"$l\" | od -An -tx1; echo ${#l}; }"),
    ("parse: read keeps control characters",
     "printf 'a\\001b\\033c\\177d\\n' | { IFS= read -r l; printf %s \"$l\" | od -An -tx1; }\n"
     "printf 'x\\004y\\003z\\n' | { IFS= read -r l; printf %s \"$l\" | od -An -tx1; }\n"
     "printf 'a\\0b\\n' | { read -r l; echo \"$l\"; }"),
    ("parse: while-read copy of Latin-1 text",
     "printf 'caf\\xe9\\n' > /tmp/l.txt\nwhile IFS= read -r line; do echo \"$line\"; done < /tmp/l.txt > /tmp/copy.txt\nod -An -tx1 /tmp/copy.txt"),
    ("parse: mapfile keeps bytes and control characters",
     "printf 'a\\377\\n\\376b\\n' | { mapfile -t arr; printf '%s|' \"${arr[@]}\" | od -An -tx1; }\n"
     "printf 'x\\003\\n\\004y\\n' | { mapfile -t a; echo ${#a[@]}; printf '%s|' \"${a[@]}\" | od -An -tx1; }"),
    ("parse: here-document and here-string bytes",
     "x=$'\\xfe'; cat <<EOF | od -An -tx1\n$x\nEOF\nod -An -tx1 <<< \"$x\""),
    ("parse: byte arguments to tr, cut and printf",
     "printf 'a\\377b\\n' | tr $'\\xff' X; printf 'a\\377b\\n' | cut -d $'\\xff' -f2; printf '%s\\n' $'\\xff' | od -An -tx1"),
    ("parse: printf -v keeps bytes",
     "printf -v v '\\xff'; printf %s \"$v\" | od -An -tx1; echo ${#v}"),
    ("parse: patterns over bytes",
     "x=$'a\\xffb'; echo \"${x#a}\" | od -An -tx1; [[ $x == a?b ]] && echo match; echo \"${x/$'\\xff'/-}\""),
    ("parse: valid UTF-8 is unchanged",
     "x=$(printf 'caf\\xc3\\xa9'); echo \"${#x} $x\"; y=$'\\xc3\\xa9'; echo \"${#y} $y\""),
    ("parse: null bytes in a substitution",
     "x=$(printf 'a\\0b'); echo \"$x\""),
    # declare -f prints here-documents (and a word's own lines) as bash does, so the
    # README's persistence recipe reads the function back.
    ("parse: declare -f here-document round trip",
     "cd /tmp\nbuild() {\n  cat <<EOF\nconfig for $1\nEOF\n  echo built\n}\ndeclare -f build >> settings.sh\n"
     "#--call-- /tmp\n. ./settings.sh; build prod; echo \"status=$?\"; declare -f build"),
    ("parse: declare -f here-document shapes",
     "f() {\ncat <<EOF | grep x\nx1\nEOF\ncat <<EOF && echo ok\nb2\nEOF\ncat <<EOF &\nb3\nEOF\n"
     "while read l; do echo $l; done <<EOF\nb4\nEOF\ncat <<A <<-B >/dev/null\na5\nA\n\tb5\n\tB\n"
     "cat <<EOF\nlast\nEOF\n}\ng() { cat <<\"Q\"\nquoted $x\nQ\n}\nh() { cat <<E\\OF 2>&1 >&2 1>&2\ne\nEOF\n"
     "cat 0<<X; }\nX\ndeclare -f f g h; g; h 2>&1"),
    ("parse: declare -f here-documents in conditions and substitutions",
     "f() {\nif cat <<A; then echo t; fi\na\nA\nwhile read -r l <<B; do echo \"$l\"; break; done\nb\nB\n"
     "x=$(cat <<C\nc\nC\n)\necho \"multi\nline\"\ncase x in x) cat <<D;;\nd\nD\nesac\n}\ndeclare -f f; f"),
    ("parse: declare -f after a here-document",
     "f() {\ncat <<A\na\nA\nif true; then :; fi\ncat <<B\nb\nB\nwhile false; do :; done\ncat <<C\nc\nC\n"
     "echo x; if true; then :; fi\n}\ndeclare -f f\nh() { cat <<A > /tmp/x\na\nA\necho x > /dev/null; echo y; }\n"
     "declare -f h; k() { sleep 0 & echo a; sleep 0 & }; declare -f k; wait"),
    ("parse: declare -f redirections and substitutions",
     "f() { diff <(echo a) <(echo a; echo b) >/dev/null; cat < <(echo x) 2>&1 >&2; x=$(( 1 + 2 )); (( x++ )); "
     "echo a |& cat; { echo; } > /dev/null 2>&1; }; declare -f f; f\n"
     "g() { cat 0</dev/null 1>/dev/null 2>/dev/null 0<>/dev/null 1>>/dev/null 0<<<s; cat 3</dev/null 4<<<r; }; declare -f g"),
    ("parse: function with a multi-line string reads back",
     "f() { printf '%s\\n' \"a\n  b\" 'c\n\td'; }\ndeclare -f f > /tmp/f.sh; unset -f f; . /tmp/f.sh; f; type f"),
    ("parse: declare -f other bodies, elif and nested functions",
     "f() ( echo sub ) > /dev/null\ng() { echo g; } 2>/dev/null\n"
     "h() if true; then echo t; elif false; then echo e; elif :; then echo x; fi\n"
     "k() { function inner { echo i; }; inner2() ( echo j ); }\nm() for a in \"$@\"; do echo \"<$a>\"; done\n"
     "declare -f f g h k m; type k; declare -f h > /tmp/h.sh; unset -f h; . /tmp/h.sh; h"),
    # arithmetic text is expanded as if double-quoted (single quotes stay and fail),
    # $[ ] nests brackets, and NAME[subscript]= is one assignment word.
    ("parse: quotes inside arithmetic",
     "a=7; ( echo $(( 'a' + 1 )) ) 2>/dev/null; echo \"s=$?\"; ( echo $(( 1 \\+ 2 )) ) 2>/dev/null; echo \"s=$?\"\n"
     "( echo $(( $'1' + 1 )) ) 2>/dev/null; echo \"s=$?\"; bash -c \"(( 'a' == 7 ))\" 2>/dev/null; echo \"s=$?\"\n"
     "echo $(( \"1\"+\"2\" )) $(( \"a\" + 1 )); x=5; echo $(( \"$x\" * 2 )) $(( \"$(echo 3)\" + 1 )) $(( ${x:-\"9\"} ))\n"
     "[[ '1' -eq 1 ]] && echo cond-quotes"),
    ("parse: legacy arithmetic nests brackets",
     "a=(10 20 30); x=5; echo $[a[0] < 9] $[++a[0]] $[+(a[x-4])] $[+(~(a[1]))] $[z>=a[x-4] / 2]; echo \"${a[*]}\""),
    ("parse: array element assignments with blanks and operators",
     "cd /tmp; x=3; b[x>2]=y; declare -p b; a[1 + 1]=z; declare -p a; c=1 d[c+1 > 1]=w; declare -p d\n"
     "if true; then e[1 + 1]=v; fi; declare -p e; f[1]+=x; f[a[1]>0]=q; declare -p f; echo g[1 + 1]=u; ls /tmp"),
    ("parse: increment signs, empty hex and # in arithmetic",
     "c=0; echo $(( ++(c) )) $(( --(c) )) $(( - -c )) $(( 0x )) $(( 0X )) c=$c; bash -c '(( 1 # 2 ))' 2>/dev/null; echo \"s=$?\"; echo $(( 2#101 ))"),
    # a malformed ${...} is a bad substitution; ${!}, its operator forms and the
    # case toggles expand.
    ("parse: bad substitution ends the script",
     "v=abcd; echo before; echo \"${v:}\"; echo after"),
    ("parse: bad substitution in a redirection target",
     "cd /tmp; echo data > \"${dest }\"; echo \"status=$?\"; ls /tmp"),
    ("parse: bad substitutions in subshells",
     "v=abcd; a=(x); exec 2>&1; ( echo ${} ); echo \"s=$?\"; ( echo ${1a} ); echo \"s=$?\"; ( echo \"${v w}\" ); echo \"s=$?\"; "
     "( echo ${#v:-x} ); echo \"s=$?\"; ( echo ${#v:1} ); echo \"s=$?\"; ( echo ${[0]} ); echo \"s=$?\"; "
     "( echo ${${v}} ); echo \"s=$?\"; ( echo ${#!v} ); echo \"s=$?\"; ( echo ${x$} ); echo \"s=$?\""),
    ("parse: bad substitution with an unclosed subscript",
     "a=(x); ( echo \"${a[}\" ); echo \"status=$?\""),
    ("parse: bad substitution in a here-document or here-string fails the command",
     "cat <<< \"${v:}\"; echo \"after $?\"; cat <<EOF\nx ${v w} y\nEOF\necho \"after $?\""),
    ("parse: bad substitution diagnostics quote the enclosing text",
     "( echo \"a${v w}b\" ); ( echo a${v w}b ); ( x=\"a${v w}b\" ); ( echo \"${x:-a${v w}}\" ); ( echo a\"${v w}\"b ); echo \"s=$?\""),
    ("parse: bad transformation ends bash -c with 127",
     "v=x; ( echo \"${v@Z}\" ); echo \"status=$?\"; echo \"${v@QQ}\"; echo after"),
    ("parse: the special parameter ! in braces",
     "echo \"[${!}] [${!:-d}] [${!:+alt}] [${#!}]\"; sleep 0 & p=$!\n"
     "[ \"${!}\" = \"$p\" ] && echo same; [ \"${#!}\" -gt 0 ] && echo \"${!:+set}\" \"${!-unset}\" | tr -d 0-9; wait\n"
     "set -- a b c; echo \"${!#}\"; set --; echo \"[${!@}] [${!*}]\""),
    ("parse: invalid indirect names",
     "x='a b'; ( echo ${!x} ); echo \"status=$?\"; x=; ( echo \"[${!x}]\" ); echo \"status=$?\"; set -- p q; echo \"${!*}\"; echo after"),
    ("parse: case toggles",
     "y=AbC; echo \"${y~}\" \"${y~~}\" \"${y~[a-c]}\" \"${y~~[AB]}\"; a=(aB Cd); echo \"${a[@]~}\" \"${a[@]~~}\"; z=éÀ; echo \"${z~~}\"; declare -A m=([k]=xY); echo \"${m[k]~~}\""),
    # the shortest prefix or suffix a pattern matches can be empty.
    ("parse: shortest match can be empty",
     "x=abc; echo \"${x#*}\" \"${x%*}\" \"${x##*}\" \"${x%%*}\" \"${x#?(a)}\" \"${x%@(c|)}\" \"${x#*b}\" \"${x%b*}\"; p='*'; echo \"${x#$p}\" \"${x%$p}\"\n"
     "a=(abc def); echo \"${a[@]#*}\" \"${a[@]%*}\"; set -- one two; echo \"${@#*}\" \"${*%*}\"; y=; echo \"[${y#*}]\""),
    # a backslash in backquotes escapes $, ` and \ (and " inside double quotes).
    ("parse: backquote unescaping",
     "v=V; HOME=/home/x; echo `echo \\$HOME` `echo \\$v`; echo \"`echo \\$v`\"; echo `echo \\\\$v`; echo `echo a\\\\\\\\b`\n"
     "echo \"`echo \\\"hi\\\"`\"; echo `echo \\\"hi\\\"`; echo `echo 'a\\$b'`; echo `echo \\`echo nested\\``; echo \"`echo a\\\\b`\"; echo `echo x\\y`"),
    ("parse: a backslash that ends the input or a backquote",
     "echo `echo \\\\`; echo a \\"),
    # expansion details.
    ("parse: key-value and attribute transforms of arrays",
     "p() { printf \"%s\" \"$#\"; printf \" <%s>\" \"$@\"; echo; }\n"
     "a=(one \"two three\" \"\"); declare -A m=([k]=v)\n"
     "p \"${a[@]@K}\"; p \"${a[*]@K}\"; p \"${m[@]@K}\"; p \"${a[@]@k}\"; p \"${a[*]@k}\"; p \"${m[@]@k}\"\n"
     "p \"${a[@]@A}\"; p \"${a[*]@A}\"; p \"${a[1]@A}\" \"${a[1]@K}\" \"${a[1]@a}\"; p \"${a[@]@a}\" \"${a[*]@a}\"\n"
     "declare -ri r=5; p \"${r@A}\"; declare -x ex=1; p \"${ex@A}\" \"${ex@a}\"; t=target; declare -n v=t; p \"${v@A}\" \"${v@a}\"\n"
     "set -- one \"two three\"; p \"${@@A}\"; p \"${*@A}\"; p \"${@@K}\"; p \"${@@a}\" \"${*@a}\"; set --; p \"${@@A}\" \"${*@A}\"\n"
     "e=(); p \"${e[@]@A}\" \"${e[@]@K}\" \"${e[@]@a}\"; unset u; p \"${u@A}\" \"${u[@]@A}\"; declare -a d; p \"${d@A}\" \"${d[@]@A}\"\n"
     "w='a  b'; x=('x y' z); p ${w@K} ${x[@]@K}; p ${w@k} ${x[@]@k}; p ${w@a} ${x[@]@a}"),
    ("parse: lengths of special parameters",
     "false; echo \"${#?}\"; set -- a b c; echo \"${##}\" \"${#@}\" \"${#*}\"; echo \"${#-}\" | grep -c .; true; echo \"${#?}\""),
    ("parse: prefix names skip declared but unset variables",
     "p() { printf \"%s\" \"$#\"; printf \" <%s>\" \"$@\"; echo; }\n"
     "declare pre_x; pre_y=1; pre_z=; echo ${!pre*}; p \"${!pre@}\"; IFS=-; p \"${!pre*}\""),
    ("parse: alternative and default of empty arrays",
     "p() { printf \"%s\" \"$#\"; printf \" <%s>\" \"$@\"; echo; }\n"
     "a=(); p \"${a[@]:+W}\" \"${a[@]+W}\" \"${a[*]:+W}\" \"${a[@]:-D}\" \"${a[@]-D}\"\n"
     "a=(\"\"); p \"${a[@]:+W}\" \"${a[@]+W}\" \"${a[*]:+W}\" \"${a[@]:-D}\"\n"
     "a=(\"\" \"\"); p \"${a[@]:+W}\" \"${a[*]:+W}\" \"${a[@]:-D}\"\n"
     "set --; p \"${@:+W}\" \"${@+W}\" \"${*:+W}\" \"${@:-D}\" \"${@-D}\"; set -- \"\"; p \"${@:+W}\" \"${*:+W}\"; set -- \"\" \"\"; p \"${@:+W}\"\n"
     "unset u; p \"${u[@]:+W}\" \"${u[@]-D}\" \"${u:+W}\""),
    ("parse: ANSI-C quotes in a double-quoted default",
     "unset u; x=\"${u:-$'a\\tb'}\"; echo \"$x\"; echo \"${u:-a$'\\x41'b}\" \"${u:-$'x  y'}\"; y=\"${u:+$'z'}\"; echo \"[$y]\""),
    ("parse: IFS with a multibyte character",
     "p() { printf \"%s\" \"$#\"; printf \" <%s>\" \"$@\"; echo; }\n"
     "IFS=\" é\"; for x in \"a é b\" \"aéb\" \"aébéc\" \" éa\" \"a éb\" \"a é é b\"; do p $x; done; IFS=\" €\"; x=\"a € b\"; p $x"),
    ("parse: read keeps escaped IFS characters",
     "IFS=: read x y <<< 'a\\:b:c'; echo \"$x|$y\"; read x y <<< 'a\\ b c\\ '; echo \"[$x|$y]\"; read -r x y <<< 'a\\ b c'; echo \"[$x|$y]\"\n"
     "read <<< ' a\\:b\\\\c '; echo \"[$REPLY]\"; IFS=: read -a arr <<< 'x\\:y:z'; echo \"${#arr[@]} ${arr[0]}\"; read x <<< 'a\\'; echo \"[$x]\""),
    # GLOBIGNORE patterns match the results as returned.
    ("parse: GLOBIGNORE with relative patterns",
     "cd /tmp && touch a.txt b.txt c .hid && mkdir -p d && touch d/x d/y\n"
     "GLOBIGNORE='a*'; echo *; GLOBIGNORE='[ab]*'; echo *; GLOBIGNORE='?'; echo *; GLOBIGNORE='a*:b*'; echo *\n"
     "GLOBIGNORE='d/x'; echo d/*; GLOBIGNORE='.*'; echo *; GLOBIGNORE='/tmp/b*'; echo /tmp/*.txt; unset GLOBIGNORE; echo *"),
    # globstar lists no empty word, sorts all results, and */** names directories bare.
    ("parse: globstar results",
     "cd /tmp; mkdir -p d/e x a a-b; : > d/f; : > d/e/h; : > g; : > x/a.txt; : > b.txt; : > a/z; : > a-b/y; : > z.txt\n"
     "shopt -s globstar; echo **; echo */**; echo d/**; echo **/; echo **/*.txt; echo d/**/h; echo x/**/*.txt\n"
     "for f in **; do echo \"[$f]\"; done | head -2; echo ./**; echo */*; cd /; echo /tmp/*/**; echo /tmp/**/*.txt; echo /tmp/d/**"),
    # patsub_replacement: an unquoted & in a replacement is the matched text.
    ("parse: ampersand in a pattern replacement",
     "x=abcb; r=\"&\"; q=\"<&>\"; d='$0'\n"
     "echo \"${x/b/[&]}\" ${x/b/[&]} \"${x//b/[&]}\" \"${x/b/\\&}\" \"${x/b/\"&\"}\" \"${x/b/'&'}\"\n"
     "echo \"${x/b/$r}\" ${x/b/$r} \"${x/b/\"$r\"}\" \"${x/b/$q}\" \"${x/b/&&}\" \"${x/b/\\\\&}\"\n"
     "echo \"${x/#a/[&]}\" \"${x/%b/[&]}\" \"${x/b*/<&>}\" \"${x/b/$d}\" \"${x/b/\\$1}\"\n"
     "shopt -u patsub_replacement; echo \"${x/b/[&]}\"; shopt -s patsub_replacement\n"
     "p=\"b\"; echo \"${x/$p/(&)}\"; y=a.b; echo \"${y/./[&]}\"; a=(xb yb); echo \"${a[@]/b/<&>}\""),
    # case conversion maps one character to one, as bash's towupper/towlower do.
    ("parse: per-character case conversion",
     "x=\"straße ﬁx ŉ İi ǅ ẞ ΣΑΣ σς Ω ǈ ı\"; echo \"${x^^}\"; echo \"${x,,}\"; echo \"${x~~}\"; echo \"${x^}\"\n"
     "declare -u u=\"$x\"; echo \"$u\"; declare -l l=\"$x\"; echo \"$l\"; echo \"${x@U}\"; echo \"${x@L}\"\n"
     "y=\"ßa\"; echo \"${y@u}\" \"${y^}\" \"${y~}\" \"${#y}\" \"${y^^}\"; z=\"ﬁa\"; echo \"${z^}\"; declare -c c=ßOB; echo \"$c\"; declare -c d=éTÉ; echo \"$d\""),
    # an unquoted here-document joins backslash-newline, and its parameter words keep
    # single quotes.
    ("parse: here-document line continuation",
     "cat <<EOF > /tmp/Dockerfile\nRUN apt-get update && \\\n    apt-get install -y curl\nEOF\ncat /tmp/Dockerfile\n"
     "cat <<EOF\na \\\nb\nc\\\\\nd \\\\\\\ne\nEOF\ncat <<'EOF'\nq \\\nr\nEOF\ncat <<-EOF\n\tt \\\n\tu\n\tEOF"),
    ("parse: here-document parameter words keep single quotes",
     "v=1; u=; cat <<EOF\n[${v:+'x'}] [${v:+\"y\"}] [${u:-'z w'}] [${u:-\"q r\"}] [${v:+\\$}] [${v/1/'o'}] [\"$v\"] ['$v']\nEOF"),
    # !(*) never matches the empty string, escaped characters work in pattern lists, and
    # failglob ends the shell from a function or a redirection.
    ("parse: negated extglob and the empty string",
     "shopt -s extglob; for s in a b ab '' x; do [[ $s == !(*) ]] && printf '[%s]' \"$s\"; [[ $s == !(a) ]] && printf '<%s>' \"$s\"; done; echo\n"
     "[[ '' == !(a|b) ]] && echo e1; [[ '' == !(*(x)) ]] || echo e2; [[ ab == !(a)b ]] && echo e3; [[ b == !(a)b ]] && echo e4"),
    ("parse: escaped characters in pattern lists",
     "shopt -s extglob\ncd /tmp; : > b; : > 'a*'; : > c; echo @(\\*|b); echo @(a\\*|c); echo +(\\*|c); [[ '*' == @(\\*|b) ]] && echo m1; [[ a == @(\\*|b) ]] || echo m2"),
    ("parse: failglob in a function ends the shell",
     "shopt -s failglob; f() { echo nomatch*; echo in-f; }; f; echo \"status=$?\""),
    ("parse: failglob in a redirection ends the shell",
     "cd /tmp; shopt -s failglob; echo x > nomatch*; echo \"status=$?\""),
    ("parse: failglob abandons the top-level command, line by line",
     "cd /tmp; shopt -s failglob\necho nomatch*; echo same-line\necho \"after $?\"\nf() { echo nomatch*; echo in-f; }; f; echo same-line\n"
     "echo \"after2 $?\"\necho x > nomatch*; echo same-line\necho \"after3 $?\"\nfor x in nomatch*; do echo $x; done; echo same-line\necho \"after4 $?\""),
    ("parse: failglob in a subshell ends only the subshell",
     "cd /tmp; shopt -s failglob; ( f() { echo nomatch*; echo in; }; f ); echo \"status=$?\"; x=$(echo nomatch*); echo \"s2=$?\""),
    # ANSI-C escapes beyond a byte or a character.
    ("parse: ANSI-C octal above 377 and code points that are not characters",
     "for x in $'\\ud800' $'\\udfff' $'\\U00110000' $'\\U001fffff' $'\\U00200000' $'\\U7fffffff' $'\\UFFFFFFFF' $'\\777' $'\\400' $'a\\u0b'; do\n"
     "  printf %s \"$x\" | od -An -tx1; echo \"len=${#x}\"\ndone; echo -e '\\0777\\0400' | od -An -tx1; x=$'\\777'; echo ${#x}"),
    # valid scripts that were refused or misparsed.
    ("parse: case with pat) inside a command substitution",
     "os=$(case linux in linux) echo penguin;; *) echo other;; esac); echo \"$os\"; echo $(case a in a) echo m;; esac)\n"
     "x=\"$(case b in (b) echo pb;; esac)\"; echo \"$x\"; y=$(echo 1; case q in q) case r in r) echo nested;; esac;; esac; echo 2); echo $y"),
    ("parse: nested subshells with a blank between the parentheses",
     "( (echo a) ); ( ( echo b ) ); x=$( ( echo c ) ); echo \"$x\"; ( ( cd /tmp && pwd ) ); x=$( ( ( echo deep ) ) ); echo \"[$x]\"; echo end"),
    ("parse: letter ranges across punctuation",
     "p() { printf '%s' \"$#\"; printf ' <%s>' \"$@\"; echo; }; p {Z..a}; p {A..c}; p {a..Z}; echo {Y..b..2}"),
    ("parse: for with an invalid name or a brace body",
     "for 1x in a; do echo \"[$1x]\"; done; echo \"status=$?\"\nfor a in 1 2; { echo b$a; }; set -- p q; for b; { echo c$b; }\n"
     "for d in e\n{ echo $d; }; for ((i=0;i<2;i++)); { echo i$i; }; for ((j=0;j<1;j++)) { echo j$j; }; echo end"),
    ("parse: a for brace body needs its separator",
     "echo before; set -- p; for b { echo c$b; }; echo end"),
    ("parse: bash 5.3 command substitutions in the current shell",
     "y=1; x=${ y=2; echo z; }; echo \"$x $y\"; x=${ echo hi; echo there\n}; echo \"[$x]\"; x=${| REPLY=val; }; echo \"[$x] [${REPLY-unset}]\"\n"
     "REPLY=keep; x=${| REPLY=v2; echo out; }; echo \"[$x] [$REPLY]\"; x=${ false; }; echo \"s=$?\"; cd /tmp; x=${ cd /; pwd; }; echo \"$x $PWD\"\n"
     "echo \"a${ echo b; }c\"; x=${ { echo g; }; }; echo $x; f() { local l=1; x=${ l=2; echo $l; }; echo \"$x $l\"; }; f\n"
     "x=${ echo \"}\"; }; echo \"$x\"; x=${ echo {a,b}; }; echo \"$x\"; x=${ case q in q) echo cq;; esac; }; echo $x; x=${ printf 'a\\n\\n'; }; echo \"[$x]\""),
    ("parse: process substitution in an assignment",
     "echo first; x=<(echo hi); echo \"$x\"; a=(<(true) <(true)); echo ${#a[@]} \"${a[@]}\"\n"
     "a=(<(echo 1) x<(echo 2)); echo \"${a[@]}\"; declare d=<(echo q); echo $d; f() { local l=<(echo r); echo $l; }; f; export e=<(echo s); echo $e"),
    ("parse: process substitution inside a word",
     "echo b<(echo q)c; g() { echo \"${1#--c=}\"; cat \"${1#--c=}\"; }; g --c=<(echo inner); for i in a<(echo z); do echo $i; done\n"
     "[[ x<(true) == x/dev/fd/* ]] && echo yes; [[ a<b ]] && echo less; (( 1<(2) )) && echo lt; echo $(( 3>(2) )) \"x<(y)\" x\\<\\(y\\)\n"
     "h() { cat \"${1#in=}\" \"${2#in=}\"; }; h in=<(echo one) in=<(echo two)"),
    # here-documents the script ends in, and a backquote left open in a body.
    ("parse: a here-document tag with a comma and a blank",
     'cat <<"A, B"\nhello\n'),
    ("parse: line numbers of here-documents ended by the end of the script",
     "cat <<'Q, R' <<\"S\"\nq\nQ, R\ns\necho t"),
    ("parse: line numbers of several open here-documents",
     'cat <<EOF1 <<EOF2\nfoo\n'),
    ("parse: line numbers after a closed here-document on the same line",
     'cat <<A <<B <<C\na\nA\nb'),
    ("parse: the warning comes before the command holding the here-document",
     'echo e1 >&2\necho e2 >&2; cat <<EOF; echo e3 >&2\nbody'),
    ("parse: no warning when the script exits before the here-document",
     'echo x; exit 3\ncat <<EOF\nbody'),
    ("parse: a script that ends on a here-document's tag",
     'echo a; cat <<A'),
    ("parse: a syntax error after a here-document the script ends in",
     'f() {\ncat <<A\nx\n}\nf'),
    ("parse: a backquote left open in a here-document",
     'echo before\ncat <<EOF > /tmp/notes.md\nDon`t forget\nEOF\necho "after status=$?"\n'
     'x=1\n\ncat <<EOF; echo same\na $x `echo hi` b`c\nd\nEOF\necho s=$?; cat <<\'EOF\'\nquoted ` ok\nEOF'),
    # syntax errors named as bash names them, with or without a final newline.
    ("parse: a misplaced fi on a last line without a newline",
     "echo first; fi"),
    ("parse: a misplaced done after a missing separator",
     "while true do echo x; done"),
    ("parse: an extra done at the end",
     "for f in a b; do echo $f; done; done"),
    ("parse: an open parenthesis at the end",
     "echo ("),
    ("parse: a bang after a pipe",
     "echo a | ! true"),
    ("parse: a conditional missing its binary operator",
     "[[ a b ]]"),
    ("parse: a conditional word before a newline",
     "[[ a\n&& b ]]"),
    ("parse: a conditional group left open at a newline",
     "[[ ( a\n) ]]"),
    ("parse: a conditional unary operator without its argument",
     "[[ -f ]]"),
    ("parse: a conditional binary operator without its argument",
     "[[ a == ]]"),
    ("parse: a conditional group without its closing parenthesis",
     "[[ ( a == b ]]"),
    ("parse: a conditional and-list missing a term",
     "[[ a && && b ]]"),
    ("parse: a conditional word after a group",
     "[[ ( a ) b ]]"),
    ("parse: a conditional left open at the end",
     "[[ a == b"),
    ("parse: an empty conditional at the end",
     "[["),
    ("parse: a conditional word that is not an operator",
     "[[ -% 3 ]]"),
    ("parse: a conditional left open inside if",
     "if true; then\n[[ a =="),
    # Arithmetic text as written: (( )) keeps its blanks, and a comment in $(( )) hides its end.
    ("parse: arithmetic commands keep their text",
     "f() { ((  x  +=  1  )); ((x)); ((\ty\t)); ((  z\n  +1  )); for ((  i = 0 ;  i < 1 ;  i++  )); do :; done; "
     "for ((;;)); do break; done; }; declare -f f; f; echo \"$x $z $i\""),
    ("parse: a comment hides the end of an arithmetic expansion",
     "echo $(( 2#101 )) $((2#11+1)); echo a$((1 # c))b; echo not reached"),
    ("parse: a comment hides the end of a quoted arithmetic expansion",
     'x="x$((1 #c))y"; echo "not reached"'),
    ("parse: an escaped blank before a comment in an arithmetic expansion",
     'echo $((1\\ #c)); echo "not reached"'),
    ("parse: a regular expression keeps its blanks",
     '[[ "a  b" =~ (a  b) ]] && echo two || echo no; [[ "a b" =~ (a  b) ]] && echo one || echo no2; '
     '[[ "x\ty" =~ (x\ty) ]] && echo tab || echo notab'),
    ("parse: an invalid indirect name abandons the command, not the script",
     "x=1a; echo ${!x}; echo after\necho line2\nf() { x=1a; echo ${!x}; echo in; }; f; echo s=$?\n"
     "echo next $?\ny=${!x} echo run; echo after\necho last"),
    # Nesting deeper than the WASM stack holds is refused before the script runs, not a trap.
    ("parse: 1000 nested parameter expansions",
     "a=x; echo " + "${a:-" * 1000 + "y" + "}" * 1000 + "; echo after"),
    ("parse: 3000 nested quoted parameter expansions",
     "a=x; echo " + '"${a:-' * 3000 + "y" + '}"' * 3000 + "; echo after"),
    ("parse: 1000 nested parameter expansions in a here-document",
     "a=x; cat <<EOF\n" + "${a:-" * 1000 + "y" + "}" * 1000 + "\nEOF\necho after"),
    ("parse: deeply nested text in single quotes is only text",
     "echo '" + "${a:-" * 1000 + "y" + "}" * 1000 + "' | wc -c"),
    ("parse: deeply nested text given to eval is refused where eval runs",
     "a=x; eval 'echo " + '"${a:-' * 3000 + "y" + '}"' * 3000 + "'; echo \"after $?\""),
    # Here-documents inside a command substitution, whose bodies hold parentheses.
    ("parse: a here-document in a substitution with parentheses in its body",
     'x=$(cat <<EOF\n)\nEOF\n); echo "[$x]"; y=$(cat <<\'EOF\'\n(\nEOF\n); echo "[$y]"\n'
     'echo "$(cat <<EOF\na)b\nEOF\n)"; z=$(cat <<-EOF\n\t) (\n\tEOF\n); echo "[$z]"'),
    ("parse: a usage message in a substitution",
     'usage=$(cat <<EOF\nUsage: tool [options]\n  1) install\n  2) remove (the default\nEOF\n)\necho "$usage"; echo done'),
    ("parse: two here-documents on one line in a substitution",
     'x=$(cat <<A; cat <<B\n(a\nA\nb)\nB\n); echo "$x"; y=$(cat <<A | tr a-z A-Z\n)x(\nA\n); echo "$y"'),
    # A here-document body's command substitutions are parsed when it is expanded: one that is
    # left open or does not parse fails that command, as in bash.
    ("parse: a command substitution left open in a here-document",
     'cat <<EOF\n$(echo hi\nEOF\necho s=$?\necho 1\nf() {\n  cat <<EOF\nA\n$(echo a b\nc\nEOF\n}\nf; echo s=$?\n'
     'cat <<EOF; echo same\n$(echo x\nEOF\necho s=$?'),
    ("parse: a command substitution that does not parse in a here-document",
     'cat <<EOF\n$(if) more\nEOF\necho s=$?\ncat <<EOF\nok $(echo a; fi) x\nEOF\necho s=$?\n'
     'cat <<EOF\n$(case x in x) echo y\nEOF\necho s=$?'),
    ("parse: a quote left open in a here-document's command substitution",
     'cat <<EOF\n$(echo \'a\nEOF\necho s=$?\ncat <<EOF\n$(echo "a)" b\nEOF\necho s=$?\ncat <<EOF\n$(echo `a\nEOF\necho s=$?'),
    ("parse: expansions left open in a here-document",
     'x=5\ncat <<EOF\na ${x\nb\nEOF\necho s=$?\ncat <<EOF\na $((x+\nb\nEOF\necho s=$?\ncat <<EOF\na $[1+2\nb\nEOF\necho s=$?'),
    ("parse: command substitutions that parse in a here-document",
     'cat <<EOF\n$(echo ")") $(cat <<IN\n)\nIN\n) $(case a in a) echo c;; esac) $((1+2)) `echo b`\nEOF\necho s=$?'),
    # A syntax error inside a compound array assignment's parentheses exits 1, as in bash.
    ("parse: a compound assignment left open", "x=(a b"),
    ("parse: a compound assignment left open inside if", "if true; then\nx=(a\n"),
    ("parse: an operator inside a compound assignment", "x=(a | b)"),
    ("parse: a quote left open inside a compound assignment", 'declare -a x=(a "b'),
    ("parse: an extra parenthesis after a compound assignment", "x=(a b)); echo c"),
    ("parse: compound assignment errors in eval and source",
     "eval 'x=(a (b) c)'; echo s=$?; eval 'echo a; fi'; echo s=$?; "
     "printf 'x+=(a b; )' > /tmp/s.sh; source /tmp/s.sh; echo s=$?; eval 'x=(a b'; echo s=$?"),
    ("parse: a subscript nested too deeply at run time",
     'x="$(printf "(%.0s" {1..10000})1$(printf ")%.0s" {1..10000})"; a=(p q)\n'
     'unset "a[$x]"; echo s=$?; declare -p a\nr="a[$x]"; echo "${!r}"\necho s=$?\n'
     'printf -v "a[$x]" z; echo s=$?; declare -p a'),
    ("parse: eval syntax errors are numbered from the eval's line",
     "echo x\neval 'echo a; fi'\necho s=$?\nf() {\n  eval $'\\n[[ a b ]]'\n}\nf; echo s=$?\n"
     "eval 'x=(a b'; echo s=$?"),
    ("parse: an eval's unexpected end of file names its command's line from the eval's",
     "echo x\neval 'case x in'\necho s=$?\neval '\nif true\nthen echo a'\necho s=$?\nf() {\n"
     "  eval 'while :'\n}\nf; echo s=$?"),
    ("parse: an eval closes a here-document left open, as bash does",
     "echo x\neval 'cat <<E\na'\necho s=$?\neval '\ncat <<E; cat <<F\nb\nE'\necho s=$?"),
    # Bash parses a `$( )` with the command it is in: a syntax error inside names the line it is
    # on and the whole line, and ends the shell with 127 (a subshell with 1). The commands before
    # it print nothing: bash runs them, bash-tool checks the whole script first.
    ("parse: a syntax error in a command substitution names the line it is on",
     ":\necho $(fi)\necho s=$?\n"
     "#--call-- /\n:\nf() {\n  echo \"a $(echo b; fi) c\"\n}\nf\n"
     "#--call-- /\n:\ny=$(echo a\nfi)\n"
     "#--call-- /\n:\necho $(if)\n"
     "#--call-- /\necho ${x:-$(fi)}\n"
     "#--call-- /\n: x; echo $(echo $(fi))\n"),
    ("parse: a syntax error in an eval's command substitution ends the shell",
     "echo x\neval '\n\necho $(fi)'\necho s=$?\n"
     "#--call-- /\ntrap 'echo trapped $?' EXIT\nf() {\n  eval 'y=$(echo a; fi)'\n}\nf\necho s=$?\n"
     "#--call-- /\n(eval 'echo $(fi)'); echo s=$?\nx=$(eval 'echo $(fi)'); echo s=$?\n"
     "eval 'echo $(fi)' | cat; echo s=$?\n"
     "#--call-- /\nprintf '\\n\\necho $(fi)\\necho a\\n' > /tmp/s.sh\n. /tmp/s.sh\necho s=$?\n"),
    # Bash reads `${ }`, `$(( ))`, `$[ ]` and `(( ))` as matched pairs and names the line one left
    # open began on; a `$( )` left open, the line after the last.
    ("parse: an expansion left open names the line it began on",
     ":\necho ${a:-b\nc\n"
     "#--call-- /\n:\necho \"${a\n"
     "#--call-- /\n:\necho $((1+\n2\n"
     "#--call-- /\n:\necho $[1+\n"
     "#--call-- /\n:\n((1+\n2\n"
     "#--call-- /\n:\n((\n"
     "#--call-- /\n:\necho $(echo a\n"
     "#--call-- /\n:\neval '\necho ${a'; echo s=$?\neval '((1+'; echo s=$?\n"),
    # Bash reads backquotes only as they run.
    ("parse: a syntax error in backquotes is reported where it runs",
     "echo x\necho `fi`\necho s=$?\neval 'echo `fi`'\necho s=$?\nf() {\n  y=`echo a; fi`\n}\n"
     "f; echo s=$?"),
    # `NAME=(...)` is a compound assignment only as an argument of an assignment builtin.
    ("parse: a compound assignment argument of echo", "echo x=(a b)"),
    ("parse: compound assignment arguments only for assignment builtins",
     "eval 'true x=(a b)'; echo s=$?; eval 'builtin declare x=(a)'; echo s=$?; "
     "eval \"'declare' x=(a)\"; echo s=$?; eval 'for i in x=(a); do :; done'; echo s=$?\n"
     "declare x=(a b) y=(c); eval x=(d e); declare -p x y; f() { local l=(a b); declare -p l; }; f\n"
     "export e=(a); readonly r=(b); typeset t=(c); declare -p e r t; z=1 export w=(d); declare -p w"),
    ("parse: quoting of bytes",
     "x=$'a\\xffb\\x01'; declare -p x; echo \"${x@Q}\"; printf '%q\\n' \"$x\"; y=(1 \"$x\"); declare -p y; : ${z:?$x}"),
    ("parse: file names that are not UTF-8",
     "cd /tmp; x=$(printf 'a\\xe9'); echo hi > \"$x\"; echo \"s=$?\"; [ -e \"$x\" ]; echo \"e=$?\""),
    # jq's command line, compile gaps and input reader.
    ("parse: jq --args and --jsonargs take negative numbers",
     "jq -cn '$ARGS.positional' --jsonargs -5 3; echo s=$?; jq -cn '$ARGS.positional' --args -5 -.5 3; "
     "echo s=$?; jq -n '-0 | tostring'; jq -cn '$ARGS' --args a -- -b"),
    ("parse: jq --jsonargs and --argjson parse as jq does",
     "jq -cn '$ARGS.positional' --jsonargs 1 '{\"a\":2}' '-1e3' nan; echo s=$?; "
     "jq -cn '$ARGS.positional' --jsonargs x; echo s=$?; jq -cn '$ARGS.positional' --jsonargs '1 2'; "
     "echo s=$?; jq -nc --argjson x nan '$x'; jq -n --argjson x '1 2' '$x'; echo s=$?"),
    ("parse: jq -s prints nothing before a parse error",
     "printf '1 2 x\\n' | jq -s .; echo s=$?; printf 'x' | jq -c -s .; echo s=$?; "
     "printf '1 2 x\\n' | jq -c .; echo s=$?"),
    ("parse: jq parse errors are jq's",
     "for t in '{\"a\" 1}' '{\"a\":1,}' '{,}' '[,1]' ':1' '{\"a\"::1}' '{1:2}' '[1:2]' '{\"a\"}' "
     "'{\"a\":1 \"b\":2}' '[1 2]' 'tru' 'truex' \"'a'\" '\"a\\x\"' '\"\\u12\"' '\"\\u12g4\"' '\"\\ud800\"' "
     "'\"\\ud800\\u0041\"' '\"a' '[' ']' '}' '1 }' '{\"a\":[}' '\"x\\u0001\"' \"$(printf '\"a\\tb\"')\" "
     "'[1,,2]' '{\"a\":1,,}' '{\"a\":}' '1]' '[1}' 'nul' 'nan5'; do printf '%s => ' \"$t\"; "
     "printf '%s' \"$t\" | jq -c . 2>&1 | tr '\\n' '|'; echo; done"),
    ("parse: jq parse error positions",
     "printf '[1,\\n2,\\n}' | jq -c .; echo s=$?; printf '{\"a\":1' | jq -c .; echo s=$?; "
     "printf '1\\n2\\n\\n  x' | jq -c .; echo s=$?; printf '{\\n\"a\"\\n:\\n\\n1 2' | jq -c .; echo s=$?; "
     "printf '[1,\\n\\t2,]' | jq -c .; echo s=$?; printf '\"abc\\n' | jq -c .; echo s=$?"),
    ("parse: jq number literals jq reads",
     "printf '[Infinity, -inf, NaN, +1, .5, 5.]' | jq -c .; printf '[nan0, sNaN, -NaN, 00, -00, 1e+5, 1E-0]'"
     " | jq -c .; printf '[Infinity, 0x1]' | jq -c .; echo s=$?; printf 'nan5' | jq .; echo s=$?; "
     "echo '[-0, -0.0, 0.0, -0e0, 1.0, 1.50, 1e3, 1E-2]' | jq -c ."),
    ("parse: jq .5 and 1. literals and IN",
     "jq -n '[.1 + .2]' -c; jq -nc '[.5, 1., 1.e2, .5e1, 1e1000, 00012, 1.50]'; jq -n '2 | IN(1,2)'; "
     "jq -nc '[1,5] | IN(.[]; 5, 6)'; jq -n '.e5'"),
    ("parse: jq input_line_number",
     "printf '1\\n2\\n3\\n' | jq -c '[., input_line_number]'; printf '1 2 3' | jq -c '[., input_line_number]'; "
     "printf '[1,\\n2]\\n3\\n\\n4' | jq -c '[., input_line_number]'; echo x | jq -R -c '[., input_line_number]'; "
     "printf 'a\\nb' | jq -R -c '[., input_line_number]'; jq -n 'input_line_number'"),
    ("parse: jq input_line_number counts jq's 4091-byte chunks",
     "{ printf '1 '; i=0; while [ $i -lt 3000 ]; do printf '2 '; i=$((i+1)); done; echo; echo 3; } > /tmp/long; "
     "jq -c '[., input_line_number]' /tmp/long | uniq -c"),
    ("parse: jq tostream, fromstream and truncate_stream",
     "jq -c 'tostream' <<< '{\"a\":[1,{\"b\":2}],\"c\":[]}'; jq -c 'fromstream(tostream)' <<< '{\"a\":[1,{\"b\":2}],\"c\":[]}'; "
     "jq -nc '[1 | truncate_stream([[0],1],[[1,0],2],[[1,0]],[[1]])]'"),
    ("parse: jq destructuring alternatives",
     "jq -nc '[[1,2],{\"a\":3}] | .[] as [$a,$b] ?// {a:$a} | [$a,$b]'; "
     "jq -nc '[[3]] | .[] as [$a] ?// [$b] | if $a != null then error(\"err: \\($a)\") else {$a,$b} end'; "
     "jq -nc '1 | . as [$a] ?// {a:$a} | $a'; echo s=$?; jq -nc '[[1,2],[3]] | .[] as [$a,$b] ?// [$c] | [$a,$b,$c]'"),
    ("parse: jq surrogate pair escapes",
     "jq -n '\"\\ud83d\\ude00\"' | od -An -tx1; jq -n '\"a\\ude00b\"' | od -An -tx1; "
     "printf '\"\\\\ud83d\\\\ude00\" \"\\\\ude00x\" \"\\\\ud83d\"\\n' | jq -c . | od -An -tx1; echo s=$?"),
    ("parse: jq bad escapes in a program string",
     "jq -n '\"\\ud83d\"'; echo s=$?; jq -n '\"\\x\"'; echo s=$?; jq -n '\"\\u12\"'; echo s=$?; "
     "jq -n '\"\\u12g4\"'; echo s=$?; jq -n '\"a\\u00e9\\(1)\\t\"'; jq -n '\"\\\n x\"'; echo s=$?"),
    ("parse: jq byte order marks",
     "printf '\\xef\\xbb\\xbf{\"a\":1}\\n' | jq -c .; echo s=$?; printf '\\xef\\xbb\\xbf' | jq -c .; echo s=$?; "
     "printf '\\xef\\xbb\\xbf1 \\xef\\xbb\\xbf2' | jq -c .; echo s=$?; printf '\\xef\\xbb1' | jq -c .; echo s=$?; "
     "printf '\\xef\\xbb\\xbf\"x\"' | jq -R .; echo s=$?"),
    ("parse: jq invalid UTF-8 input becomes U+FFFD as jq decodes it",
     "printf '\"a\\377b\" \"\\xc3\" \"\\xe2\\x82\" \"\\xf0\\x9f\\x98\" \"\\xc3(\" \"\\xed\\xa0\\x80\" \"\\xc0\\xaf\" "
     "\"\\xf4\\x90\\x80\\x80\"' | jq -c . | od -An -tx1; printf 'x\\377y\\n' | jq -R . | od -An -tx1"),
    ("parse: jq unreadable operands",
     "echo 1 2 > /tmp/f; jq -c . /nonexistent /tmp/f; echo s=$?; jq -c . /tmp/f /nonexistent /tmp/f; echo s=$?; "
     "jq -c . /tmp /tmp/f; echo s=$?; jq -n '[inputs]' -c /nonexistent /tmp/f; echo s=$?; jq -n 1 /nonexistent; "
     "echo s=$?"),
    ("parse: jq one reader across operands",
     "printf '[1,' > /tmp/c.json; printf '2]\\n\"z\"\\n' > /tmp/d.json; jq -c '[., input_filename]' /tmp/c.json /tmp/d.json; "
     "printf '\"ab' > /tmp/q1; printf 'c\"' > /tmp/q2; jq -c . /tmp/q1 /tmp/q2; "
     "jq -n -c 'input_filename, input, input_filename, input_line_number' /tmp/d.json; "
     "echo 5 | jq -n '[input, input_filename]' -c"),
    ("parse: jq raw input across operands",
     "printf 'a' > /tmp/r1; printf 'b\\nc' > /tmp/r2; jq -R -c '[., input_line_number, input_filename]' /tmp/r1 /tmp/r2; "
     "jq -Rs . /tmp/r1 /tmp/r2"),
    ("parse: jq jq 1.8 builtins jaq lacked",
     "printf '[{\"id\":1,\"v\":\"a\"},{\"id\":2,\"v\":\"b\"}]' | jq -c 'INDEX(.id)'; "
     "jq -nc '[1,2] | JOIN({\"1\":\"x\"}; tostring)'; jq -nc '\"xabcx\" | trimstr(\"x\")'; "
     "jq -n 'have_decnum, have_literal_numbers'; jq -nc '[1,\"a\"] | format(\"csv\"), format(\"json\"), format(\"text\")'; "
     "jq -nc '1 | format(\"x\")'; echo s=$?"),
    # the rest of jq's edge cases.
    ("parse: jq input -0 keeps its sign",
     "echo '[-0, -00, -0.0]' | jq -c .; echo '-0' | jq .; echo '[-0]' | jq -c 'map(tostring)'"),
    ("parse: jq limit, skip and nth refuse negative counts",
     "jq -nc '[limit(-1; 1,2,3)]'; echo s=$?; jq -nc '[skip(-1; 1,2,3)]'; echo s=$?; "
     "jq -nc '[nth(-1; 1,2,3)]'; echo s=$?; jq -nc '[limit(0; 1,2,3)], [skip(0; 1,2)], [first(empty)]'"),
    ("parse: jq implode",
     "jq -nc '[65, 66, 128512] | implode'; for x in '[-1]' '[1114112]' '[55296]' '[1.5]' '[0.5, 65.9]' "
     "'[\"a\"]' '\"a\"' '[nan]' '[{\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\":1}]'; do jq -nc \"$x | implode\"; "
     "echo s=$?; done"),
    ("parse: jq input past the end",
     "jq -n 'input'; echo s=$?; echo 1 | jq -c '[., input]'; echo s=$?; echo '1 2' | jq -c '[inputs]'; echo s=$?"),
    ("parse: jq input and inputs meet a parse error",
     "printf '1 x 2' | jq -n '[inputs]'; echo s=$?; printf '1 x 2' | jq -nc 'input, input, input'; echo s=$?; "
     "printf '1 2 x 3' | jq -c '[., input]'; echo s=$?"),
    ("parse: jq tonumber edge cases",
     "jq -nc '[\"1\", \" 1\", \"1 \", \"0x10\", \"1e3\", \"nan\", \"-\", \"\", \"1.5.6\", \"+1\", \".5\", \"1e1000\", \"-0\"]"
     " | map(try tonumber catch .)'"),
    # jq 1.8's tonumber and fromjson wording.
    ("parse: jq tonumber and fromjson messages",
     "for x in '\"abc\"' '\"1 2\"' '[1]' 'null'; do jq -n \"$x | tonumber\"; echo s=$?; done; "
     "for x in '\"1 2\"' '\"{\"' '\"[1,]\"' '\"nan\"' '\"x\"' '1' '\"\"'; do jq -n \"$x | fromjson\"; echo s=$?; done; "
     "jq -nc '[\"Infinity\", \"+Infinity\", \"-inf\"] | map(fromjson | tostring)'; "
     "jq -nc '[\"true\", \"false\", \"x\", 1] | map(try toboolean catch .)'"),
    # jq computes on doubles.
    ("parse: jq big integers in arithmetic become doubles",
     "echo '[12345678901234567890123]' | jq -c 'map(.+0)'; "
     "echo '[12345678901234567890123, 100000000000000000001, 1.000000000000000000001]' | jq -c .; "
     "echo '12345678901234567890123' | jq '. == 12345678901234567890124, tostring, (.|tojson), length, -.'; "
     "jq -nc '[9007199254740993 + 0, 9007199254740993, 4611686018427387904 * 2]'; "
     "jq -nc 'def fib: recurse([.[1], add])[0]; nth(100; [0, 1] | fib)'"),
    ("parse: jq the sign of zero",
     "jq -nc '[-0, -0.0, 0 * -1, -0 + 0, 0 / -1, -(0), -1 * 0, 0 * -1.5]'; jq -n -- '-0.0'; jq -n '0 * -1'; "
     "jq -n '[0 * -1] | tojson'; jq -n '-0 | tostring'; jq -nc '[-0.4 | round, (-0.0 | floor)]'; "
     "jq -nc '[1,2,3] | [.[0 * -1], .[-1.0], .[1.5], .[-1.5]]'"),
    ("parse: jq remainder truncates to integers",
     "jq -nc '[5.5 % 2, -5.5 % 2, 5 % 2.5, 7 % -3, -7 % 3, 1e30 % 7, 9007199254740993 % 2, 5 % -1, 5 % 1e30]'; "
     "jq -n '5 % 0.5'; echo s=$?; jq -n 'nan % 2 | isnan'"),
    ("parse: jq rounding past 2^53",
     "jq -nc '[(1e30|floor), (12345678901234567890123|floor), (2.5|round), (-2.5|round), (nan|floor), "
     "(infinite|ceil), (2e22 | round | tostring)]'"),
    # Extras: jq's error positions and wording.
    ("parse: jq error positions name the operand and line",
     "echo '\"x\"' > /tmp/a.json; printf '1\\n' > /tmp/b.json; jq '.a' /tmp/a.json /tmp/b.json; echo s=$?; "
     "echo 1 > /tmp/f1; printf '\\n\\n\"x\"\\n' > /tmp/f2; jq '.a' /tmp/f1 /tmp/f2; echo s=$?; "
     "printf '1' > /tmp/e.json; printf '\\n2' > /tmp/g.json; jq -c '.a' /tmp/e.json /tmp/g.json; echo s=$?; "
     "printf '{\"a\":1}\\n\\n\\n{\"b\":2}\\n' | jq '.[0]'; echo s=$?"),
    ("parse: jq error positions under -s and -R",
     "printf '1 2' | jq -s '.a'; printf '1\\n2\\n' | jq -s '.a'; printf '1\\n2\\n' | jq -R '.a'; "
     "printf '1\\n2\\n' | jq -R -s '.a'; printf '' | jq -s '.a'; echo s=$?; jq -n 'error(\"x\")'; "
     "printf '1\\n2\\n' | jq '{a:.} | error'; echo s=$?"),
    ("parse: jq index errors are jq's",
     "printf '1\\n\"x\"\\n[1]\\n{}\\nnull\\ntrue\\n' | jq '.a'; jq -n '[1] | .[\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"]'; "
     "jq -n '{} | .[[1]]'; jq -n '1 | .[0]'; jq -n '\"abc\" | .[0]'; jq -n 'true | .[{}]'; jq -n '{} | .[null]'; "
     "jq -nc '1 | indices(1)'; echo s=$?"),
    ("parse: jq exit statuses follow jq's last input",
     "jq -e -n 'null'; echo s=$?; jq -e -n 'empty'; echo s=$?; printf '1 null' | jq -e .; echo s=$?; "
     "printf 'null 1' | jq -e 'if . == 1 then empty else . end'; echo s=$?; printf '1 2' | jq -e 'error(\"x\")'; "
     "echo s=$?; jq -n '{} | halt_error'; echo s=$?; jq -n '\"bye\\n\" | halt_error(1)'; echo s=$?"),
    ("parse: jq --slurpfile and --rawfile read as jq does",
     "printf '[1,2' > /tmp/s.json; jq -n --slurpfile x /tmp/s.json '$x'; echo s=$?; printf '1 2' > /tmp/s2.json; "
     "jq -nc --slurpfile x /tmp/s2.json '$x'; printf 'a\\377b' > /tmp/raw; jq -n --rawfile x /tmp/raw '$x'"),
    # Compound-assignment subscripts (from brush-runtime): blanks belong to the key, and an
    # indexed array's keys are arithmetic.
    ("parse: blanks inside a compound-assignment subscript belong to it",
     "b=([ 1 ]=x); declare -p b; a=(p); a+=([ 2 ]=y); declare -p a; "
     "b=([ 1 + 1 ]=x [3]=y [ 4]=z [5 ]=w); declare -p b; b=([\t1\t]=x); declare -p b; b=([\n1]=x); declare -p b; "
     "b=( [ 1 ]=x ); declare -p b; b=( [1]=a [ 2 ]=b c ); declare -p b"),
    ("parse: blanks inside an associative compound-assignment key",
     "declare -A m=([ a b ]=x); declare -p m; "
     "declare -A m=([ a b ]=x [c d]=y ['e f']=z [\"g h\"]=w [ $HOME ]=v); declare -p m"),
    ("parse: a bracketed word without = is an element",
     "b=([ 1 ] =x); echo s=$?; declare -p b; b=([ 1 ]); declare -p b"),
    ("parse: declare and local compound assignments with blank subscripts",
     "declare -a c=([ 1 ]=x); declare -p c; f() { local d=([ 2 ]=q); declare -p d; }; f"),
    ("parse: indexed compound-assignment subscripts are arithmetic",
     "x=1; b=([ x ]=q [ $x+1 ]=r); declare -p b; b=([2**3]=x [-1]=y); declare -p b; "
     "b=(a b c); b+=([-1]=z); declare -p b; x=3; b=([x]=q [x+1]=r [\\$x]=s); declare -p b; "
     "b=([$(echo 5)]=q); declare -p b; i=0; b=([i++]=a [i++]=b); declare -p b i"),
    ("parse: a subscript that does not evaluate ends the shell",
     "b=([1 2]=x); echo s=$?; declare -p b"),
    ("parse: a subscript that does not evaluate ends the shell in declare",
     "declare -a c=([x y]=1); echo s=$?"),
    ("parse: a subscript that does not evaluate ends the shell in local",
     "f() { local d=([x y]=q); echo in; }; f; echo s=$?"),
    ("parse: a negative subscript before the first element",
     "b=([-5]=x); echo s=$?"),
    ("parse: a negative subscript before the first element when appending",
     "b=([1]=a); b+=([-5]=x); echo s=$?"),
    ("parse: an empty compound-assignment subscript abandons the command",
     "e=; b=(1 [$e]=x 2); echo s=$?\necho s=$?; declare -p b"),
    ("parse: an empty compound-assignment subscript abandons the command in declare",
     "declare -a b=(1 []=x 2); echo s=$?\necho s=$?; declare -p b"),
    ("parse: a subscript before the first element abandons the command after the elements before it",
     "v=-5; b=(1 [$v]=x$v 2)\necho s=$?; declare -p b; declare -a c=(1 [$v]=x$v 2)\necho s=$?; declare -p c\n"
     "f() { local -a d=(1 [$1]=x 2); echo in; }; f -9\necho s=$?"),
    ("parse: declare's element subscript counts back from the array's end",
     "a=(1 2); declare 'a[-1]=x'; declare -p a; declare 'a[-5]=y'; echo same $?; declare -p a; "
     "f() { local 'a[-5]=y'; echo in $?; declare -p a; }; f; declare 'b[]=x'; echo same $?; "
     "x=3; declare 'c[$x]=v'; declare -p c"),
    ("parse: declare's element += appends to the element",
     "a=(1 2); declare -a 'a[-1]+=z'; declare -p a; declare 'a[0]+=q'; declare 'a[5]+=n'; declare -p a; "
     "declare -ai n=(1 2); declare 'n[0]+=5'; declare -i 'n[1]+=3'; declare -p n; declare -A m=([k]=v); "
     "declare 'm[k]+=w'; declare 'm[j]+=x'; declare -p m; s=abc; declare 's[0]+=d'; declare -p s; "
     "f() { local 'b[0]+=x'; declare -p b; }; b=(p q); f; declare -p b"),
    ("parse: declare's empty associative subscript is a bad array subscript",
     "declare -A m; declare 'm[]=x'; echo same $?; declare -p m; declare -A 'm2[]=x'; echo same $?; "
     "declare -p m2; f() { local -A m3; local 'm3[]=x'; echo in $?; declare -p m3; }; f; "
     "declare -A m4=([k]=v); declare -r 'm4[]=x'; echo same $?; declare -p m4; k=; declare \"m[$k]+=x\"; "
     "echo same $?"),
    ("parse: declare's element without a subscript or value is not a valid identifier",
     "declare 'a[]'; echo same $?; declare -p a; a=(1); declare 'a[]'; echo same $?; declare -p a; "
     "declare -A 'm[]'; echo same $?; declare 'b[]=x'; echo same $?; declare -p b; "
     "declare -a 'c[-1]=x'; echo same $?; declare -p c"),
    ("parse: declare evaluates a compound subscript as an assignment does",
     "x=3; declare -a c=([\\$x]=1); declare -p c; x=0; declare -a a=([x++]=$x [x++]=$x); declare -p a x"),
    # jq, continued.
    ("parse: jq a computed zero negates to -0, a literal zero to 0",
     "jq -nc '[((1-1) | -.), -(1-1), -(0), ([] | length | -.), (\"\" | length | -.), (0 | -.)]'; "
     "echo '[0, 0.0, -0]' | jq -c 'map(-.)'; echo 0 | jq -- '-.'; jq -nc '[0 + 0, 0 - 0] | map(-.)'"),
    ("parse: jq string repetition as jq 1.8 counts",
     "jq -nc '[\"ab\" * 0, \"ab\" * 1.5, \"ab\" * 2.7, \"ab\" * -1, 0 * \"ab\", \"ab\" * 0.5, \"ab\" * nan, "
     "\"\" * 5, \"ab\" * (1-1)]'"),
    ("parse: jq --seq reads a JSON text sequence",
     "printf '\\x1e1\\n\\x1e[2]\\n' | jq -c --seq .; echo s=$?; echo 1 | jq --seq -c .; echo s=$?; "
     "printf '\\x1e1' | jq --seq -c .; echo s=$?; printf '\\x1e{\"a\":1\\x1e2\\n' | jq --seq -c .; echo s=$?; "
     "printf '\\x1e1\\x1e2\\n' | jq --seq -c .; printf '\\x1e1 \\x1e2 \\n' | jq --seq -c .; "
     "printf '\\x1e[1,}\\n\\x1e3\\n' | jq --seq -c .; printf 'x\\x1e3\\n' | jq --seq -c .; "
     "printf '\\x1e\"a' | jq --seq -c .; printf '\\x1e1\\n\\x1e[2]\\n' | jq -c --seq -s .; "
     "printf '\\xef\\xbb1\\x1e2\\n' | jq --seq -c .; printf '\\x1etrue\\x1enull\\n' | jq --seq -c .; echo s=$?"),
    ("parse: jq iteration and arithmetic errors are jq's",
     "for p in '1 | .[]' 'null | .[]' '\"a\" | .[]' '{} + 1' '[] + {}' '\"a\" + 1' '1 - \"a\"' '[1] - 1' "
     "'{} * 2' '\"a\" * {}' '1 / \"a\"' '[] / []' '{} % 1' '\"a\" % 2' '-\"a\"' '-[1]' 'null | abs' "
     "'1 | add' '1 | map(.)' '1 | any' '1 | .a = 1' '{} | .[0] = 1' '[] | .[\"a\"] = 1' '[1] | .[nan] = 3' "
     "'[1] | .[1.5] = 3' '1 | .[1:2]' '\"abc\" | .[1:\"a\"]' '[1,2,3] | [.[1.5:], .[:1.5], .[-1.5:]]' "
     "'1 | {(.): 2}'; do printf '%s => ' \"$p\"; jq -nc \"$p\" 2>&1 | tr '\\n' '|'; echo; done"),
    ("parse: jq builtin errors are jq's",
     "for p in '1 | keys' 'null | keys' '1 | to_entries' '1 | with_entries(.)' 'true | length' "
     "'1 | has(\"a\")' '{} | has(0)' '{} | has(null)' '[1] | has(0.5)' '1 | contains(\"a\")' "
     "'true | contains(false)' '1 | inside(\"a\")' '1 | sort' '1 | sort_by(.)' '{} | sort_by(.)' "
     "'1 | unique' '1 | min' '{\"a\":1} | min' '{\"a\":1} | min_by(.)' '1 | reverse' '\"abc\" | reverse' "
     "'null | reverse' '1 | flatten' '[1] | flatten(-1)' '{\"a\":[1]} | flatten' '[1] | from_entries' "
     "'[{\"key\":null}] | from_entries' '1 | combinations' '1 | range(.;\"a\")' '1 | limit(\"a\"; 1)' "
     "'1 | getpath(1)' '1 | delpaths(1)' '1 | delpaths([1])' '{} | delpaths([[\"a\",0]])' "
     "'{\"a\":1} | delpaths([[]])' '[1] | del(.[5])'; do printf '%s => ' \"$p\"; jq -nc \"$p\" 2>&1 | "
     "tr '\\n' '|'; echo; done"),
    ("parse: jq string builtin errors are jq's",
     "for p in '1 | split(\",\")' '1 | test(\"a\")' '\"a\" | test(1)' '\"A\" | test([\"a\",\"i\"])' "
     "'\"A\" | test([])' '\"a\" | test(\"a\"; \"q\")' '1 | sub(\"a\";\"b\")' '\"a\" | sub(1;\"b\")' "
     "'\"a1b2\" | [scan(\"([a-z])([0-9])\")]' '1 | ascii_downcase' '1 | explode' '1 | ltrimstr(\"a\")' "
     "'\"a\" | ltrimstr(1)' '1 | startswith(\"a\")' '1 | endswith(\"a\")' '1 | trim' '1 | utf8bytelength' "
     "'{} | @sh' '[{}] | @sh' '1 | @base64d' '\"%%%\" | @base64d' '\"YQ\" | @base64d' '\"YWJjZA\" | @base64d' "
     "'\"=\" | @base64d' '\"/w==\" | @base64d | explode' '\"a\" | sqrt' '[] | floor' 'pow(\"a\";2)'; "
     "do printf '%s => ' \"$p\"; jq -nc \"$p\" 2>&1 | tr '\\n' '|'; echo; done"),
    ("parse: jq date builtins as jq 1.8",
     "for p in '\"x\" | fromdate' '\"x\" | strptime(\"%Y\")' '1 | strptime(\"%Y\")' '\"x\" | mktime' "
     "'[1] | mktime' '[] | mktime' '[2024,0,1] | mktime' '[2024.5,0,1,0,0,0,0,0] | mktime' '[\"a\"] | mktime' "
     "'\"x\" | gmtime' '\"x\" | localtime' '1 | strftime(1)' '\"x\" | strftime(\"%Y\")' '[1] | todate' "
     "'86400.5 | todate' '\"2000-01-01T00:00:00.5Z\" | fromdate'; do printf '%s => ' \"$p\"; jq -nc \"$p\" "
     "2>&1 | tr '\\n' '|'; echo; done"),
    ("parse: jq builtins jaq lacked: $__loc__, lgamma_r, get_search_list and builtins",
     "jq -nc '1 | $__loc__'; jq -nc '\n\n $__loc__'; jq -nc '[1.5, -0.5, 0, -2, 3] | map(lgamma_r)'; "
     "jq -nc 'get_search_list'; jq -L /tmp -nc 'get_search_list'; jq -nc 'get_search_list' -L /tmp -L /x; "
     "jq -nc 'builtins | length'; jq -nc '[builtins][0][0:8]'; jq -nc '1 | modulemeta'; echo s=$?"),
    ("parse: jq names the first unknown letter of a short option",
     "jq -null; echo s=$?; jq -nx 1; echo s=$?; jq --nope; echo s=$?"),
    ("parse: jq modulemeta is refused",
     "jq -n '\"foo\" | modulemeta'; echo s=$?"),
    ("parse: jq module imports jq does not find",
     "jq -n 'import \"a\" as a; 1'; echo s=$?; jq -n 'include \"a\"; 1'; echo s=$?"),
    ("parse: jq module imports jq finds are refused",
     "mkdir -p /tmp/m; echo 'def f: 7;' > /tmp/m/a.jq; jq -n -L /tmp/m 'import \"a\" as a; a::f'; "
     "echo s=$?; jq -n -L /tmp/m 'include \"a\"; f'; echo s=$?"),
    # jq's own parser: bison's syntax errors, recovery and parse-time checks.
    ("parse: jq syntax errors are bison's, at jq's locations",
     'for p in \'.[\' \')\' \'1 )\' \'1 | )\' \'{\' \'{a:}\' \'[1,]\' \'.a.\' \'"abc\' \'@\' \'$\' \'. as [$a\' \'def f: ;\' \'.[1:2:3]\' \'.. ..\' \'"\\(1"\' \'{a b}\' \'[.[] | select(.a == 1]\' \'.["a"\' \'label\' \'import\' \'1 2\' \'.a b\' \'é\' \'foreach .[] as $x (0; .; .; .)\' \'reduce . as $x\' \'{(1):2} | )\' \') | {(1):2}\'; do jq -n "$p" 2>&1 | tr \'\\n\' \'|\'; echo " s=${PIPESTATUS[0]}"; done'),
    ('parse: jq recovers from syntax errors as jq does',
     'for p in \'if . then 1\' \'if . then 1 else 2\' \'try 1 catch\' \'. as {a b} | 1\' \'{a b: 1}\' \'{(1): 2, a b: 3}\' \'.a | .%\' \'. | break\' \'break\' \'.123abc\' \'{a: (1}\' \'[.[] | {a: .b, c: .d\' \'1 +\n2 +\n\' \'if 1 then 2 elif\' \'"\\(1 + )"\'; do jq -n "$p" 2>&1 | tr \'\\n\' \'|\'; echo " s=${PIPESTATUS[0]}"; done'),
    ('parse: jq reports string escapes, and the end of a program after blanks or a comment, as jq does',
     'for p in \'"\\q"\' \'"a\\u12"\' \'"\\ud800"\' \'"x\\\' \'. |  \' \'. | # c\' \'.[1] |\n  # comment\n\' \'1 +\t\n\'; do jq -n "$p" 2>&1 | tr \'\\n\' \'|\'; echo " s=${PIPESTATUS[0]}"; done'),
    ('parse: jq checks module metadata and import paths as it parses',
     'for p in \'module 1; .\' \'module {a: .}; .\' \'import "a" as a 1; .\' \'import "a\\(1)" as a; .\' \'module {}; .\' \'module {a: 1}; def f: 1; f\'; do jq -n "$p" 2>&1 | tr \'\\n\' \'|\'; echo " s=${PIPESTATUS[0]}"; done'),
    ('parse: jq refuses a program without a main query',
     'for p in \'\' \'def f: 1;\' \'def f: 1; def g: 2;\' \'# c\n\' \'   \'; do jq -n "$p" 2>&1 | tr \'\\n\' \'|\'; echo " s=${PIPESTATUS[0]}"; done; printf \'def f: 1;\\n\' > /tmp/p.jq; jq -n -f /tmp/p.jq; echo s=$?; : > /tmp/e.jq; jq -n -f /tmp/e.jq; echo s=$?'),
    ('parse: jq names an undefined label as it binds it, at its break',
     'for p in \'break $x\' \'label $a | break $b\' \'label $a | break   $b\' \'[.[] | break $x]\' \'break $x, break $y\'; do jq -n "$p" 2>&1 | tr \'\\n\' \'|\'; echo " s=${PIPESTATUS[0]}"; done'),
    # jq's depth limits: flat chains, nesting, evaluation and deep values.
    ('parse: jq runs flat chains of 10,000 elements',
     'p="[$(printf \'1,%.0s\' $(seq 9999))1]"; jq -n "$p | length"; p="$(printf \'1 + %.0s\' $(seq 9999))1"; jq -n "$p"; p="$(printf \'1 - %.0s\' $(seq 9999))1"; jq -n "$p"; p="$(printf \'.a%.0s\' $(seq 10000))"; jq -n "{} | $p"; p="$(printf \'.[0]?%.0s\' $(seq 10000))"; jq -n "[] | $p"'),
    ('parse: jq runs flat chains of 1,000 elements',
     'p="$(printf \'. | %.0s\' $(seq 999))."; jq -n "$p"; p="$(printf \'true and %.0s\' $(seq 999))true"; jq -n "$p"; p="$(printf \'null // %.0s\' $(seq 999))1"; jq -n "$p"; p="$(printf \'1 as $x | %.0s\' $(seq 1000))\\$x"; jq -n "$p"; p="{$(for i in $(seq 999); do printf \'a%s:%s,\' $i $i; done)a1000:1000}"; jq -n "$p | length"; p="\\"$(printf \'\\\\(1)%.0s\' $(seq 1000))\\""; jq -n "$p | length"; p="[$(printf \'1,%.0s\' $(seq 999))1] as [$(for i in $(seq 999); do printf \'$a%s,\' $i; done)\\$a1000] | \\$a1000"; jq -n "$p"; p="2 | if . == 0 then 0 $(for i in $(seq 999); do printf \'elif . == %s then %s \' $i $i; done)else -1 end"; jq -n "$p"; p="$(for i in $(seq 1000); do printf \'def f%s: %s; \' $i $i; done)f1000"; jq -n "$p"'),
    ('parse: jq runs a program nested 256 levels deep',
     'p="$(printf \'(%.0s\' $(seq 256))1$(printf \')%.0s\' $(seq 256))"; jq -n "$p"; p="$(printf \'[%.0s\' $(seq 256))1$(printf \']%.0s\' $(seq 256))"; jq -nc "$p | flatten"; p="$(printf \'if true then %.0s\' $(seq 256))1$(printf \' end%.0s\' $(seq 256))"; jq -n "$p"'),
    ('parse: jq refuses a program nested more than 256 levels deep',
     'p="$(printf \'(%.0s\' $(seq 257))1$(printf \')%.0s\' $(seq 257))"; jq -n "$p"; echo s=$?; p="$(printf \'[%.0s\' $(seq 1000))1$(printf \']%.0s\' $(seq 1000))"; jq -nc "$p"; echo s=$?'),
    ('parse: jq refuses evaluation that would recurse past the stack',
     "jq -n 'def f: if . > 0 then (. - 1 | f) + 1 else 0 end; 2000 | f'; echo s=$?; jq -n 'def f: if . > 0 then . - 1 | f else . end; 100000 | f'; echo s=$?"),
    ('parse: jq compares, contains, merges, writes and frees deeply nested values',
     "jq -nc 'reduce range(3000) as $i (null; [.]) | [., .] | [.[0] == .[1], .[0] < .[1], (.[0] | contains(.))]'; jq -nc 'reduce range(3000) as $i (null; {a: .}) | . * . | [..] | length'; jq -nc 'reduce range(12000) as $i (1; [.]) | tojson | [length, .[10000:10021]]'; jq -nc 'reduce range(100000) as $i (null; [.]) | length'"),
    ('parse: jq prints values nested deeper than 1,000 levels',
     'jq -nc \'reduce range(2000) as $i (null; [.])\' | md5sum; jq -n \'reduce range(1200) as $i (null; {a: .})\' | md5sum; jq -nc \'reduce range(10002) as $i (1; [.])\' | md5sum; printf \'%s\' "$(printf \'[%.0s\' $(seq 2999))$(printf \']%.0s\' $(seq 2999))" > /tmp/d.json; jq -c \'[..] | length\' /tmp/d.json; jq -c . /tmp/d.json | md5sum'),
    ("parse: jq runs a binary operator's right operand first",
     'jq -nc \'[(1,2) + (10,20) + (100,200)]\'; jq -nc \'[(1,2) * (3,4)]\'; jq -nc \'[(1,2) < (1,2)]\'; jq -nc \'try (error("x") + error("y")) catch .\'; jq -nc \'[try ((1, error("a")) + (10, 20)) catch .]\'; jq -nc \'2 | ["\\(., .+1) \\(., .*2)"]\'; jq -nc \'[{a: (1,2), b: (3,4)}]\''),
    # Associative compound assignments and xtrace of compound assignments.
    ("parse: an associative array's elements need subscripts once the first has one",
     "declare -A m=([x]=1 [y]); echo s=$?"),
    ("parse: an associative array's elements need subscripts in a plain assignment",
     "v=b; declare -A m; m=(a b); declare -p m; m=([a]=1 $v); echo s=$?"),
    ("parse: an associative array's elements need subscripts when appending",
     "declare -A m; m+=(c d); declare -p m; m+=([e]=f g); echo s=$?"),
    ("parse: an associative array's elements need subscripts in local",
     "f() { local -A m=([x]=1 [y]); echo in; }; f; echo s=$?"),
    ("parse: key-value pairs fill an associative array without subscripts",
     "declare -A m=(a b c d); declare -p m; declare -A n=(a b c); declare -p n"),
    ("parse: set -x shows a compound assignment as written",
     "x=5; set -x; a=($x \"$x\" [1]=$x '[2]=q' $(echo 7)); b+=( [ 3 ]=y ); m=(); c=(\"\" x); set +x"),
    ("parse: set -x shows a declaration's compound assignments quoted",
     "x=5; set -x; declare -a c=(1 \"a b\" [5]=$x $x); declare -A d=([k]=\"v w\" [j]=$x); f() { local e=(1 [3]=2); }; "
     "f; readonly r=(q); export s=(t); declare -a x=(1) y=2 z; declare -A e=(); declare -a q=(''); "
     "declare -a z=(\"it's\" $'a\\tb'); declare -a z+=([5]=2); set +x"),
    # Declaration builtins take options only before the first name.
    ("parse: declare takes options only before the first name",
     "declare -a x -r v=4; echo s=$?; declare -p x v; declare x -- y; echo s=$?; declare -p x y; "
     "declare -i n1 +i n2; echo s=$?; declare -p n1 n2; declare -- -x; echo s=$?; declare x=1 -p; echo s=$?"),
    ("parse: local, readonly, export and typeset take options only before the first name",
     "f() { local a -r b=1; echo s=$?; declare -p a b; }; f; readonly a -f; echo s=$?; declare -p a; "
     "export e1 -n; echo s=$?; declare -p e1; typeset t1 -i t2=3; echo s=$?; declare -p t1 t2"),
    ("parse: declare's must-use-subscript error shows the element as written",
     "v=b; (declare -A m=([a]=1 \"$v c\")); echo s=$?; (declare -A m; m=([a]=1 \"$v c\")); echo s=$?"),
    # jq refuses a constant non-string object key when it compiles the program.
    ("parse: jq refuses a constant non-string object key before running",
     "jq -nc '{(1): 2}'; echo s=$?; jq -nc '{( 1 ): 2}'; echo s=$?; jq -nc '{a: 1, (2): 3, (4): 5}'; "
     "echo s=$?; jq -nc '{(1): {(2): 3}}'; echo s=$?; jq -nc '. as {(1): $x} | $x'; echo s=$?; "
     "jq -nc 'def f: {(1):2}; 3'; echo s=$?; jq -nc '{(1):2} | foo'; echo s=$?; jq -nc '{\n(1): 2}'; "
     "echo s=$?; jq -nc '{(1 # c\n): 2}'; echo s=$?"),
    ("parse: jq folds constant object keys as jq does",
     "for p in '{(1+1): 2}' '{(0.1+0.2): 2}' '{(1<2): 3}' '{([]): 1}' '{([1,[2,{\"a\":[3]}]]): 1}' "
     "'{({a:(1+1)}):1}' '{($__loc__): 1}' '{(1.50): 2}' '{(1e3): 2}' '{(null+1):1}' '{(\"a,b\" / \",\"): 1}' "
     "'{({$__loc__}):1}' '{([@base64 \"x\"]):1}' 'def true: 5; {(true): 1}' '{(true):1} | {(false): 2}'; "
     "do jq -nc \"$p\" 2>&1 | tr '\\n' '|'; echo \"s=${PIPESTATUS[0]}\"; done"),
    ("parse: jq leaves object keys it cannot fold to run time",
     "for p in '{(-1): 2}' '{(1,2): 3}' '{(nan): 1}' '{(\"a\"+1): 1}' '{(1/0):1}' '{([-1]):1}' '{({a}):1}' "
     "'{(.):1}' '{(\"a\"+\"b\"): 1}' '{(@base64 \"x\"):1}' '{(1 // 2): 1}' '{([1,\"a\"+1]):1}'; "
     "do jq -nc \"$p\" 2>&1 | tr '\\n' '|'; echo \"s=${PIPESTATUS[0]}\"; done"),
    ("parse: jq get_jq_origin and get_prog_origin",
     "mkdir -p /tmp/o3/real && cd /tmp/o3 && ln -s real link && echo 'get_prog_origin' > real/p.jq && "
     "jq -n -f link/p.jq; cd link && jq -nc '[get_jq_origin, get_prog_origin]'; env jq -n get_jq_origin; "
     "cd / && jq -n -f tmp/o3/real/../real/p.jq; cd /tmp/o3/real && jq -n -f p.jq; jq -n --from-file ./p.jq"),
]

NESTING = (
    "bash-tool's limit: nesting deeper than its WASM stacks can hold is refused before the script "
    "runs; bash runs until its own stack overflows"
)

MODULES = (
    "bash-tool does not load jq modules (import, include, modulemeta): one jq would find on its "
    "search path is refused; one it would not find is jq's own error"
)

EXPECTED = {
    "parse: jq refuses a program nested more than 256 levels deep": (
        0,
        b"s=2\ns=2\n",
        b"jq: maximum nesting level exceeded: deeper nesting is unsupported in bash-tool\n"
        b"jq: maximum nesting level exceeded: deeper nesting is unsupported in bash-tool\n",
        "bash-tool's limit: jaq parses, compiles and runs a program recursively on the stack the "
        "shell uses, so a program nested more than 256 levels deep (fewer inside nested shell calls) "
        "is refused before it runs; jq runs it",
    ),
    "parse: jq refuses evaluation that would recurse past the stack": (
        0,
        b"s=2\n0\ns=0\n",
        b"jq: maximum nesting level exceeded: deeper nesting is unsupported in bash-tool\n",
        "bash-tool's limit: jaq evaluates recursively on the stack the shell uses, so evaluation "
        "that would recurse past what it holds (a recursive definition that is not a tail call, "
        "here 2,000 levels deep) stops with a refusal; jq runs it. Tail calls do not recurse",
    ),
    "parse: jq modulemeta is refused": (
        0, b"s=2\n", b"jq: modulemeta is unsupported in bash-tool\n", MODULES,
    ),
    "parse: jq module imports jq finds are refused": (
        0, b"s=2\ns=2\n",
        b"jq: module imports are unsupported in bash-tool\njq: module imports are unsupported in "
        b"bash-tool\n",
        MODULES,
    ),
    "parse: a subscript nested too deeply at run time": (
        0,
        b's=1\ndeclare -a a=([0]="p" [1]="q")\ns=1\ns=1\ndeclare -a a=([0]="p" [1]="q")\n',
        b"bash: line 2: unset: maximum nesting level exceeded: deeper nesting is unsupported in "
        b"bash-tool\nbash: line 3: maximum nesting level exceeded: deeper nesting is unsupported "
        b"in bash-tool\nbash: line 5: maximum nesting level exceeded: deeper nesting is "
        b"unsupported in bash-tool\n",
        "bash-tool's limit: text built at run time (a subscript from a variable's value) nested "
        "deeper than the word parser's stack holds fails with an error; bash evaluates it",
    ),
    "parse: 1000 nested parameter expansions": (
        2, b"", b"bash: shell code is nested too deeply for bash-tool\n", NESTING,
    ),
    "parse: 3000 nested quoted parameter expansions": (
        2, b"", b"bash: shell code is nested too deeply for bash-tool\n", NESTING,
    ),
    "parse: deeply nested text given to eval is refused where eval runs": (
        0, b"after 2\n", b"bash: shell code is nested too deeply for bash-tool\n", NESTING,
    ),
    "parse: 1000 nested parameter expansions in a here-document": (
        2, b"", b"bash: shell code is nested too deeply for bash-tool\n", NESTING,
    ),
    "parse: file names that are not UTF-8": (
        0, b"s=1\ne=1\n", b"bash: line 1: a\xe9: Illegal byte sequence\n",
        "WASI names files with Unicode strings, so a file name holding bytes that are not UTF-8 "
        "cannot exist: creating one fails with EILSEQ (Illegal byte sequence) instead of creating "
        "a file with a different name",
    ),
}
