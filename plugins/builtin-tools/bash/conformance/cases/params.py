"""Parameters: positionals, specials, arrays, namerefs and attributes."""

CASES = [
    ("param: readonly assignment abandons its line", 'readonly r=1\nr=2; echo same-line\necho next=$?\nf() { r=3; echo in-f; }\nf; echo after-f\necho last', ["param.readonly"]),
    ("param: nameref as a for variable", 'target=""; declare -n ref=target; for ref in a b c; do echo "loop: target=$target"; done; echo "final: target=$target"', ["param.nameref"]),
    ("param: exported nameref reaches a child as its name", "target=value; declare -nx myref=target; bash -c 'echo \"myref: $myref\"'", ["param.nameref"]),
    ("param: nameref to an element in arithmetic", "arr=(0 1 0); declare -n ref='arr[1]'; echo $(( ref ? 100 : 200 ))", ["param.nameref"]),
    ("param: positional parameters", 'set -- a "b c" d; echo $# "$1" "$2"; echo "$@"; echo "${10-none}"; set -- 1 2 3 4 5 6 7 8 9 10 11; echo ${10} ${11} $10'),
    ("param: dollar zero and dash", 'echo $- | grep -q h && echo hashall; case $- in *c*) echo command-string;; esac; set -u; case $- in *u*) echo nounset;; esac'),
    ("param: last argument", 'echo a b c >/dev/null; echo $_; true last; echo $_'),
    ("param: status and pid", 'false; echo $?; echo $? ; [ $$ -gt 0 ] && echo pid; sleep 0.01 & [ $! -gt 0 ] && echo bgpid; wait'),
    ("param: indexed arrays", 'a=(zero one two); echo ${a[0]} ${a[@]} ${#a[@]}; a[5]=five; echo ${#a[@]} ${!a[@]}; a+=(six); echo ${a[6]}; echo ${a[-1]} ${a[@]: -2}'),
    ("param: array assignment forms", 'a=([2]=two [0]=zero); echo ${!a[@]}; b=("${a[@]}"); echo ${#b[@]}; c=(); echo ${#c[@]}; d[3]=x; echo ${d[@]}'),
    ("param: array quoting", 'a=("x y" z); for e in "${a[@]}"; do echo "[$e]"; done; for e in ${a[@]}; do echo "<$e>"; done'),
    ("param: array element operations", 'a=(apple banana cherry); echo ${a[1]:0:3} ${a[@]^} ${#a[2]}; unset a; echo ${#a[@]}'),
    ("param: negative index errors", 'a=(1 2); echo ${a[-3]}; echo status=$?; a[-3]=x; echo status=$?', ["error"]),
    ("param: associative arrays", 'declare -A m; m[one]=1; m[two]=2; m["with space"]=3; echo ${m[one]} ${#m[@]} "${m[with space]}"; for k in "${!m[@]}"; do echo "$k=${m[$k]}"; done | sort'),
    ("param: associative literal and unset", 'declare -A m=([a]=1 [b]=2 [c]=3); unset "m[b]"; echo ${#m[@]}; [[ -v m[a] ]] && echo has-a; [[ -v m[b] ]] || echo no-b; printf "%s\\n" "${m[@]}" | sort'),
    ("param: associative arrays need declare", 'm[k]=v; echo ${m[k]} ${m[0]}; declare -p m'),
    ("param: namerefs", 'declare -n r=target; r=value; echo $target; target=changed; echo $r; f() { local -n out=$1; out=result; }; f dest; echo $dest', ["param.nameref"]),
    ("param: namerefs to arrays", 'arr=(1 2 3); declare -n ref=arr; echo ${ref[1]} ${#ref[@]}; ref+=(4); echo ${arr[@]}', ["param.nameref"]),
    ("param: nameref loops", 'a=1 b=2; for n in a b; do declare -n v=$n; echo $v; done; declare +n v; echo done', ["param.nameref"]),
    ("param: integer attribute", 'declare -i i=10; i+=5; echo $i; i="i*2"; echo $i; declare +i i; i=3+3; echo $i'),
    ("param: export reaches children only when exported", 'x=plain; export y=exported; bash -c \'echo "[${x-}] [${y-}]"\'; z=inline bash -c \'echo "[${z-}]"\'', ["param.export"]),
    ("param: allexport", 'set -a; w=auto; set +a; bash -c \'echo "[${w-}]"\'', ["option.allexport", "param.export"]),
    ("param: export -n and -p", 'export e=1; export -n e; bash -c \'echo "[${e-}]"\'; export -p | grep -c " e=" ; echo status=$?', ["param.export"]),
    ("param: readonly and declare -r in functions", 'f() { local -r c=1; c=2; echo status=$?; }; f 2>/dev/null; readonly -f f 2>/dev/null; echo done', ["param.readonly"]),
    ("param: RANDOM and SECONDS", '[[ $RANDOM =~ ^[0-9]+$ ]] && (( RANDOM < 32768 )) && echo random; (( SECONDS >= 0 )) && echo seconds; [[ $EPOCHSECONDS =~ ^[0-9]+$ ]] && echo epoch', ["param.random"]),
    ("param: BASH_SOURCE and LINENO in a script", 'echo "[${BASH_SOURCE[0]-}]"; echo $LINENO\necho $LINENO', ["param.callstack"]),
    ("param: unset variables in nounset", 'set -u; echo "${x-default}"; echo "${arr[@]-}"; ( echo $undefined ) 2>/dev/null; echo status=$?', ["option.nounset"]),
    ("param: special parameters in functions", 'f() { echo "$0|$#|$*"; }; f a b; set -- x; f; echo $#'),
    ("param: assignment before a command", 'x=outer; x=inner true; echo $x; f() { echo $x; }; x=temp f; echo $x'),
    ("param: variable names", 'a_1=1 _b=2; echo $a_1 $_b; 1x=3 2>/dev/null; echo status=$?'),
    (
        "param: a nameref to an array element reads and assigns it",
        "a=(1 2 3); declare -n r='a[1]'; echo \"read=[$r]\"; r=x; echo \"${a[*]}\"; r+=z; echo \"${a[*]}\"; "
        "(( r = 7 )); echo \"${a[*]}\"; (( r += 5 )); echo \"${a[*]}\"; "
        "declare -A m=([k]=v); declare -n mr='m[k]'; echo \"[$mr]\"; mr=w; echo \"${m[k]}\"; "
        "i=2; declare -n ri='a[$i]'; ri=last; echo \"${a[*]}\"",
    ),
    (
        "param: a nameref to an element in a function and an arithmetic for loop",
        "f() { local -n e=$1; e=changed; }; b=(1 2 3); f 'b[1]'; echo \"${b[*]}\"; "
        "arr=(0); declare -n ref='arr[0]'; for (( ref=1; ref<=3; ref++ )); do echo \"ref=$ref arr0=${arr[0]}\"; done",
    ),
    ("param: read into an array element", "read -r 'c[3]' <<< val; echo \"[${c[3]}] ${#c[@]}\""),
]
