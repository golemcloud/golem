"""Shell runtime semantics: readonly, noclobber, tests, aliases, xtrace, traps, and the order in
which bash expands and assigns.

Scripts run as `bash -c` would.
"""

CASES = [
    # noclobber guards &> and >&word as it guards >; &>> appends.
    (
        "runtime: noclobber refuses &> and >&word over a file",
        "cd /tmp; echo old > f; set -C; echo new &> f; echo \"s1=$?\"; echo dup >& f; echo \"s2=$?\"; "
        "echo abs >& /tmp/f; echo \"s3=$?\"; echo app &>> f; echo \"s4=$?\"; echo n &> /dev/null; echo \"s5=$?\"; "
        "echo y >| f; echo \"s6=$?\"; echo z &> newf; echo \"s7=$?\"; cat f newf",
    ),
    # readonly covers array elements, prefix assignments, locals and functions.
    (
        "runtime: readonly array element assignment abandons the line",
        "readonly -a a=(1 2); a[0]=3; echo \"status=$? ${a[0]}\"\necho next=$? ${a[*]}",
    ),
    (
        "runtime: readonly array element in a function and an associative array",
        "a=(x); readonly a; f() { a[1]=y; echo in-f; }; f; echo after\n"
        "declare -A m=([k]=1); readonly m; m[k]=2; echo \"s=$? ${m[k]}\"\necho \"${a[*]} ${m[k]}\"",
    ),
    (
        "runtime: readonly variables refuse builtins that assign them",
        "readonly r=1; read r <<< z; echo s=$? $r; printf -v r %s q; echo s=$? $r; declare r=4; echo s=$?; "
        "export r=5; echo s=$?; mapfile r < /dev/null; echo s=$?; getopts a r; echo s=$?; echo r=$r",
    ),
    (
        "runtime: readonly elements refuse read and printf -v",
        "readonly -A m=([k]=1); read \"m[k]\" <<< z; echo s=$? ${m[k]}; declare m[k]=5; echo s=$?; "
        "printf -v \"m[k]\" %s q; echo s=$? ${m[k]}",
    ),
    (
        "runtime: an assignment prefix does not override a readonly variable",
        "readonly r=1; f() { r=2 echo in-f; echo \"still $r\"; }; f; echo after=$?; r=3 :; echo s=$?",
    ),
    (
        "runtime: a local cannot shadow a readonly variable",
        "readonly r=1; f() { local r; echo \"a $? $r\"; declare r=3; echo \"b $? $r\"; local r=4; echo \"c $? $r\"; }; "
        "f; echo \"after $? $r\"",
    ),
    (
        "runtime: a for loop over a readonly variable fails and carries on",
        "readonly r=1; f() { for r in 1; do echo loop; done; echo in $?; }; f; echo after $?",
    ),
    (
        "runtime: readonly functions cannot be redefined or unset",
        "f() { echo f; }; g() { echo g; }; export -f g; readonly -f f; f() { echo changed; }; echo s=$?; f; "
        "unset -f f; echo s=$?; unset f; echo s=$?; f; readonly -f; declare -F; declare -pF g",
    ),
    # unset of all elements, quoted associative keys, readonly elements; empty keys.
    (
        "runtime: unset of every element of an indexed array",
        "a=(x y); unset 'a[@]'; echo \"${#a[@]} [${a+set}]\"; declare -p a; b=(x y); unset 'b[*]'; echo ${#b[@]}; "
        "a+=(z); declare -p a; declare -A m=([k]=v [j]=w); unset 'm[@]'; echo ${#m[@]}; "
        "x=1; unset 'x[@]'; echo \"s=$? [${x-unset}]\"; y=1; unset 'y[0]'; echo \"[${y-unset}]\"; "
        "z=1; unset 'z[1]'; echo \"s=$? [${z-unset}]\"",
    ),
    (
        "runtime: unset removes quotes around an associative key",
        "declare -A m=([k]=v [q]=2 [a]=1); m[\"x y\"]=3; unset \"m['k']\"; unset 'm[\"q\"]'; unset 'm[x y]'; "
        "k=a; unset 'm[$k]'; echo ${#m[@]}",
    ),
    (
        "runtime: unset refuses readonly variables and elements",
        "readonly a=(1 2); unset 'a[0]'; echo s=$?; unset a; echo s=$?; unset 'a[@]'; echo s=$?; "
        "readonly -A m=([k]=1); unset 'm[k]'; echo s=$?; declare -p a m",
    ),
    ("runtime: an empty associative key is a bad subscript", "declare -A m; m[\"\"]=v; echo \"s=$?\"\necho next ${#m[@]}"),
    ("runtime: an empty key from an expansion is a bad subscript", "declare -A m; k=; m[$k]=v; echo \"s=$?\"\necho next ${#m[@]}"),
    ("runtime: an empty indexed subscript is a bad subscript", "a=(1); a[]=v; echo \"s=$?\"\necho next ${a[*]}"),
    # -r/-w/-x/-O/-G/-ef answer from the filesystem, not constants.
    (
        "runtime: file tests answer for existing and missing paths",
        "cd /tmp; echo x > a; mkdir d\nfor t in -e -r -w -x -O -G; do for p in a d /tmp/nope; do "
        "test $t $p && echo \"test $t $p\"; [ $t $p ] && echo \"[ $t $p\"; done; done\n"
        "[[ -x d && ! -x a && ! -r /tmp/nope && ! -w /tmp/nope && -r a && -w d ]] && echo double",
    ),
    (
        "runtime: -ef compares files, not paths",
        "cd /tmp; echo x > a; echo y > b; ln a h; mkdir d\n"
        "[ a -ef b ] || echo differ; [ a -ef ./a ] && echo same; [ a -ef h ] && echo hardlink; "
        "[ a -ef /tmp/nope ] || echo missing; [[ d -ef /tmp/d/. ]] && echo dir\n"
        "for f in a b; do [ \"$f\" -ef a ] && [ \"$f\" != a ] && rm \"$f\"; done; ls",
    ),
    # an associative array's subscript in (( )), $(( )) and let is its key.
    (
        "runtime: counting words in an associative array",
        "declare -A count\nfor w in apple pear apple; do ((count[$w]++)); done\n"
        "echo \"apple=${count[apple]} pear=${count[pear]}\"\ndeclare -A m=([k]=5)\necho $((m[k] * 2))",
    ),
    (
        "runtime: associative keys that are not identifiers",
        "declare -A m; ((m[ k ]=1)); ((m[foo.txt]++)); ((m[a-b]+=2)); f=x.y; ((m[$f]++)); ((m[\"q\"]=3)); "
        "let 'm[z w]=4'; let 'm[y]+=3'; echo $((m[foo.txt]*10)) ${m[y]}; for k in \" k \" foo.txt a-b x.y q \"z w\"; do "
        "echo \"[$k]=${m[$k]}\"; done; echo ${#m[@]}",
    ),
    (
        "runtime: indexed subscripts in arithmetic are still evaluated",
        "a=(5 6 7); i=1; echo $((a[i+1])) $((a[ 1 ])) $((a[a[0]-4])); ((a[i]++)); ((a[i*2]+=10)); echo ${a[*]}; "
        "declare -A m; ((m[k]=4)); declare -p m; m[x]=1; m[x]=$((m[x]+1)); echo \"x=${m[x]}\"; "
        "k=key; (( m[$k] += 2 )); echo \"key=${m[key]} $(( m[x] ))\"",
    ),
    # PIPESTATUS survives the compound command that contains the pipeline.
    (
        "runtime: PIPESTATUS after compound commands",
        "p() { echo \"$1: ${PIPESTATUS[*]}\"; }\n{ false | true; }; p group\n{ false | true; } | { false; }; p grouppipe\n"
        "(false | true); p subshell\nf() { false | true; }; f; p func\ng() { false | true; return 3; }; g; p funcret\n"
        "if false | true; then :; fi; p if\nif false | true; then false | false | true; fi; p if2\n"
        "for i in 1; do false | true; done; p for\nwhile false | true; do break; done; p while\n"
        "while false; do :; done; p whilefalse\ncase x in x) false | true;; esac; p case\n(( 0 )); p arith\n"
        "[[ -z x ]]; p cond\nfalse | true; x=1; p assign\n! false | true; p bang\n{ true | false; } > /dev/null; p groupredir\n"
        "false | true; case x in y) ;; esac; p emptycase\nfalse | true; for i in; do :; done; p emptyfor\n"
        "false | true; { false | true; } > /tmp/no/x; p redirfail\nfalse | true; h() { :; }; p funcdef",
    ),
    # an arithmetic error or a failed compound redirection fails only that command.
    (
        "runtime: arithmetic errors fail the command and the list goes on",
        "y=0\n((x = 10 / y)); echo \"after (( )): $?\"\nlet 'z = 1 / 0'; echo \"after let: $?\"\n"
        "[[ 1/0 -eq 1 ]]; echo \"after [[: $?\"\nfor ((i = 0; i < 1/0; i++)); do echo in; done; echo \"after for: $?\"\n"
        "((2**-1)); echo \"neg $?\"; f() { ((1/0)); echo \"in f $?\"; }; f; echo \"after f $?\"\necho end",
    ),
    (
        "runtime: a readonly variable in arithmetic fails the command",
        "readonly k=1; ((k = 2)); echo \"after ro: $? $k\"; let k=3; echo \"let: $?\"; ((k++)); echo \"inc: $? $k\"",
    ),
    (
        "runtime: a failed redirection fails only its compound command",
        "{ echo in; } > /tmp/no/x; echo \"after redir: $?\"; if true; then echo in; fi > /tmp/no/y; echo \"if: $?\"; "
        "while false; do :; done < /tmp/no/z; echo \"while: $?\"; v=1 > /tmp/no/x; echo \"[${v-unset}] $?\"",
    ),
    (
        "runtime: empty and blank arithmetic is zero",
        "echo $(( )) $(()) $((  )); x=; echo $((x + 1)); (( )); echo \"s=$?\"",
    ),
    (
        "runtime: an unset variable in arithmetic under set -u ends the shell",
        "set -u; ((y + 1)); echo \"after $?\"",
    ),
    (
        "runtime: an unset variable in let under set -u ends the shell",
        "set -u; f() { let 'q=y+1'; echo \"in $?\"; }; f; echo \"after $?\"",
    ),
    # promptvars is on, so PS4 and ${x@P} expand as in bash.
    (
        "runtime: PS4 is expanded in xtrace",
        "PS4='+ line ${LINENO}: '; set -x; echo traced\nf() { echo in-f; }\n"
        "PS4='+${FUNCNAME[0]:-main}:${LINENO}: '; f; set +x",
    ),
    (
        "runtime: promptvars is on by default and can be restored",
        "shopt promptvars; echo s=$?; [[ $BASHOPTS == *promptvars* ]] && echo listed; shopt -u promptvars; "
        "PS4='$x '; x=X; set -x; : off; set +x; shopt -s promptvars; echo s=$?; set -x; : on; set +x",
    ),
    ("runtime: the P transform expands a prompt string", "p='a\\$ ${x}'; x=val; echo \"${p@P}\""),
    # a simple command is traced to the stderr it had before its own redirections.
    (
        "runtime: xtrace ignores the traced command's redirections",
        "set -x; echo hi > /tmp/out.txt 2>&1; v=$(echo captured 2>&1); x=1 2>/dev/null; (( x++ )) 2>/dev/null; "
        "[[ -n x ]] 2>/dev/null; { echo grp; } 2>/dev/null; f() { echo inf; }; f 2>/dev/null; y=2 echo pre 2>/dev/null; "
        "set +x; cat /tmp/out.txt; echo \"v=$v\"",
    ),
    # assigning RANDOM seeds bash's generator; assigning SECONDS restarts the count.
    (
        "runtime: RANDOM=seed repeats bash's sequence",
        "RANDOM=42; a=$RANDOM; RANDOM=42; b=$RANDOM; [[ $a == \"$b\" ]] && echo equal\n"
        "RANDOM=42; echo $RANDOM $RANDOM $RANDOM; RANDOM=0; echo $RANDOM $RANDOM; RANDOM=-5; echo $RANDOM; "
        "RANDOM=4294967338; echo $RANDOM; RANDOM=40+2; echo $RANDOM; (( RANDOM = 7 )); echo $RANDOM; "
        "declare -i RANDOM=9; echo $RANDOM",
    ),
    (
        "runtime: SECONDS=n restarts the count at n",
        "SECONDS=100; (( SECONDS >= 100 && SECONDS <= 101 )) && echo hundred; SECONDS=5+5; "
        "(( SECONDS >= 10 && SECONDS <= 11 )) && echo ten; SECONDS=-3; (( SECONDS >= -3 && SECONDS <= -2 )) && echo negative; "
        "SECONDS=0; (( SECONDS < 2 )) && echo reset; declare -p SECONDS | cut -c1-19",
    ),
    # aliases chain, a trailing blank expands the next word, a function sees the
    # aliases of its definition, and any text defines an alias.
    (
        "runtime: chained aliases and a trailing blank",
        "shopt -s expand_aliases\nalias ll='echo LL' l=ll\nl\nalias g='echo G ' h=hello\ng h\n"
        "alias run='echo run ' w=word\nrun w\nalias sudo='sudo '\nalias e='echo E'\nalias ls='ls -d'\nls /tmp\n"
        "alias a=b b=a\na 2>&1 | sed 's/line [0-9]*/line N/'\n\\ll; \"ll\"",
    ),
    (
        "runtime: an alias is not expanded on its own line or in an earlier function",
        "shopt -s expand_aliases\nf() { hi; }\nalias hi='echo hi'; hi\nhi\nf\ng() { hi; }\nalias hi='echo changed'\ng\n"
        "unalias hi\ng",
    ),
    (
        "runtime: any text defines an alias",
        "alias q1=\"a'b\"; echo \"alias status=$?\"\nalias q1\nalias z='echo it'\\''s'; alias; alias q=\"it's\"; alias q",
    ),
    # declaration builtins and the environment.
    (
        "runtime: a word that expands to name=value assigns",
        "v='x=1'; export $v; echo \"[${x-unset}]\"; env | grep '^x='; i=1; declare v$i=x; echo \"[$v1]\"; "
        "w='y=2 z=3'; declare $w; echo \"$y $z\"; f() { local $w; echo \"in $y\"; local a$i+=q; echo \"[$a1]\"; }; f; "
        "u='r=1'; readonly $u; echo \"r=$r\"; e='b[1]=x'; declare $e; echo \"${b[1]}\"",
    ),
    (
        "runtime: export before assignment",
        "export ev; declare -p ev; env | grep -c '^ev'; ev=1; env | grep '^ev'; declare -x ux; env | grep -c '^ux'; "
        "bash -c 'echo \"[${ev-unset}] [${ux-unset}]\"'; export 1x; echo s=$?",
    ),
    (
        "runtime: arrays are not exported",
        "export a=(x); declare -A m=([k]=v); export m; bash -c 'echo \"[${a-unset}] [${m-unset}]\"'; env | grep -c '^[am]='",
    ),
    ("runtime: declare -lu converts neither way", "declare -lu v=MiX; echo $v; declare -ul w=MiX; echo $w; declare -l x=MiX; echo $x"),
    # LINENO in $( ), backquotes and eval, and diagnostics that name the right line.
    (
        "runtime: LINENO in substitutions and eval",
        "echo \"top $LINENO\"\nx=$(echo \"cmdsub $LINENO\")\necho \"$x\"\neval 'echo \"eval $LINENO\"'\n"
        "eval $'echo \"e1 $LINENO\"\\necho \"e2 $LINENO\"'\ny=`echo \"bq $LINENO\"`; echo $y\n"
        "f() {\n  echo \"f $LINENO $(echo \"fc $LINENO\")\"\n  eval 'echo fe $LINENO'\n}\nf\n(echo \"sub $LINENO\")\necho \"after $LINENO\"",
    ),
    (
        "runtime: errors inside a substitution name their line",
        "true\necho $(cat < /tmp/nope)\nv=$(cd /nowhere)\neval 'cd /nowhere2'\necho end",
    ),
    (
        "runtime: an unset variable in a function body names its line",
        "set -u\nmyfunc() {\n  local unset_local\n  echo \"$unset_local\"\n}\nmyfunc\necho \"should not print\"",
    ),
    # read's field splitting, read -n 0, and NUL and control bytes in read and mapfile.
    ("runtime: read splits fields as bash does", 't() { local ifs=$1 in=$2; IFS=$ifs read -r a b c <<< "$in"; printf \'%s|\' "[$a]" "[$b]" "[$c]"; IFS=$ifs read -r -a arr <<< "$in"; printf \' arr(%d):\' ${#arr[@]}; printf \'[%s]\' "${arr[@]}"; echo; }\nt \': \' \'key : value\'\nt \': \' \'key : value : more : x\'\nt \',\' \'a,b,c,\'\nt \',\' \'a,b,c,d,\'\nt \',\' \'a,,b\'\nt \',\' \',a\'\nt \', \' \' a , , b ,\'\nt \', \' \'a,b,c,d , \'\nt \':\' \'x::y\'\nt \':\' \'x::y::\'\nt \' \' \'  a  b  c  d  \'\nt \': \' \'a :: b\'\nt \',\' \'a,b\'\nt \',\' \'a,b,\'\nt \', \' \'a, b, c, \'\nt \',\' \',\'\nt \',\' \',,\''),
    (
        "runtime: read -n 0 reads nothing",
        "read -n 0 x <<< abc; echo \"s=$? [$x]\"; read -N 0 y <<< abc; echo \"s=$? [$y]\"",
    ),
    (
        "runtime: read and mapfile keep control bytes and end strings at NUL",
        "printf 'a\\0b\\n' | { read r; echo \"[$r]\"; }; printf 'a\\0b\\0' | { read -d '' r; echo \"[$r]\"; }; "
        "printf 'a\\001b\\033c\\003d\\004e\\n' | { read r; printf %s \"$r\" | od -An -c; }; "
        "printf 'a\\001b\\003c\\004d\\nz\\n' | { mapfile m; printf %s \"${m[0]}\" | od -An -c; echo ${#m[@]}; }; "
        "printf 'a\\0b\\n' | { mapfile -t a; printf %s \"${a[0]}\" | od -An -c; }; "
        "printf 'a\\0b\\0' | { mapfile -d '' m; declare -p m; }; printf 'a\\0b\\0' | { mapfile -t -d '' m; declare -p m; }",
    ),
    # an out-of-range break or continue count leaves every loop with status 1.
    (
        "runtime: an out-of-range loop count leaves every loop",
        "for i in 1 2; do for j in a b; do break 0; done; echo $i; done; echo \"end $?\"\n"
        "for i in 1 2; do for j in a b; do continue 0; done; echo $i; done; echo \"cend $?\"\n"
        "for i in 1 2; do while :; do break -1; done; echo $i; done; echo \"neg $?\"\n"
        "for i in 1 2; do break 5; echo $i; done; echo \"big $?\"\n"
        "for i in 1 2; do for j in a; do continue 2; echo no; done; echo $i; done; echo \"c2 $?\"",
    ),
    # ERR runs once, for the command that failed; DEBUG runs before (( )), [[ ]] and each
    # for iteration; RETURN runs after source and sees the status before `return`.
    (
        "runtime: ERR runs once, for the command that failed",
        "trap 'echo err $BASH_COMMAND' ERR\n{ false; }\n( false )\nif false; then :; fi\nfor i in 1; do false; done\n"
        "while false; do :; done\ncase x in x) false;; esac\ntrue | false\n(( 0 ))\n[[ -z x ]]\n{ true; false; true; }\necho end",
    ),
    (
        "runtime: DEBUG runs before (( )), [[ ]] and each for iteration",
        "trap 'echo \"D: $BASH_COMMAND\"' DEBUG; (( 1 )); [[ x ]]; [[ -z x && y ]]; x=1; { :; }; if true; then :; fi; "
        "for i in 1 2; do :; done; for x; do :; done; trap - DEBUG",
    ),
    (
        "runtime: RETURN sees the status before return, and runs after source",
        "f() { trap 'echo \"ret $?\"' RETURN; return 4; }; f; echo \"s=$?\"\n"
        "g() { trap 'echo \"gret $?\"' RETURN; false; }; g; echo \"s=$?\"\n"
        "k() { trap 'echo \"kret $?\"' RETURN; (exit 3); return 5; }; k; echo \"s=$?\"\ntrap - RETURN\n"
        "echo 'echo body; return 7' > /tmp/s.sh; trap 'echo \"sret $?\"' RETURN; . /tmp/s.sh; echo \"s=$?\"\n"
        "echo 'echo body2' > /tmp/s2.sh; . /tmp/s2.sh; echo \"s=$?\"",
    ),
    # nameref self-references and cycles.
    (
        "runtime: a nameref to itself is refused",
        "declare -n r=r; echo \"status=$?\"; declare -p r 2>&1; f() { declare -n q=q; echo \"f $?\"; }; f",
    ),
    (
        "runtime: a circular nameref warns and assigning through it fails",
        "declare -n a=b b=a; echo \"s=$?\"; a=1; echo same\necho next; echo \"[$a]\"; x=$a; echo \"x=[$x]\"; b=2 && echo no\necho last",
    ),
    # associative arrays list their elements in bash's hash-table order.
    (
        "runtime: associative arrays iterate in bash's order",
        "declare -A port=([web]=80 [db]=5432 [cache]=6379 [api]=8080)\nfor svc in \"${!port[@]}\"; do echo \"$svc=${port[$svc]}\"; done\n"
        "declare -p port; echo \"${port[@]}\"\ndeclare -A m=([one]=1 [two]=2 [three]=3 [four]=4); echo ${!m[@]}\n"
        "unset 'port[db]'; port[db]=1; port[zz]=2; echo \"${!port[@]}\"\n"
        "declare -A t; for i in $(seq 1 40); do t[x$i]=1; done; echo \"${!t[@]}\"",
    ),
    (
        "runtime: a large associative array grows as bash's does",
        "declare -A m; for i in $(seq 1 3000); do m[k$i]=$i; done; echo \"${!m[@]}\" | md5sum\n"
        "declare -A u; for i in $(seq 1 2100); do u[y$i]=1; done; for i in $(seq 1 1500); do unset \"u[y$i]\"; done; "
        "u[new]=1; echo \"${!u[@]}\" | md5sum; echo ${#u[@]}",
    ),
    # >&-N closes and leaves N as a word; redirection diagnostics name what the script wrote.
    (
        "runtime: a dash after >& or <& closes the descriptor",
        "cd /tmp; echo x >&-1; echo \"s=$?\"; echo y 2>&-1; echo \"s=$?\"; echo z >&-; echo \"s=$?\"; "
        "echo w 1>&-2; echo \"s=$?\"; echo v >&1-; echo \"s=$?\"; cat <&-1; echo \"s=$?\"; ls -A /tmp",
    ),
    (
        "runtime: redirection diagnostics name what the script wrote",
        "echo x > ''; echo \"s=$?\"; cat < ''; echo \"s=$?\"; echo a >> ''; echo s=$?; x=; echo r > \"$x\"; echo s=$?; "
        "echo q &> ''; echo s=$?; exec {fd}>&-; echo s=$?; fd=9; echo z >&$fd; echo s=$?; echo z >&\"$fd\"; echo s=$?",
    ),
    (
        "runtime: a quoted declaration is still refused for readonly and +a",
        "side_effect=0; readonly var=1; declare -i 'var=side_effect=1'; echo \"status: $? side effect: $side_effect\"; "
        "declare -a arr=(1 2); declare +a 'arr=(3 4)'; echo \"s=$?\"; declare +a arr; echo \"s=$?\"; "
        "declare -A m=([k]=v); declare +A m; echo \"s=$?\"; x=1; declare +a x; echo \"s=$?\"; declare -p arr m x",
    ),
    # trap -p and compgen list in bash's order, and compgen -A function honours its prefix.
    (
        "runtime: trap -p lists traps in bash's order",
        "trap 'echo r' RETURN; trap 'echo d' DEBUG; trap 'echo t' TERM INT; trap 'echo h' HUP; trap 'echo e' ERR; "
        "trap 'echo x' EXIT; trap -p; trap; trap - EXIT RETURN DEBUG ERR TERM INT HUP",
    ),
    (
        "runtime: compgen lists keywords, options and functions in bash's order",
        "zfa() { :; }; other() { :; }; zfb() { :; }\ncompgen -A function zf\ncompgen -k | tr '\\n' ' '; echo\n"
        "compgen -A setopt | tr '\\n' ' '; echo\ncompgen -A setopt err; compgen -A shopt ext",
    ),
    # cd -, CDPATH, cd .. at /, cd //, DIRSTACK and the first PIPESTATUS.
    ("runtime: cd - without OLDPWD", "cd -; echo \"status=$? [$PWD]\"; pwd; OLDPWD=; cd -; echo \"s=$? [$PWD]\"; cd ''; echo \"s=$? [$PWD]\""),
    (
        "runtime: cd follows CDPATH and leaves / at /",
        "cd / && cd ..; echo \"status=$? $PWD\"; cd /..; echo \"s=$? $PWD\"; cd /tmp/../..; echo \"s=$? $PWD\"\n"
        "mkdir -p /tmp/a/b /tmp/c/sub; CDPATH=/tmp/c; cd sub; echo \"s=$? $PWD\"; cd /tmp; CDPATH=:/tmp/a; cd b; echo \"s=$? $PWD\"\n"
        "cd /tmp/a; CDPATH=/tmp; cd b; echo \"s=$? $PWD\"; CDPATH=/tmp/a cd ./b; echo \"s=$?\"; cd /; cd nonexist/..; echo \"s=$?\"",
    ),
    (
        "runtime: cd // keeps two slashes, DIRSTACK and PIPESTATUS start as bash's",
        "echo \"[${PIPESTATUS[@]}]\"; echo \"${DIRSTACK[@]} ${#DIRSTACK[@]}\"; cd //; pwd; cd tmp; pwd; cd ///; pwd; "
        "cd //tmp/..; pwd; cd /; pushd /tmp >/dev/null; echo \"${DIRSTACK[@]}\"",
    ),
    # time follows TIMEFORMAT.
    (
        "runtime: time follows TIMEFORMAT",
        "TIMEFORMAT=; time true; TIMEFORMAT=took; { time true; } 2>&1; TIMEFORMAT='a%%b [%0lR] [%' ; time true; "
        "TIMEFORMAT='%x'; time true; declare TIMEFORMAT=done; { time -p true; } 2>&1 | head -c 5; echo",
    ),
    # xpg_echo, and the last of -e and -E wins.
    (
        "runtime: echo follows xpg_echo and the last of -e and -E",
        "echo -eE 'a\\tb'; echo -Ee 'a\\tb'; echo -ne 'x\\n'; echo -x; echo --; echo -n -e 'y\\n'; echo -nq z\n"
        "shopt -s xpg_echo; echo 'a\\tb'; echo -E 'a\\tb'; echo 'c\\c'; echo d",
    ),
    # command_not_found_handle runs in a subshell and gives the status.
    (
        "runtime: command_not_found_handle handles an unknown command",
        "command_not_found_handle() { echo \"handled $1 [$2]\"; x=changed; return 9; }; x=orig; nosuchcommand arg; "
        "echo \"$? $x\"; unset -f command_not_found_handle; nosuch3; echo $?",
    ),
    # test's -a binds tighter than -o.
    (
        "runtime: test -a binds tighter than -o",
        "test x -o '' -a ''; echo $?; [ '' -a x -o x ]; echo $?; [ x -o x -a '' ]; echo $?; [ '' -o x -a x -o '' ]; echo $?",
    ),
    # a here-document or here-string for a program is expanded as in bash's child.
    (
        "runtime: here-document side effects stay in the program's child",
        "n=0; cat <<< $((n+=1)); echo \"hs n=$n\"; cat /dev/null $((n+=10)) 2>/dev/null; echo \"arg n=$n\"\n"
        "cat > /dev/null <<EOF\n$((n+=100)) ${m:=set}\nEOF\necho \"hd n=$n m=${m-unset}\"; f() { cat; }; f <<EOF\n$((n+=1000))\nEOF\n"
        "echo \"fn n=$n\"; { cat; } <<EOF\n$((n+=10000))\nEOF\necho \"grp n=$n\"; read x <<EOF\n$((n+=5))\nEOF\necho \"read n=$n x=$x\"",
    ),
    # builtin usage errors in bash's words and with bash's statuses.
    ("runtime: exit with too many operands ends the shell", "exit 1 2; echo after"),
    ("runtime: exit with a word that is not a number fails", "exit abc; echo \"after $?\"; (exit 3a); echo \"sub $?\"; (exit -1); echo \"neg $?\""),
    ("runtime: return with too many operands ends the shell", "h() { return 1 2; echo in; }; h; echo same\necho next"),
    (
        "runtime: return operands",
        "f() { return -1; }; f; echo \"neg $?\"; g() { return abc; }; g; echo \"abc $?\"; return 1; echo \"top $?\"",
    ),
    (
        "runtime: invalid options are reported with the builtin's usage",
        "cd -Z; echo \"cd $?\"; pwd -Q; echo \"pwd $?\"; hash -Q; echo \"hash $?\"; type -Q ls; echo \"type $?\"; "
        "wait -Q; echo \"wait $?\"; trap -Q; echo \"trap $?\"; set -Q; echo \"set $?\"; shopt -Q; echo \"shopt $?\"; "
        "cd /tmp /; echo \"cd2 $?\"",
    ),
    (
        "runtime: read, source, printf and command usage errors",
        "read -n x v < /dev/null; echo \"readn $?\"; read -u 9 x; echo \"readu $?\"; source; echo \"src $?\"; .; echo \"dot $?\"; "
        "printf -v 1bad %s x; echo \"printf $?\"; command nosuchcmd; echo \"cmd $?\"",
    ),
    (
        "runtime: dirs, pushd and popd take +N and -N",
        "cd /; mkdir -p /tmp/d1 /tmp/d2 /tmp/d3; dirs +1; echo \"s=$?\"; popd; echo \"s=$?\"; pushd; echo \"s=$?\"; "
        "pushd /tmp/d1 >/dev/null; pushd /tmp/d2 >/dev/null; pushd /tmp/d3 >/dev/null; dirs; dirs +1; dirs -0; dirs +5; echo \"s=$?\"; "
        "pushd +2; echo \"s=$?\"; dirs -v; popd +1; echo \"s=$?\"; popd -0; echo \"s=$?\"; pushd; echo \"s=$?\"; "
        "popd +9; echo \"s=$?\"; pushd -9; echo \"s=$?\"; popd; popd; popd; echo \"s=$?\"",
    ),
    (
        "runtime: expansion diagnostics name the parameter",
        "set --; : ${1:=x}\necho next; r='a b'; echo ${!r}\necho next2; f() { : ${1:?need}; }; f\necho next3",
    ),
    (
        "runtime: kill, wait and getopts diagnostics",
        "kill; echo \"s=$?\"; kill abc; echo \"s=$?\"; kill -0 999999; echo \"s=$?\"; wait abc; echo \"s=$?\"; "
        "getopts a 1x; echo \"s=$?\"",
    ),
    # the diagnostic prefix on a builtin's broken-pipe write error and the NUL warning.
    (
        "runtime: write errors and the NUL warning carry the diagnostic prefix",
        "trap '' PIPE; { sleep 0.1; echo late; } | true; x=$(printf 'a\\0b'); echo \"[$x]\"",
    ),
    # invalid input fails as in bash.
    (
        "runtime: invalid names and options fail",
        "set -Z; echo $?; set -q; echo $?; export 1x; echo $?; read 1abc <<< x; echo $?; read a 2b <<< 'x y'; echo \"$? a=$a\"; "
        "mapfile 1bad <<< x; echo $?; alias 'a b=c'; echo $?; alias 'a/b=c'; echo $?; alias 'p=b' 'c d=e' 'f=g'; echo $?; alias p f; "
        "declare -A m; m[]=x; echo $?\necho next",
    ),
    (
        "runtime: declarations, let, logout, compopt and history",
        "let; echo \"let $?\"; declare -A m; declare -a m; echo \"convert $?\"; declare -a n=(1); declare -A n; echo \"convert2 $?\"; "
        "declare -n r='1bad'; echo \"nameref $?\"; declare -n r2='a[1]'; echo \"element $?\"; logout; echo \"logout $?\"; "
        "compopt -o default; echo \"compopt $?\"; history; echo \"history $?\"",
    ),
    (
        "runtime: an invalid regular expression fails [[ with status 2",
        "for re in '[' '(' 'a{' '[[:foo:]]' '(a|'; do [[ a =~ $re ]]; echo \"$? [$re]\"; done; [[ abc =~ b ]]; echo \"ok $?\"",
    ),
    ("runtime: an unset $! is unbound under set -u", "set -u; echo \"[$!]\"; echo after"),
    (
        "runtime: $! is kept after wait",
        "sleep 0 & wait; [ -n \"$!\" ] && echo set; true & p=$!; wait; [ \"$!\" = \"$p\" ] && echo same; set -u; : \"$!\"; echo fine",
    ),
    ("runtime: an unset variable in $(( )) under set -u ends the shell", "set -u\nresult=$((unset_var + 5))\necho \"$result\""),
    # Reported by the tool-layer agent: a pipeline stage ends alone, declarations with expanded
    # names, and read/mapfile on closed descriptors.
    (
        "runtime: TL an error that ends a pipeline stage ends only that stage",
        "unset x; echo a | { echo \"${x:?gone}\"; echo stage-after; } | cat; echo \"after ${PIPESTATUS[*]}\"; echo end",
    ),
    (
        "runtime: TL declarations with expanded names",
        "i=1; declare a$i=x; echo \"[$a1]\"; declare \"b$i=y\"; echo \"[$b1]\"; f() { local c$i=z; echo \"[$c1]\"; }; f",
    ),
    (
        "runtime: TL read and mapfile on closed descriptors",
        "read x <&-; echo \"s=$?\"; read -u 0 y <&-; echo \"s=$?\"; m=(a); mapfile m <&-; echo \"s=$?\"; declare -p m; "
        "mapfile -u 5 q; echo \"s=$?\"; read -u 5 z; echo \"s=$?\"",
    ),
    (
        "runtime: a missing sourced file ends a POSIX-mode shell",
        "cd /tmp; . ./non-existent-file; echo \"normal $?\"; set -o posix; . ./non-existent-file; echo \"after builtin call (status $?)\"",
    ),
    # arithmetic diagnostics name the error and the token as bash 5.3 does, and
    # overflowing literals wrap.
    (
        "runtime: malformed numbers are named as bash names them",
        "for e in 08 '09 + 1' '1 + 08 + 2' 2#3 0xg 0x1g 1a 65#1 1# 0#1 2#1#1 10# 2# 10#12a 37#Z 1_0; do "
        "( echo \"$(($e))\" ); echo \"s=$?\"; done",
    ),
    (
        "runtime: syntax errors name the token bash stops at",
        "for e in '1+' '1 @ 2' '3 4' 'a b c' '1=2' 'b = 3 = 4' 'x++=7' '1 <<= 2' '1 ? 2' '1 ? : 3' '1 ? 2 : ' '(1' "
        "'1 +* 2' '!' '1 < ' '1.5' 'x=' '1++' '--4++' 'a[' 'a[1' \"'a'\" '(a)=1' '-a=1' '1 ? a : b = 2'; do "
        "( echo \"$(($e))\" ); echo \"s=$?\"; done",
    ),
    (
        "runtime: overflowing literals wrap and a bare 0x is 0",
        "echo $((99999999999999999999)) $((18446744073709551616)) $((0777777777777777777777777)) "
        "$((0xffffffffffffffffff)) $((0x)) $((12345678901234567890123 % 7)) $((++1)) $((-- 5)) $((- -5)) $((64#_@))",
    ),
    (
        "runtime: an error in a variable's value names that value",
        "( x='1+'; echo $((x)) ); ( x='1/0'; echo $((x+1)) ); ( x=08; echo $((x+1)) ); ( a=(1 2); echo $((a[1+])) ); "
        "( y=a; a=y; echo $((y + 1)) ); ( a=a; echo $((a)) ); echo end",
    ),
    (
        "runtime: let, (( )) and a division by zero",
        "let '1+'; echo \"s=$?\"; let 'z = 1 / 0'; echo \"s=$?\"; x=0; ((x = 10 / y)); echo \"s=$?\"; ((3 4)); echo \"s=$?\"; "
        "echo $((1/0+1)); echo $(( 5 / 0 ))\necho after",
    ),
    (
        "runtime: an integer assignment that does not evaluate ends the shell",
        "declare -i m; (m='1+'; echo same $?); echo \"sub $?\"; (declare -i n='4 +'; echo same $?); echo \"sub $?\"; "
        "f() { declare -i k='2#'; echo in-f; }; (f; echo after-f); echo \"sub $?\"; (declare -i n=1/0; echo same); echo \"sub $?\"\n"
        "declare -i top='1 +'; echo same $?\necho next",
    ),
    (
        "runtime: a prefix assignment to an integer variable keeps its text",
        "declare -i m=5; m='1+2' eval 'echo \"[$m]\"'; echo \"m=$m\"; f() { echo \"f[$m]\"; declare -p m; }; m='2*3' f; "
        "m='x+' eval 'echo \"[$m]\"'; echo end",
    ),
    # xtrace levels and words, and a bash -c child's options, status and levels.
    (
        "runtime: xtrace levels: stages, subshells, eval, source, substitutions and traps",
        "cd /tmp; echo 'echo h; eval \"echo h2\"' > s; trap 'echo err' ERR\ntrue | { set -x; echo s; set +x; }\n"
        "set -x\n(echo b)\nx=$(echo d)\n"
        "eval 'echo e'\nf() { echo g; }; f\n. /tmp/s\n( y=$(echo i) )\necho $(echo $(echo k))\neval 'eval \"echo m\"'\nfalse\n"
        "trap 'echo dbg' DEBUG; : one; trap - DEBUG\n(trap 'echo x' EXIT; echo in)",
    ),
    (
        "runtime: xtrace prints empty assignments and (( )) as bash does",
        "set -x; a= b='' c=1; x=\"p q\"; ((x=1+2)); ((x > 2)); for ((i=0;i<1;i++)); do :; done; PS4=; echo end",
    ),
    (
        "runtime: a bash -c child's options, xtrace, set -u status and SHLVL",
        "bash -x -c 'echo hi'; bash -x -c 'a=1'; bash -u -c 'echo $nope'; echo \"s=$?\"; bash -o bogus -c 'echo ran'; echo \"s=$?\"; "
        "bash +O bogus -c 'echo ran'; echo \"s=$?\"; bash -o -c 'echo ran'; echo \"s=$?\"; "
        "bash -xe -c 'echo \"$1 $SHLVL $BASH_SUBSHELL\"' n a; echo \"s=$?\"; echo \"SHLVL=$SHLVL\"; SHLVL=x bash -c 'echo $SHLVL'; "
        "(export -n SHLVL; bash -c 'echo $SHLVL'); (bash -u -c 'echo ${nope}'); echo \"s=$?\"; bash -c 'echo ${x?} | cat; echo yes'; echo \"s=$?\"",
    ),
    (
        "runtime: BASH_SUBSHELL counts subshells as bash does",
        "echo $BASH_SUBSHELL | cat; { echo $BASH_SUBSHELL; } | cat; (echo $BASH_SUBSHELL); echo $BASH_SUBSHELL & wait; "
        "{ echo $BASH_SUBSHELL; } & wait; echo $(echo $BASH_SUBSHELL | cat); echo a | (echo $BASH_SUBSHELL; cat); "
        "(echo $BASH_SUBSHELL) & wait; bash -c 'echo $BASH_SUBSHELL'; (bash -c 'echo $BASH_SUBSHELL'); x=$( (echo $BASH_SUBSHELL) ); echo $x",
    ),
    # introspection: caller and BASH_SOURCE, OSTYPE, set -, set -k and set -v.
    (
        "runtime: caller and BASH_SOURCE in a command string",
        "f() { declare -p BASH_SOURCE FUNCNAME BASH_LINENO; caller; echo \"c=$?\"; caller 0; echo \"c0=$?\"; caller 1; echo \"c1=$?\"; }; "
        "g() { f; }\ng\nf\ndeclare -p BASH_SOURCE FUNCNAME BASH_LINENO 2>&1\n"
        "cd /tmp; printf '%s\\n' 'h() { caller 0; caller 1; caller; }' 'h' > s.sh; . ./s.sh; echo sourced\neval 'f'\n"
        "bash -c 'f() { echo \"${BASH_SOURCE[*]} $0\"; caller 0; }; g() { f; }; g' myname",
    ),
    (
        "runtime: OSTYPE names Linux",
        "case $OSTYPE in linux*) echo linux ;; *) echo \"other $OSTYPE\" ;; esac; echo \"$OSTYPE\"",
    ),
    (
        "runtime: set - turns off xtrace and verbose, and set -k passes assignments to the command",
        "set -x; set -; echo quiet; echo \"$-\"; set - a b; echo \"$# $@\"; set - -x -v; echo \"$# $1 $-\"; set -v; set -; echo \"$-\"; "
        "set -k; k() { echo \"[${kw-}] $#\"; }; k kw=1 x; echo a=1 b; k 'kw=2'; set +k; k kw=3",
    ),
    (
        "runtime: set -v echoes each input line as it is read",
        "cd /tmp; printf '%s\\n' 'echo in-file' 'echo two' > f.sh\nset -v\neval \"echo e1\necho e2\"\n. ./f.sh\n"
        "f() { echo fn; }\nf\nbash -c \"echo child\"\ncat <<EOF\nhere\nEOF\nif true; then\n  echo inif\nfi\necho a; set +v; echo b\necho c",
    ),
    # PS4 and ${x@P} run their command substitutions, so the tool checks their
    # text before they expand, as eval's; PS4 expands with xtrace off, and @P leaves $? alone.
    (
        "runtime: PS4's command substitutions run untraced, and @P leaves $? alone",
        "PS4='$(echo x)+ '; set -x; echo hi; set +x; echo \"s=$?\"; x='$(exit 3)z'; y=\"${x@P}\"; echo \"s=$? [$y]\"; "
        "PS4='+$(echo $BASH_SUBSHELL) '; set -x; (echo sub); set +x",
    ),
    (
        "runtime: a refused construct in ${x@P} is refused when it expands",
        "x='$(coproc cat </dev/null; echo in)'; y=\"${x@P}\"; echo same\necho \"s=$? [$y]\"",
    ),
    (
        "runtime: a refused command named in ${x@P} refuses the expansion",
        "x='$(umask)'; echo \"${x@P}\"; echo same\necho \"s=$?\"",
    ),
    (
        "runtime: a refused command reached through ${x@P} stops where it is reached",
        "c=umask; x='$($c)'; y=\"${x@P}\"; echo \"s=$? [$y]\"",
    ),
    (
        "runtime: a refused PS4 is reported and traced as written",
        "PS4='$(coproc true)+ '; set -x; echo hi; set +x; echo \"s=$?\"",
    ),
    (
        "runtime: code nested too deeply in ${x@P} is refused",
        "x='$(echo 1)'; for i in $(seq 200); do x=\"\\$(echo $x)\"; done; y=\"${x@P}\"; echo \"deep ${#y}\"\necho \"s=$?\"",
    ),
    (
        "runtime: compgen -C checks the command it runs",
        "compgen -C 'coproc cat' x; echo \"s=$?\"; c='coproc cat'; compgen -C \"$c\" x; echo \"s=$?\"",
    ),
    # A prompt that expands itself nests without end: bash overflows its stack; ours stops at the
    # nesting limit other recursion has, with an error, instead of trapping the whole call.
    (
        "runtime: a prompt that expands itself stops at the nesting limit",
        "x='${x@P}'; echo \"[${x@P}]\"; echo \"s=$?\"",
    ),
    (
        "runtime: a subshell whose prompt expands itself ends with an error",
        "(x='${x@P}'; echo \"[${x@P}]\"); echo \"sub s=$?\"",
    ),
    (
        "runtime: a PS4 that expands itself is reported and traced as written",
        "PS4='${PS4@P}'; set -x; echo hi; set +x; echo done",
    ),
    # declaration builtins' assignments in xtrace.
    (
        "runtime: xtrace of declaration builtins' assignments",
        "set -x\nexport a=1\nexport e= g=1\ndeclare x=1\ndeclare -a n=() o=('')\ndeclare -A m=([k]=v [c]=\"x y\")\n"
        "readonly r=1\nf() { local l=1 k=(1 2); }; f\nexport p\nreadonly -a ra=(1 2)\nexport -n a2=1\n"
        "export b+=2 c='x y'\nreadonly r2 r3=3\ndeclare -a n2+=(a b)\ndeclare -a e1=( \"a b\" $'c\\td' \"it's\" )\n"
        "declare \"u=(1 2)\"\nx=5 export y=6\nreadonly ro=1; export ro=2\necho done",
    ),
    (
        "runtime: an array written before a command is a string",
        "h=H; w=($h \"a  b\" [k]=v 'q r' *) declare -p w; w=(  1   2 ) declare -p w; a=(x) a+=(y) declare -p a; "
        "f() { declare -p w; }; w=(a b) f; w=(1 2) env | grep '^w='; set -x; w=($h x) true; set +x; declare -p w 2>&1",
    ),
    # the DEBUG trap for each simple command of a pipeline, in the shell that
    # starts the stage.
    (
        "runtime: DEBUG runs for each simple command of a pipeline",
        "trap 'echo \"dbg [$BASH_COMMAND] $BASH_SUBSHELL\" >&2' DEBUG\necho a | cat\ntrue | false | true\n{ echo b; } | cat\n"
        "echo c | (read x; echo $x)\nset -T; echo d | cat\ntrap - DEBUG\n"
        "trap 'n=$((n+1))' DEBUG; true | true | true; trap - DEBUG; echo \"n=$n\"\n"
        "trap 'echo out' DEBUG; echo x | cat -n; trap - DEBUG",
    ),
    # exported functions are in the environment commands see.
    (
        "runtime: env lists exported functions as bash exports them",
        "f() { echo hi; local x=1; if true; then echo y; fi; }; export -f f; g() { :; }; export -f g\n"
        "env | grep -A5 '^BASH_FUNC_f'; env | grep -A1 '^BASH_FUNC_g'; env -u 'BASH_FUNC_f%%' | grep -c BASH_FUNC; "
        "export -nf g; env | grep -c 'BASH_FUNC_g'; bash -c f",
    ),
    (
        "runtime: DEBUG runs before each arithmetic for clause",
        "trap 'echo \"dbg [$BASH_COMMAND]\"' DEBUG\nfor ((i=0 ; i<2; i++ )); do echo \"b$i\"; done\n"
        "for (( ; ; )); do break; done\nfor ((j=0;j<1;j++)) { echo x; }\nfor (( k = 0 ;k < 1 ; k++ )); do :; done\ntrap - DEBUG\n"
        "for ((x=0;x<1;x++)); do echo \"[$BASH_COMMAND]\"; done",
    ),
    # bash expands a command's words, then its assignments, and makes its
    # redirections last, without the command's own variables; a here-document written before a
    # program's name is expanded in the program's process.
    (
        "runtime: words, assignments, then redirections",
        "x=\"$(echo A >&2)\" >\"$(echo R >&2; echo /dev/null)\"\n>\"$(echo R3 >&2; echo /dev/null)\" y=\"$(echo A3 >&2)\"\n"
        "echo \"$(echo W4 >&2)\" >\"$(echo R4 >&2; echo /dev/null)\" \"$(echo W5 >&2)\"\n"
        "f() { :; }; >\"$(echo R6 >&2; echo /dev/null)\" f \"$(echo W6 >&2)\"\n"
        "x=\"$(echo A >&2)\" >\"$(echo R2 >&2; echo /dev/null)\" true \"$(echo W2 >&2)\"\n"
        "cd /tmp; x=A; x=B echo hi >r_$x; ls r_*; y=A; y=B f >s_$y; ls s_*; z=A; z=B cat <<<\"$z\"\n"
        "printf 'from file\\n' > in; echo \"$(cat)\" < in; echo \"[$(cat)]\" 0<in",
    ),
    (
        "runtime: a here-document before a program's name is expanded in its process",
        "n=0; <<EOF cat\n$((n+=1))\nEOF\necho \"n=$n\"\nm=0; <<<\"$((m+=1))\" cat; echo \"m=$m\"\n"
        "k=0; <<EOF read v\n$((k+=1))\nEOF\necho \"k=$k v=$v\"",
    ),
    # Deep and long arithmetic: a chain of operators of any length works; nesting past the
    # limit fails as an arithmetic error instead of overflowing the stack.
    (
        "runtime: long arithmetic chains and 100 levels of nesting",
        "x=\"1$(printf '+1%.0s' {1..30000})\"; echo $((x)); y=\"$(printf '(%.0s' {1..100})1$(printf ')%.0s' {1..100})\"; echo $((y)); "
        "z=\"$(printf -- '- %.0s' {1..100})1\"; echo $((z)); p=\"2$(printf '**1%.0s' {1..99})\"; echo $((p)); "
        "f() { if (( $1 < 73 )); then f $(( $1 + 1 )); else echo $((x)) $((y)); fi; }; f 0",
    ),
    (
        "runtime: arithmetic nested past the limit fails without trapping",
        "x=\"$(printf '(%.0s' {1..10000})1$(printf ')%.0s' {1..10000})\"; echo $((x)); echo same\necho \"s=$?\"; (( x )); echo \"s=$?\"; "
        "let x; echo \"s=$?\"",
    ),
    # a subshell reseeds RANDOM before its first value, as bash does.
    (
        "runtime: each subshell reseeds RANDOM",
        "varied() { local first=$1; shift; for v; do [ \"$v\" != \"$first\" ] && { echo varied; return; }; done; echo same; }\n"
        "varied $(echo $RANDOM) $(echo $RANDOM) $(echo $RANDOM) $(echo $RANDOM) $(echo $RANDOM)\n"
        "RANDOM=5; varied $(echo $RANDOM) $(echo $RANDOM) $(echo $RANDOM) $(echo $RANDOM) $(echo $RANDOM)\n"
        "RANDOM=5; x=$RANDOM; (echo $RANDOM >/dev/null); echo | cat >/dev/null; y=$RANDOM; RANDOM=5; x2=$RANDOM; y2=$RANDOM; "
        "[ \"$x $y\" = \"$x2 $y2\" ] && echo parent-kept\n(RANDOM=7; echo $RANDOM $RANDOM); echo \"$(RANDOM=9; echo $RANDOM)\"",
    ),
    # An arithmetic error in an array subscript ends the shell, reported without the command's
    # name, as bash's does; an indexed array literal's keys are evaluated arithmetically.
    (
        "runtime: an arithmetic error in an array subscript ends the shell",
        "for form in 'a[1+]=x' 'echo \"${a[1+]}\"' 'declare \"a[1+]=x\"' '(( a[1+] ))' 'echo $(( a[1+] ))' '[[ -v a[1+] ]]' "
        "'unset \"a[1+]\"' 'let \"a[1+]\"' 'f() { a[1+]=y; }; f' 'local_f() { local \"a[1+]=z\"; }; local_f' 'a=([1+]=w)'; do "
        "(a=(1 2); eval \"$form\"; echo not-reached); echo \"s=$?\"; done\n"
        "a=(1 2); declare \"a[1+]=x\"; echo not-reached\necho nor-this",
    ),
    (
        "runtime: indexed array literal keys are evaluated arithmetically",
        "a=([1+1]=x); declare -p a; declare -a b=([2*3]=y); declare -p b; declare \"c[1+1]=z\"; declare -p c; i=2; declare d[i+1]=w; "
        "declare -p d; f() { local e[1+2]=v; declare -p e; }; f; declare -a h; h+=([3]=t [1*2]=s); declare -p h; "
        "declare -A m=([1+1]=k); declare -p m; i=0; n=([i++]=a [i++]=b); declare -p n\n"
        "a=(9); a=([2-3]=x y); echo \"same $?\"; declare -p a\necho \"next $?\"; declare -p a\n"
        "b=(1 2 3); b+=([-1]=z [-5]=w q); echo \"same $?\"\ndeclare -p b; d=(1 2); declare -a d+=([-1]=e); declare -p d\n"
        "f=(1 2 3); f+=([-1]=z q [-2]=r); declare -p f",
    ),
    # extra operands to the other builtins, as bash treats them.
    (
        "runtime: extra operands: ignored, usage errors, or too many",
        "pwd x; echo \"pwd $?\"; pwd -P x y; times x >/dev/null; echo \"times $?\"; pushd /tmp /; echo \"pushd $?\"; "
        "popd a b; echo \"popd $?\"; popd +1 x; echo \"popd2 $?\"; dirs x; echo \"dirs $?\"; dirs +0 y; echo \"dirs2 $?\"; "
        "getopts; echo \"getopts $?\"; getopts a; echo \"getopts1 $?\"; break 1 2; echo \"outside $?\"",
    ),
    (
        "runtime: too many operands to break, continue, shift and suspend end the shell",
        "(for i in 1 2; do break 1 2; echo in; done; echo after); echo \"s=$?\"; (for i in 1; do continue 1 2; done; echo after); "
        "echo \"s=$?\"; (set -- a b; shift 1 2; echo after); echo \"s=$?\"; (suspend x; echo after); echo \"s=$?\"\n"
        "f() { shift 1 2; echo in-f; }; f a b; echo \"f $?\"\necho not-reached",
    ),
    (
        "runtime: read evaluates a subscript, readonly refuses one, mapfile -O takes a number",
        "a=(); read \"a[1+1]\" <<< v; declare -p a; i=1; read \"k[i+1]\" <<< u; declare -p k\n"
        "readonly \"b[1]=v\"; echo \"r=$?\"; declare -p b 2>&1; readonly \"b2[1+1]\"; echo \"r2=$?\"\n"
        "mapfile -O 2 d <<< x; declare -p d; mapfile -O 1+1 e <<< y; echo \"m=$?\"; mapfile -O -1 g <<< z; echo \"m2=$?\"; "
        "mapfile -O x f <<< z; echo \"m3=$?\"\n(read \"a[1+]\" <<< w; echo not-reached); echo \"s=$?\"",
    ),
    (
        "runtime: a subscript nested past the limit fails on every path",
        "x=\"$(printf '(%.0s' {1..10000})1$(printf ')%.0s' {1..10000})\"; a=()\n"
        "for form in 'declare \"a[$x]=v\"' 'f() { local \"a[$x]=v\"; }; f' 'read \"a[$x]\" <<< v' 'a[$x]=v' "
        "'echo \"${a[$x]}\"' '[[ -v a[$x] ]]' 'let \"a[$x]=1\"' 'echo $((a[$x]))' 'declare -n r=\"a[$x]\"; r=v'; do "
        "(eval \"$form\"; echo not-reached); echo \"s=$?\"; done; declare -p a",
    ),
    # set -v does not echo the lines after the first of a multi-line $( ).
    (
        "runtime: set -v skips the continuation lines of a multi-line command substitution",
        "set -v\nx=$(echo a\necho b); echo after\ny=`echo c\necho d`\nz=$((1+\n2))\necho \"e\nf\"\n"
        "w=$(echo $(echo g\necho h)\n); echo \"$w\"\nu=$( # comment )\necho i)\nt=$(case x in x) echo j;;\nesac)\necho end",
    ),
    # a function imported by a child shell comes from `environment`, and a
    # diagnostic in a function names the file it came from.
    (
        "runtime: an imported function's source, lines and diagnostics",
        "\n\nx() {\n echo \"a $LINENO\"\n echo \"b $LINENO\"\n\n echo \"c $LINENO\"; y\n}; "
        "y() { echo \"y $LINENO ${BASH_LINENO[*]}\"; nosuch; }; export -f x y; x; bash -c x\n"
        "f() { echo \"[${BASH_SOURCE[*]}] [${FUNCNAME[*]}] [${BASH_LINENO[*]}]\"; caller 0; }; export -f f; bash -c f; "
        "bash -c \"g() { f; }; g\"\ncd /tmp; printf \"g() {\\n  nosuch1\\n}\\n\" > s.sh; . ./s.sh; g; eval \"h() { nosuch2; }\"; h",
    ),
    # in POSIX mode a special builtin's usage error, invalid name, readonly
    # variable or failed redirection ends a non-interactive shell; an operational failure does not.
    (
        "runtime: special builtin errors in POSIX mode",
        'set -o posix\nreadonly r=1\nfor c in \'set -Z\' \'export 1x=2\' \'readonly 2y\' \'unset -Q x\' \'shift 5\' \'shift x\' \'trap -Q\' \'trap x BOGUS\' \'eval "if"\' \'export -Q\' \'times -Q\' \'break\' \': > /nonexistent/f\' \'set -o bogus\' \'exec 3</nonexistent\' \'unset r\' \'r=2 :\' \'export r=2\' \'readonly r=3\' \'. /nonexistent\' \'return\' \'exit x\' \'continue 0\' \'set +o bogus\' \'eval ": > /nonexistent/g"\' \'shift -1\' \'f() { return 2; }; f\' \'eval false\' \'eval "exit 3"\'; do\n  (eval "$c"; echo "  continued") 2>/dev/null; echo "[$c] -> $?"\ndone\n',
    ),
    # bash runs a last command without forking, as `exec` does, so a program
    # there sees SHLVL one lower: a command string's or substitution's last command, a ( )
    # subshell's body, a background command, a function's last command in those, and exec.
    (
        "runtime: SHLVL for a command run in place of the shell",
        't() { printf \'%s -> \' "$1"; bash -c "$1" 2>&1 | grep -E \'^SHLVL=|warning|readonly\' | tr \'\\n\' \' \'; echo; }\nt env\nt \'true; env\'\nt \'env; true\'\nt \'true && true && env\'\nt \'true; true && env\'\nt \'true & env\'\nt \'env # comment\'\nt \'env;\'\nt $\'env\\n\\n\'\nt $\'env\\n \'\nt \'env > /dev/stdout\'\nt \'! env\'\nt \'time env\'\nt \'command env\'\nt \'x=1 env\'\nt \'SHLVL=5 env\'\nt \'SHLVL=5; env\'\nt \'SHLVL=abc; env\'\nt \'SHLVL=5000; env\'\nt \'readonly SHLVL; env\'\nt \'unset SHLVL; env\'\nt \'export -n SHLVL; env\'\nt \'trap "echo x" EXIT; env\'\nt \'trap "echo x" INT; env\'\nt \'trap "" INT; env\'\nt \'trap "echo x" DEBUG; env\'\nt \'f() { env; }; f\'\nt \'exec env\'\nt \'SHLVL=5 exec env\'\nt \'(exec env)\'\nt \'exec env | cat\'\nt \'(exec env | cat)\'\nt \'(env)\'\nt \'(env > /dev/stdout)\'\nt \'(true; env)\'\nt \'(true; true && env)\'\nt \'(trap "echo x" INT; env)\'\nt \'(env) | cat\'\nt \'env & wait\'\nt \'! env & wait\'\nt \'trap "echo x" INT; env & wait\'\nt \'{ env; } & wait\'\nt \'SHLVL=5 env & wait\'\nt \'x=$(env); echo "$x"\'\nt \'x=`env`; echo "$x"\'\nt \'x=$(env 2>&1); echo "$x"\'\nt \'x=$(eval env); echo "$x"\'\nt \'echo "$(env)" | cat\'\nt \'(x=$(env); echo "$x") | cat\'\nt \'x=$(trap "echo t" EXIT; env); echo "$x"\'\nt \'trap "echo t" EXIT; x=$(env); echo "$x"\'\nt \'trap "x=\\$(env); echo \\"\\$x\\"" EXIT\'\nt \'cat <(env)\'\nt \'f() { env; }; x=$(f); echo "$x"\'\nt \'f() { env; true; }; x=$(f); echo "$x"\'\nt \'f() { g; }; g() { true && env; }; x=$(f); echo "$x"\'\nt \'f() { env; }; (f)\'\nt \'f() { env; }; f & wait\'\nt \'bash -c env\'\nt \'x=$(bash -c env); echo "$x"\'\nSHLVL=999 bash -c \'echo "startup $SHLVL"\'\nSHLVL=999 sh -c \'echo "startup $SHLVL"\' name 2>&1\nbash -c \'echo "last $SHLVL"\'\n',
    ),
    # a function's local nameref that comes back to itself (local -n v=v, or a
    # cycle of locals) reads and assigns the global variable of that name, as bash does.
    (
        "runtime: a circular local nameref uses the global variable",
        'v=global\nf() { local -n v=v; echo "in: [$v]"; v=set; echo "in2: [$v]"; declare -p v; }\nf; echo "out: [$v]"; declare -p v\nunset v\ng() { local -n w=w; w=x; echo "g: [$w]"; }\ng; echo "after g: [${w-unset}]"; declare -p w 2>&1\nh() { local v=local; k; }; k() { local -n v=v; echo "k: [$v]"; v=kset; }; h; echo "after h: [$v]"\nn() { declare -n v=v; v=nn; }; v=orig; n; echo "after n: [$v]"\np() { local -n x=x 2>&1; echo "p st=$?"; }; p\nq() { local -n a=b; local -n b=a; a=qa; echo "q: [$a] [$b]"; }; q; echo "after q: [$a] [${b-unset}]"\nr() { local -n v=v; v+=app; v[2]=el; echo "r: [${v[*]}] [${v[0]}] ${#v}"; }; v=base; r; declare -p v\ns() { local -n v=v; echo "${v-x} ${v:-y} ${v:+z} ${#v}"; }; v=g; s; unset v; s\nt() { local -n v=v; v=(a b); }; t; declare -p v\n# read, (( )) and printf -v assign the global too; their warnings are not bash\'s: bash warns once more for each.\nu() { local -n v=v; read v <<< rd; (( v[1] = 5 )); printf -v w %s pf; }; u 2>/dev/null; declare -p v\n',
    ),
    # BASH_VERSINFO[5] is the machine type, $MACHTYPE, and stays readonly.
    (
        "runtime: BASH_VERSINFO[5] is MACHTYPE",
        '[[ ${BASH_VERSINFO[5]} == "$MACHTYPE" ]] && echo same; echo "${#BASH_VERSINFO[@]}"\n'
        'bash -c \'[[ ${BASH_VERSINFO[5]} == "$MACHTYPE" ]] && echo child same\'\n'
        'BASH_VERSINFO[5]=x; echo "st=$?"; declare -p BASH_VERSINFO | cut -c1-15',
    ),
    # Shell clones (subshells, stages, substitutions, jobs) share variable values, positional
    # parameters and aliases until they change them: every change stays in the clone.
    (
        "runtime: clones share state but keep their changes",
        'x=parent; a=(p0 p1); declare -A m=([k]=p); declare -i n=5; s=keep\nshow() { echo "$1: x=$x a=(${a[*]}) m[k]=${m[k]} m[new]=${m[new]-unset} n=$n s=${s-unset}"; }\n( x=sub; a[1]=sub; a+=(more); m[k]=sub; m[new]=1; n+=1; unset s; show sub ); show after-subshell\n{ x=stage; a[0]=stage; unset \'a[1]\'; m[k]=stage; n=7; unset s; show stage; } | cat; show after-stage\necho x | { read -r x; a=(r); m=([k]=r); show read-stage; }; show after-read-stage\ny=$(x=cmdsub; a+=(c); m[k]=c; show cmdsub); echo "$y"; show after-cmdsub\n{ x=bg; a[5]=bg; m[k]=bg; show bg; } & wait; show after-bg\nf() { x=fn; a[1]=fn; m[k]=fn; show fn; }; f | cat; show after-fn-stage\ncat <(x=procsub; a[0]=ps; show procsub); show after-procsub\nx+=-appended; a[1]+=-appended; m[k]+=-appended; n+=10; show parent-writes\n( show sub-sees-writes ); echo end | cat\nset -- p1 \'p 2\' p3; ( set -- s1; echo "sub: $# $*" ); { shift 2; echo "stage: $# $*"; } | cat; echo "args: $# $*"\nf() { ( shift; echo "fn-sub: $*" ); { set -- z; echo "fn-stage: $*"; } | cat; echo "fn: $*"; }; f a b c\nshopt -s expand_aliases; alias g=\'echo alias-parent\'; ( alias g=\'echo alias-sub\' ); alias g=\'echo alias-stage\' | cat; unalias g | cat\ng\n',
    ),
    # brush-parse leftover: the line a diagnostic or LINENO names, as bash numbers it: a simple
    # command by the line its first word ends on; (( )), [[ ]] and a compound command's failed
    # redirection by its last line; a function's by the line its body starts on; an error that
    # leaves a function by the body's line; a $( )'s commands one to a line from the command's.
    (
        "runtime: line numbers in functions, compound commands and substitutions",
        't() { echo "== ${1//$\'\\n\'/⏎}"; bash -c "$1" 2>&1; }\nt $\'f() {\\n  nosuch1\\n  echo "${x:?unset1}"\\n}\\nf\'\nt $\'g() {\\n  local a\\n  (( 1 / 0 ))\\n  echo $(( 2 / 0 ))\\n}\\ng\'\nt $\'if true; then\\n  nosuch2\\n  : "${y?unset2}"\\nfi\'\nt $\'for i in 1; do\\n  nosuch3\\n  : $(( 1 +\\n  ))\\ndone\'\nt $\'while true; do\\n  cd /nonexistent\\n  break\\ndone\'\nt $\'{\\n  nosuch4\\n  a=(1 2\\n  3); echo ${a[5]:?bad}\\n}\'\nt $\'case x in\\n  x) nosuch5\\n     echo ${q:?unset}\\n     ;;\\nesac\'\nt $\'h() { nosuch6; }; h\'\nt $\'k()\\n{\\n  echo one\\n  echo "${z:?unset3}"\\n}\\nk\'\nt $\'(\\n  nosuch7\\n)\'\nt $\'m() {\\n  for j in 1; do\\n    nosuch8\\n  done\\n}\\nm\'\nt $\'readonly r=1\\nn() {\\n  r=2\\n  declare -i q=1/0\\n}\\nn\'\nt $\'p() {\\n  echo a |\\n    nosuch9\\n}\\np\'\nt $\'if nosuch10\\nthen :\\nfi\'\nt $\'while nosuch11\\ndo :\\ndone\'\nt $\'for i in $(nosuch12)\\ndo :\\ndone\'\nt $\'f() {\\n  local x=$(nosuch13)\\n}\\nf\'\nt $\'echo a \\\\\\n  b; nosuch14 \\\\\\n  arg\'\nt $\'x=$(\\n  nosuch15\\n)\'\nt $\'f() {\\n  return 1\\n}\\nset -e\\nf\\necho after\'\nt $\'trap \\\'echo "trap $LINENO"; nosuch16\\\' EXIT\\ntrue\\n\\ntrue\'\nt $\'f() { echo "L $LINENO"; }\\n\\nf\\ng() {\\n\\n  echo "G $LINENO"\\n}\\ng\'\nt $\'if true; then\\n  echo "I $LINENO"\\nfi; echo "after $LINENO"\'\nt $\'f() {\\n  cat <<E\\n$(nosuch17)\\nE\\n}\\nf\'\nt $\'select v in a; do\\n  nosuch19\\n  break\\ndone <<< 1\'\nt $\'until nosuch20\\ndo break; done\'\nt $\'f() {\\n  nosuch21 2>&1 |\\n  cat\\n}\\nf\'\nt $\'{ nosuch22\\n} 2>&1\'\nt $\'f() {\\n  :\\n} >/nonexistent/x\\nf\'\nt $\'f() {\\n  :\\n}\\nf >/nonexistent/y\'\nt $\'while read -r l; do\\n  nosuch23\\ndone <<< x\'\nt $\'f() {\\n  echo $(( 2 / 0 ))\\n  echo after\\n}\\nf\'\nt $\'f() {\\n  x=$(( 2 / 0 ))\\n}\\nf\'\nt $\'f() {\\n  echo ${a[1/0]}\\n}\\nf\'\nt $\'f() {\\n  echo "$(\\n    nosuch24\\n  )"\\n}\\nf\'\nt $\'y=`\\nnosuch25\\n`\'\nt $\'y=$(nosuch26\\n)\'\nt $\'y=$(\\n\\n  nosuch27\\n\\n)\'\nt $\'echo "$(\\n  nosuch28\\n)" "$(\\n nosuch29\\n)"\'\nt $\'f() {\\n  :\\n}  2>/nonexistent/z\\n\\nf\'\nt $\'f()\\n{\\n  :\\n} >/nonexistent/w\\nf\'\nt $\'f() { :; } >/nonexistent/v\\n\\n\\nf\'\nt $\'cat <(\\n  nosuch30\\n)\'\nt $\'(( 1 +\\n  1 / 0 ))\'\nt $\'echo $(( 1 +\\n  1 / 0 ))\'\nt $\'if (( 1 / 0 )); then :; fi\'\nt $\'for (( i = 0;\\n  i < 1 / 0; i++ )); do :; done\'\nt $\'case $((1/0)) in *) ;; esac\'\nt $\'x=$(\\n  true\\n  nosuchA\\n)\'\nt $\'x=$(true; nosuchB)\'\nt $\'x=$(true\\n\\n\\nnosuchC)\'\nt $\'x=$(if true; then\\n nosuchD\\nfi)\'\nt $\'x=$(# comment\\n  nosuchF\\n)\'\nt $\'x=$(echo "a\\nb"; nosuchK)\'\nt $\'x=$(echo $LINENO\\n  echo $LINENO)\\necho "$x"\'\nt $\'echo x \\\\\\n$(nosuchL)\'\nt $\'echo x\\\\\\n$(nosuchM)\'\nt $\'>/dev/null \\\\\\n  $(nosuchN)\'\nt $\'a=1 \\\\\\n  b=$(nosuchO) \\\\\\n  true\'\nt $\'echo $(\\n  echo $LINENO\\n)\'\nt $\'x=$(\\n  echo $LINENO\\n); echo $x\'\nt $\'x=`\\necho $LINENO\\n`; echo $x\'\nt $\'for i in 1; do\\n  x=$(\\n    nosuchP\\n  )\\ndone\'\nt $\'f() {\\n  x=$(\\n    nosuchQ\\n  )\\n}\\nf\'\nt $\'echo $((\\n  1/0 ))\'\nt $\'x=$((\\n  1/0\\n))\'\nt $\'{\\n  :\\n} >/nonexistent/a\'\nt $\'(\\n  :\\n) >/nonexistent/b\'\nt $\'while false\\ndo\\n  :\\ndone >/nonexistent/c\'\nt $\'for i in 1\\ndo\\n  :\\ndone >/nonexistent/d\'\nt $\'if true\\nthen\\n  :\\nfi >/nonexistent/e\'\nt $\'case x in\\n  x) :\\nesac >/nonexistent/f\'\nt $\'((\\n 1 )) >/nonexistent/g\'\nt $\'[[ 1 &&\\n 1 ]] >/nonexistent/h\'\nt $\'echo $LINENO \\\\\\n  $LINENO\'\nt $\'a="x\\ny" echo $LINENO\'\nt $\'a="x\\ny" b=$LINENO; echo $b\'\nt $\'"ec"\\\'ho\\\' $LINENO "a\\nb" $LINENO\'\nt $\'x=1 \\\\\\n y=$LINENO; echo $y\'\nt $\'>/dev/null \\\\\\n echo $LINENO >&2\'\nt $\'f() {\\n  :\\n}\\n\\nf; echo "f $LINENO"\'\nt $\'{ echo "g $LINENO"\\n}\'\nt $\'if\\n  nosuchR\\nthen :; fi\'\nt $\'nosuchS \\\\\\n  a\'\nt $\'x="a\\nb" nosuchT\'\nt $\'x="a\\nb"\\\\\\n nosuchU\'\nt $\'declare -i n\\nn="1\\n+\\n1/0"\'\nt $\'echo "$((\\n 1/0 ))" $LINENO\'\nt $\'f() { echo "${x?\\n}"; }\\nf\'\n',
    ),
    # Found with the SHLVL work: text read while a function runs (eval, a sourced file, a trap,
    # a bash -c child) expands the aliases in effect then, not the function's.
    (
        "runtime: aliases in text read while a function runs",
        'g() { shopt -s expand_aliases; eval "alias k=\\"echo k\\"\nk"; }; g; t() { bash -c "shopt -s expand_aliases\nalias e=\\"echo hi\\"\ne"; }; t; shopt -s expand_aliases; alias a1="echo defined-before"; h() { a1; eval a1; alias a1="echo redefined"; eval a1; a1; }; h\nprintf "alias s1=\\"echo sourced\\"\\ns1\\n" > /tmp/s.sh; m() { . /tmp/s.sh; }; m; trapf() { trap "alias tp=\\"echo trap\\"; tp" RETURN; }; trapf\nunalias -a; alias b1="echo body"; n() { b1; eval b1; }; unalias b1; n; echo done',
    ),
    # Found with the SHLVL work: with execfail, a failed exec leaves the shell (not a subshell)
    # running; a path it cannot run is named without "exec:".
    (
        "runtime: execfail keeps the shell after a failed exec",
        'shopt -s execfail; exec /nonexistent; echo "st=$?"; exec nosuch; echo "st=$?"; exec /tmp; echo "st=$?"\n( exec nosuch2; echo "sub continued" ); echo "sub st=$?"; f() { exec nosuch3; echo "fn continued $?"; }; f\nx=$(exec nosuch4; echo cont); echo "x=[$x]"; exec nosuch5 2>/dev/null; echo "st=$?"\nbash -c "shopt -s execfail; exec nosuch7; echo child continued"; exec nosuch8 | cat; echo "pipe ${PIPESTATUS[*]}"\n{ exec nosuch9; echo group continued; } &  wait $!; echo "bg st=$?"\nshopt -u execfail; exec nosuch6; echo never',
    ),
    # brush-parse item: blanks inside an arithmetic subscript, in every arithmetic form.
    (
        "runtime: blanks inside arithmetic subscripts",
        '(( a[ 1 ] = 2 )); echo "st=$? ${a[*]}"; (( b[1 + 1] = 3 )); declare -p b; echo $(( c[ 2 ] = 4 )); declare -p c\n(( a[ 1 ] += 5 )); echo "${a[1]}"; x=$(( a[ 1 ] * 2 )); echo $x; let "d[ 3 ] = 1"; declare -p d\n(( e[ i = 2 ] = 7 )); declare -p e i; declare -A m; (( m[ k ] = 1 )); declare -p m; (( a[ 1 ]++ )); echo ${a[1]}\n((a[ 1 ]=2)); (( a[ 1 ] )); echo "st=$? $(( a[ 1 ] )) $((a[ 1 ]+1))"; [[ -v "a[ 1 ]" ]] && echo set\nfor (( a[ 0 ] = 0; a[ 0 ] < 2; a[ 0 ]++ )); do echo "i=${a[0]}"; done; (( a[\t1\t] = 9 )); echo ${a[1]}\nunset "a[ 1 ]"; declare -p a; echo ${a[ 0 ]}',
    ),
    # declare -r 'a[i]=v' makes the array readonly before it assigns the
    # element, which then fails; the declaration still succeeds, as in bash.
    (
        "runtime: declare -r of an array element",
        'declare -r "c[1+1]=v"; echo "st=$?"; declare -p c; declare -r d[3]=w; echo "st=$?"; declare -p d\nf() { local -r "e[1]=x"; echo "st=$?"; declare -p e; }; f; declare -ra g=([2]=z); declare -p g\na=(1 2); declare -r "a[5]=x"; echo "st=$?"; declare -p a; declare -A m=([k]=1); declare -r "m[j]=2"; declare -p m\ntypeset -r "t[0]=1"; declare -p t; declare -ri "n[0]=1+1"; declare -p n; declare -r x[1]; declare -p x',
    ),
    # SUBSCRIPT ERRORS leftover: declare assigns the elements before a key that counts back past
    # the start, then fails, as a plain assignment does.
    (
        "runtime: declare assigns the elements before a bad negative key",
        'declare -a a=(x y [-5]=z w)\necho "st=$?"; declare -p a\nb=(x y [-5]=z w)\necho "st=$?"; declare -p b\nf() { local -a c=(p [-3]=q r); echo "in"; }\nf\necho "st=$?"; declare -p c\ndeclare -a d=(1 2); declare -a d+=(3 [-9]=4 5)\ndeclare -p d\ng() { local -a e=(p [-3]=q r); echo in; }; g; echo "g st=$?"\ndeclare -a h=([-1]=x)\ndeclare -p h\ndeclare -ai n=(1+1 [-4]=2)\ndeclare -p n\n',
    ),
    # The printer: bash keeps a $( ) or <( ) as its command printed back from
    # the parse (print_comsub), which is what it runs (so its lines are numbered that way), what
    # declare -f shows and what BASH_COMMAND holds; `for v in;` loops over nothing.
    (
        "runtime: substitutions run and print as bash prints them back",
        'f() { a=$(\n  true\n  nosuchA\n); b=$(true; nosuchB); c=$(true\n\n\nnosuchC); d=$(if true; then\n nosuchD\nfi); e=$(true; if true; then nosuchE; fi); g=$(# comment\n  nosuchF\n); h=$(true &&\n  nosuchG); i=$(for i in 1; do\n  true\n  nosuchI\ndone); j=$(cat <<E\nhi\nE\nnosuchJ); k=$(echo "a\nb"; nosuchK); l=$(if true; then a; b\nc; fi); m=`\nx\ny`; n="$(x\ny)" o=$(g() {\n  x\n  y\n}); p=$(case q in q) r;; s|t) u;& esac); w=$( (a; b) ); z=$(a | b &&\nc || d); cat <(e1\nf1); echo $(( 1 +\n2 )); }\ndeclare -f f\nf 2>&1 | sed \'s/^/  /\'\nset -- a b; for v in; do echo "in-empty $v"; done; for v; do echo "no-in $v"; done\nk() { for v in; do :; done; for v; do :; done; select v in; do :; done; }; declare -f k\ntrap \'echo "[$BASH_COMMAND]"\' DEBUG; x=$(echo 1\necho 2); trap - DEBUG\necho $(echo $LINENO\necho $LINENO) "$(\n  echo $LINENO)"\ncat <(echo "ps $LINENO"\necho "ps $LINENO")\n',
    ),
    # Printer item 2: a circular local nameref warns as often as bash looks it up, operator by
    # operator and builtin by builtin; ${!ref} of a nameref is the name it refers to; `local r=v`
    # for a local nameref declares the variable it refers to.
    (
        "runtime: circular nameref warnings and nameref declarations",
        'v=gv; declare -a a=(x y)\nt() { echo "== $1"; eval "f() { local -n v=v; $1; }"; f 2>&1 | grep -v "local: warning"; }\nt \'echo "${v^^}"\'\nt \'echo "${v,,}"\'\nt \'echo "${v@Q}"\'\nt \'echo "${v@U}"\'\nt \'echo "${v%g}"\'\nt \'echo "${v#g}"\'\nt \'echo "${v/g/h}"\'\nt \'echo "${v:1}"\'\nt \'echo "${v:1:1}"\'\nt \'echo "${#v}"\'\nt \'echo "${v-x}"\'\nt \'echo "${v:=x}"\'\nt \'echo "${v:?x}"\'\nt \'echo "${!v}"\'\nt \'echo "${v[@]}"\'\nt \'echo "${v[0]}"\'\nt \'echo "${#v[@]}"\'\nt \'echo "${!v[@]}"\'\nt \'read v <<< rd; echo "$v"\'\nt \'read -a v <<< "r1 r2"; echo "${v[*]}"\'\nt \'printf -v v %s pf; echo "$v"\'\nt \'(( v = 5 )); echo "$v"\'\nt \'(( v++ )); echo "$v"\'\nt \'echo $(( v + 1 ))\'\nt \'let v=7; echo "$v"\'\nt \'v+=app; echo "$v"\'\nt \'mapfile -t v <<< "m1"; echo "${v[*]}"\'\nt \'unset v; echo "${v-unset}"\'\nt \'[[ -v v ]] && echo set\'\nt \'[[ $v == gv ]] && echo eq\'\nt \'declare -p v\'\nt \'export v; echo ok\'\nt \'v[1]=el; echo "${v[*]}"\'\nt \'for v in a b; do :; done; echo "$v"\'\nt \'getopts a: v -a; echo "$v"\'\nt \'test -v v && echo set2\'\nt \'(( v ))\'\nt \'(( v += 1 ))\'\nt \'(( v = v + 1 ))\'\nt \'(( ++v ))\'\nt \'(( v-- ))\'\nt \'(( v *= 2 ))\'\nt \'echo $(( v ))\'\nt \'echo $(( v + v ))\'\nt \'echo $(( v = 4 ))\'\nt \'(( v[1] = 3 ))\'\nt \'echo $(( v[0] ))\'\nt \'let v++\'\nt \'let "v = 2"\'\nt \'x=$v\'\nt \'echo "$v"\'\nt \'echo $v\'\nt \'echo "${v}"\'\nt \'echo "${v[*]}"\'\nt \'echo "${v[@]:0}"\'\nt \'echo "${v@A}"\'\nt \'echo "${v@a}"\'\nt \'echo "${v@P}"\'\nt \'echo "${v@E}"\'\nt \'echo "${v@K}"\'\nt \'echo "${v:-d}"\'\nt \'echo "${v+a}"\'\nt \'echo "${v:+a}"\'\nt \'echo "${v=d}"\'\nt \'echo "${v?e}"\'\nt \'echo "${v^}"\'\nt \'echo "${v~}"\'\nt \'echo "${v##3}"\'\nt \'echo "${v%%3}"\'\nt \'echo "${v//3/4}"\'\nt \'echo "${v/#3/4}"\'\nt \'echo "${#v}"\'\nt \'read -r v <<< a\'\nt \'read -r -N 1 v <<< ab\'\nt \'read -r a v <<< "x y"\'\nt \'printf -v v %s x\'\nt \'getopts a v -a\'\nt \'for v in a; do :; done\'\nt \'for v in a b c; do :; done\'\nt \'select v in a; do break; done <<< 1\'\nt \'unset v\'\nt \'unset -v v\'\nt \'unset -n v\'\nt \'[[ -v v ]]\'\nt \'test -v v\'\nt \'[ -v v ]\'\nt \'export v\'\nt \'readonly v\'\nt \'declare -x v\'\nt \'mapfile v <<< a\'\nt \'read -a v <<< a\'\nt \'v=(1 2)\'\nt \'v+=(3)\'\nt \'v[2]=x\'\nt \': ${v:=d}\'\nt \'local v=2\'\nx=X; g() { local -n r=x; echo "[${!r}]"; local r=2; echo "in $x [${!r}]"; local -i r=3+4; declare -p x; }; g; echo "out $x"\ndeclare -n n=x; echo "[${!n}]"; h() { local -n v=v; echo "[${!v}]"; echo never; }; h 2>&1; echo "st=$?"\n',
    ),
    # Printer item 3: set -v echoes a here-document in a $( ) as bash reads it: once as it
    # parses the substitution (twice in double quotes), and again each time it runs, from the
    # command printed back (tabs stripped for <<-).
    (
        "runtime: set -v echoes here-documents in substitutions as bash does",
        'set -v\nx=$(cat <<EOF\nbody1\nbody2\nEOF\n)\necho "$x"\ny=$(echo a\necho b)\nz=$(cat <<EOF; echo c\nb3\nEOF\n)\nw=$(\ncat <<EOF\nb4\nEOF\necho d)\ne=$(cat <<EOF\nEOF\n)\ntwo=$(cat <<A; cat <<-B\n1\nA\n\t2\n\tB\n)\nseq=$(cat <<A\n3\nA\ncat <<\'B\'\n4\nB\n)\nq="$(cat <<X\nq1\nX\n)"\nf() {\n  v=$(cat <<E\nfbody\nE\n)\n}\necho end\n',
    ),
    (
        "runtime: a syntax error in a prompt's substitution is reported as bash reports it",
        't() { x=$1; printf \'[%s] %s\\n\' "${x@P}" $?; }\nt \'$(fi)\'\nt \'a$(fi)b\'\nt \'$(if)\'\nt \'$(done)\'\nt \'$(then x)\'\nt \'$(;)\'\nt \'$(|)\'\nt \'$(echo (a)\'\nt \'p$(echo ok)q$(fi)r\'\nt \'$(fi) $(echo z)\'\nt \'$(echo "a)\'\nt \'$(echo hi\'\nt \'$(echo hi |\'\nt \'$(if x; then\'\nt \'`then`x\'\nt \'$(echo a; ))\'\nt \'$(for i in 1 2; do echo $i; done)$(esac)\'\nx=\'$(fi)\'\necho "${x@P}"\nPS4=\'+$(fi) \'\nset -x\necho traced\nset +x\necho done\n#--call-- /\nx=\'$(fi\'; echo "[${x@P}]"\n#--call-- /\nx=\'$(case)\'; echo "[${x@P}]"\n#--call-- /\nx=\'$(a &&)\'; echo "[${x@P}]"\n',
    ),
    (
        "runtime: a syntax error names the token bash's grammar rejects",
        'for s in \'fi)\' \'then x\' \'then\' \'echo hi;fi)\' \'echo (a)\' \'echo (a\' \'fi) $(echo z\' \'{ a; })\' \'if a; then b; fi)\' \'fi "a\' \'while a; do if b; then c; fi; done )\' \'f() { :; }; done\' \'case x in a) :;; esac; esac\' \'for i in a; do :; done; fi\'; do eval "$s"; echo "s=$?"; done\nbash -c \'echo hi; then x\'; echo "s=$?"\n',
    ),
    (
        "runtime: a prompt that cannot be expanded is used as it reads",
        'x=\'a$(( 1 +))b\\$\'; echo "[${x@P}] st=$?"\nx=\'$(echo q >&2)a$(( 1 +))b$(fi)\'; echo "[${x@P}] st=$?"\nx=\'a${nope:?oops}b\'; echo "[${x@P}] st=$?"\necho after1\nx=\'a${x/[}b${!}c\'; echo "[${x@P}] st=$?"\nx=\'a$((1/0))b\'; y="${x@P}"; echo "[$y] st=$?"\nPS4=\'+$(( 1 +))\\$ \'; set -x; echo hi; set +x\necho after2\n#--call-- /\nset -u; x=\'a${nope}b\'; echo "[${x@P}] st=$?"\necho after3\n#--call-- /\nset -e; x=\'a$(( 1 +))b\'; echo "[${x@P}] st=$?"\necho after4\n',
    ),
    (
        "runtime: a pipeline stage's own command leaves $_ as bash does",
        ': pre1; trap \'echo "1[$_]"\' EXIT | cat\n: pre2; ( trap \'echo "2[$_]"\' EXIT; : last2 )\n: pre3; ( trap \'echo "3[$_]"\' EXIT; : last3 ) | cat\n: pre4; { trap \'echo "4[$_]"\' EXIT; : last4; } | cat\n: pre5; x=$(trap \'echo "5[$_]" >&2\' EXIT; : last5)\n: pre6; f() { trap \'echo "6[$_]"\' EXIT; : last6; }; f | cat\n: pre7; ( trap \'echo "7[$_]"\' EXIT; echo last7 >/dev/null )\n: pre8; ( trap \'echo "8[$_]"\' EXIT; true last8 )\n: pre9; ( trap \'echo "9[$_]"\' EXIT; : last9; : ) \n: pre10; (trap \'echo "10[$_]"\' EXIT; : a | : last10)\n: pre11; echo x | { trap \'echo "11[$_]"\' EXIT; read -r v last11; }\n: pre12; ( : last12 ) ; echo "12[$_]"\n: pre13; : last13 | cat; echo "13[$_]"\n#--call-- /\ntrap \'echo "top[$_]"\' EXIT; : last\n',
    ),
]

REFUSED_PROMPT = (
    "refused: bash-tool checks the text a prompt string (PS4, ${x@P}) or compgen -C runs, as it "
    "checks eval's: a refused construct is reported and does not run (README, Commands)"
)
BASH_SEGFAULTS = "bash segfaults (status 139): a prompt that expands itself recurses until bash's stack overflows"
NESTING = b"bash: line 1: maximum nesting level exceeded: deeper nesting is unsupported in bash-tool\n"
TOO_DEEP = b"bash: line 1: arithmetic expression nesting level exceeded (10000): deeper nesting is unsupported in bash-tool\n"
EXPECTED = {
    "runtime: a subscript nested past the limit fails on every path": (
        0,
        b"s=1\n" * 9 + b"declare -a a=()\n",
        TOO_DEEP.replace(b"line 1", b"line 2") * 9,
        "bash evaluates a subscript nested 10,000 deep (and assigns a[1]); bash-tool refuses arithmetic nested more "
        "than 100 deep, loudly, on every subscript path (README, Limits)",
    ),
    "runtime: arithmetic nested past the limit fails without trapping": (
        0,
        b"s=1\ns=1\ns=1\n",
        TOO_DEEP
        + TOO_DEEP.replace(b"line 1", b"line 2").replace(b"bash: line 2: ", b"bash: line 2: ((: ")
        + TOO_DEEP.replace(b"line 1", b"line 2").replace(b"bash: line 2: ", b"bash: line 2: let: "),
        "bash nests arithmetic without a fixed limit (over 10,000 levels); bash-tool refuses more than 100, "
        "which its stack holds at the shell's deepest nesting (README, Limits)",
    ),
    "runtime: a prompt that expands itself stops at the nesting limit": (
        1, b"", NESTING, BASH_SEGFAULTS,
    ),
    "runtime: a subshell whose prompt expands itself ends with an error": (
        0, b"sub s=1\n", NESTING, BASH_SEGFAULTS,
    ),
    "runtime: a PS4 that expands itself is reported and traced as written": (
        0, b"hi\ndone\n", NESTING + b"${PS4@P}echo hi\n" + NESTING + b"${PS4@P}set +x\n", BASH_SEGFAULTS,
    ),
    "runtime: a refused construct in ${x@P} is refused when it expands": (
        0, b"s=2 []\n", b"bash: background coprocesses are unsupported in bash-tool\n", REFUSED_PROMPT,
    ),
    "runtime: a refused command named in ${x@P} refuses the expansion": (
        0, b"s=2\n", b"bash: umask is unsupported in bash-tool\n", REFUSED_PROMPT,
    ),
    "runtime: a refused command reached through ${x@P} stops where it is reached": (
        0, b"s=0 []\n", b"bash: umask is unsupported in bash-tool\n", REFUSED_PROMPT,
    ),
    "runtime: a refused PS4 is reported and traced as written": (
        0,
        b"hi\ns=0\n",
        b"bash: background coprocesses are unsupported in bash-tool\n$(coproc true)+ echo hi\n"
        b"bash: background coprocesses are unsupported in bash-tool\n$(coproc true)+ set +x\n",
        REFUSED_PROMPT,
    ),
    "runtime: code nested too deeply in ${x@P} is refused": (
        0, b"s=2\n", b"bash: shell code is nested too deeply for bash-tool\n", REFUSED_PROMPT,
    ),
    "runtime: compgen -C checks the command it runs": (
        0,
        b"s=2\ns=2\n",
        b"bash: background coprocesses are unsupported in bash-tool\n"
        b"bash: background coprocesses are unsupported in bash-tool\n",
        REFUSED_PROMPT,
    ),
}
