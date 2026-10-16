"""Grammar sweep: indexed and associative arrays and namerefs, generated from fixed tables of
assignments and accesses (see sweep_grammar_param.py for the scheme)."""
import itertools

TIER = "sweep"

CASES = []
_NAMES = []


def _add(name, script, tags=()):
    name = "gram array " + name
    if name in _NAMES:
        raise ValueError("duplicate case name " + name)
    _NAMES.append(name)
    CASES.append((name, script, list(tags)))


P = "p() { printf '%s' \"$#\"; printf ' <%s>' \"$@\"; echo; }; "

# --- Indexed arrays: assignment forms × accesses ----------------------------------------------

ASSIGNS = [
    ("list", "a=(x 'y z' '')"),
    ("explicit indices", "a=([3]=c [1]=a)"),
    ("mixed indices", "a=(x [5]=y z)"),
    ("append list", "a=(x); a+=(y 'z w')"),
    ("append to element", "a=(x y); a[1]+=suffix"),
    ("element beyond end", "a=(x); a[10]=far"),
    ("negative index assignment", "a=(x y z); a[-1]=last"),
    ("arithmetic index", "i=1; a=(x y z); a[i+1]=two"),
    ("octal index", "a[010]=eight"),
    ("hex index", "a[0x10]=sixteen"),
    ("quoted arithmetic index", "a['1+1']=q"),
    ("name index", "k=2; a[k]=byname"),
    ("unset name index", "unset k; a[k]=zero"),
    ("split assignment", "v='p  q r'; a=($v)"),
    ("glob assignment", "cd /tmp; : > f1; : > f2; a=(f*)"),
    ("brace assignment", "a=({1..3} {x,y})"),
    ("command substitution assignment", "a=($(printf 'l1\\nl 2\\n'))"),
    ("empty", "a=()"),
    ("scalar becomes array", "a=scalar; a[1]=one"),
    ("declare -a", "declare -a a=(d1 d2)"),
    ("copy", "b=(x 'y z'); a=(\"${b[@]}\")"),
    ("sparse after unset", "a=(x y z); unset 'a[1]'"),
    ("whole unset then element", "a=(x y); unset a; a[2]=new"),
    ("element with spaces in index", "a[ 1 ]=spaced"),
    ("assignment with += in list", "a=(x); a+=([5]=y)"),
    ("read -a", "read -ra a <<< 'r1 r2'"),
    ("assignment of a quoted list", "a=('(x)' '[1]=y')"),
    ("ifs during assignment", "IFS=,; v='c1,c2'; a=($v)"),
]
ACCESSES = [
    ("count", "p \"${#a[@]}\""),
    ("keys", "p \"${!a[@]}\""),
    ("values quoted", "p \"${a[@]}\""),
    ("values unquoted", "p ${a[@]}"),
    ("star quoted", "p \"${a[*]}\""),
    ("bare name", "p \"$a\" \"${a}\""),
    ("element one", "p \"${a[1]}\""),
    ("last element", "p \"${a[-1]}\""),
    ("slice", "p \"${a[@]:1:2}\""),
    ("declare -p", "declare -p a"),
    ("suffix literal", "p \"$a[1]\""),
    ("length of first", "p \"${#a}\" \"${#a[0]}\""),
]
for (aname, assign), (xname, access) in itertools.product(ASSIGNS, ACCESSES):
    if xname not in ("declare -p", "values quoted") and (ASSIGNS.index((aname, assign)) + ACCESSES.index((xname, access))) % 4:
        continue
    _add("indexed " + aname + ": " + xname, P + assign + "; " + access, ["param.array.indexed"])

for label, script in [
    ("negative index beyond start read", "a=(x y); echo \"[${a[-3]}]\"; echo status=$?"),
    ("negative index beyond start write", "a=(x y); a[-3]=z; echo status=$?; declare -p a"),
    ("negative index of sparse", "a=([2]=x [7]=y); echo \"${a[-1]} [${a[-2]}]\""),
    ("unset negative index", "a=(x y z); unset 'a[-1]'; declare -p a"),
    ("unset element unquoted", "cd /tmp; a=(x y); unset a[0]; declare -p a"),
    ("unset element with a matching file", "cd /tmp; : > a0; a=(x y); unset a[0]; declare -p a"),
    ("unset at subscript", "a=(x y); unset 'a[@]'; echo \"${#a[@]} ${a-unset}\""),
    ("unset star subscript", "a=(x y); unset 'a[*]'; declare -p a 2>&1"),
    ("unset whole", "a=(x y); unset a; declare -p a; echo status=$?"),
    ("assign at subscript", "a=(x); a[@]=y; echo status=$?"),
    ("bad subscript expression", "a=(x); a[1+]=y; echo status=$?"),
    ("empty subscript", "a=(x); a[]=y; echo status=$?; echo ${a[]}; echo status=$?"),
    ("subscript with a string that names a variable", "k=3; j=k; a[j]=v; echo ${!a[@]}"),
    ("subscript with side effects", "i=0; a[i++]=x; a[i++]=y; echo ${a[@]} $i"),
    ("nested subscript", "a=(1 2 3); echo ${a[a[0]]} ${a[a[a[0]]]}"),
    ("array in arithmetic", "a=(4 5); echo $(( a[0] * a[1] )) $(( a )) $(( a[5] ))"),
    ("increment an element", "a=(1); (( a[0]++ )); (( a[3] += 2 )); declare -p a"),
    ("length of a sparse array", "a=([100]=x); echo ${#a[@]} ${!a[@]}"),
    ("large index", "a[9223372036854775807]=big; echo ${!a[@]}"),
    ("slice of a sparse array", "a=([1]=a [5]=b [9]=c); echo ${a[@]:2} / ${a[@]:6:1} / ${a[@]: -1}"),
    ("slice with negative length", "a=(a b c d); echo ${a[@]:1:-1}; echo status=$?"),
    ("quoted empty elements survive", "a=('' x ''); p() { echo $#; }; p \"${a[@]}\"; p ${a[@]}"),
    ("star with empty IFS", "a=(x y); IFS=; echo \"${a[*]}\"; p() { echo $#; }; p ${a[*]}"),
    ("iteration by index", "a=(x [4]=y z); for i in \"${!a[@]}\"; do echo \"$i=${a[i]}\"; done"),
    ("readonly array element assignment", "a=(x); readonly a; a[1]=y; echo status=$?; declare -p a"),
    ("readonly array append", "readonly -a a=(x); a+=(y); echo status=$?"),
    ("local array in a function", "a=(g); f() { local a=(l1 l2); echo ${a[@]}; }; f; echo ${a[@]}"),
    ("local copy of a global array", "a=(g1 g2); f() { local a=(\"${a[@]}\" l); echo ${a[@]}; }; f"),
    ("array passed by name", "f() { local -n arr=$1; arr+=(added); }; a=(x); f a; echo ${a[@]}"),
    ("array of arrays is not a thing", "a=((1 2) 3); echo status=$?"),
    ("compound assignment in declare with spaces", "declare -a a=( 1  2 ); echo ${#a[@]}"),
    ("array expansion in a for loop with IFS", "a=('x y' z); IFS=; for e in ${a[@]}; do echo \"<$e>\"; done"),
    ("pattern operators on each element", "a=(apple banana cherry); echo ${a[@]#?} ${a[@]%a*} ${a[@]/an/AN} ${a[@]^^}"),
    ("transform on each element", "a=('x y' \"it's\"); echo ${a[@]@Q}; echo ${a[@]@A}"),
    ("default on an empty array", "a=(); echo \"[${a[@]:-empty}]\" \"[${a[*]-unset}]\""),
    ("default on an array with an empty element", "a=(''); echo \"[${a[@]:-empty}]\" \"[${a[@]-unset}]\""),
    ("length of each element is not a thing", "a=(ab c); echo ${#a[@]} ${#a[*]}"),
    ("BASH_REMATCH is an array", "[[ ab =~ (a)(b) ]]; echo ${#BASH_REMATCH[@]} ${BASH_REMATCH[@]:1}"),
    ("PIPESTATUS is an array", "true | false | (exit 3); a=(\"${PIPESTATUS[@]}\"); echo ${a[@]}"),
    ("FUNCNAME is an array", "f() { echo ${#FUNCNAME[@]} ${FUNCNAME[@]}; }; g() { f; }; g"),
    ("array exported is not", "export a=(x); bash -c 'echo \"[${a-unset}]\"'"),
    ("mapfile then modify", "mapfile -t a <<< $'1\\n2'; a[5]=6; declare -p a"),
    ("printf -v into an element", "printf -v 'a[2]' '%s-%s' p q; declare -p a"),
    ("read into an element", "read -r 'a[1]' <<< line; declare -p a"),
    ("unset in a loop over keys", "a=(1 2 3 4); for i in \"${!a[@]}\"; do (( a[i] % 2 )) && unset \"a[$i]\"; done; declare -p a"),
    ("append to an unset array", "unset a; a+=(x); a+=y; declare -p a"),
    ("string append to an array name", "a=(x y); a+=z; declare -p a"),
    ("element assignment keeps attributes", "declare -ai a=(1+1); a[1]=2*3; declare -p a"),
    ("uppercase attribute on elements", "declare -au a=(x); a+=(y); declare -p a"),
]:
    _add("indexed " + label, P + script, ["param.array.indexed"])

# --- Associative arrays --------------------------------------------------------------------------

KEYS = [("plain", "k"), ("with space", "'a b'"), ("empty", "''"), ("star", "'*'"), ("at", "'@'"),
        ("digits", "10"), ("expression", "1+1"), ("quote", "\"it's\""), ("bracket", "']'"),
        ("dollar", "'$x'"), ("variable", "$kv"), ("unicode", "é"), ("negative", "-1"), ("dash", "'-'")]
for kname, key in KEYS:
    _add("assoc key " + kname,
         "declare -A m; kv=var; x=X; m[" + key + "]=val; echo \"status=$?\"; declare -p m; echo \"[${m[" + key + "]}]\"; unset \"m[" + key + "]\"; echo \"after unset ${#m[@]}\"",
         ["param.array.assoc"])
    _add("assoc literal key " + kname,
         "kv=var; x=X; declare -A m=([" + key + "]=lit); echo \"status=$?\"; declare -p m",
         ["param.array.assoc"])
    if len(kname) % 2:
        _add("assoc test key " + kname,
             "kv=var; declare -A m=([" + key + "]=v); [[ -v m[" + key + "] ]] && echo set || echo unset",
             ["param.array.assoc"])

for label, script in [
    ("declare required", "m[a]=1; m[b]=2; declare -p m"),
    ("keys and values", "declare -A m=([one]=1); m[two]=2; echo ${#m[@]}; for k in one two; do echo \"$k=${m[$k]}\"; done"),
    ("key value pair list", "declare -A m=(k1 v1 k2 v2); echo ${m[k1]} ${m[k2]} ${#m[@]}"),
    ("key value pair list odd", "declare -A m=(k1 v1 k2); declare -p m"),
    ("mixed pair styles", "declare -A m=([a]=1 b 2); echo status=$?; declare -p m"),
    ("assignment without declare -A after declare", "declare -A m; m=([x]=1 [y]=2); echo ${m[x]}${m[y]}"),
    ("append element", "declare -A m=([k]=a); m[k]+=b; echo ${m[k]}"),
    ("append list", "declare -A m=([a]=1); m+=([b]=2); echo ${m[a]}${m[b]} ${#m[@]}"),
    ("append bare string", "declare -A m=([a]=1); m+=x; declare -p m"),
    ("bare assignment", "declare -A m; m=x; declare -p m"),
    ("bare expansion", "declare -A m=([0]=zero [k]=v); echo \"[$m]\""),
    ("count and keys sorted by loop", "declare -A m=([c]=3 [a]=1 [b]=2); for k in a b c; do [[ -v m[$k] ]] && echo -n \"$k \"; done; echo ${#m[@]}"),
    ("key order of declare -p", "declare -A m=([one]=1 [two]=2 [three]=3 [four]=4); declare -p m"),
    ("key order of expansion", "declare -A m=([one]=1 [two]=2 [three]=3 [four]=4); echo ${!m[@]}; echo ${m[@]}"),
    ("unset element", "declare -A m=([a]=1 [b]=2); unset 'm[a]'; declare -p m"),
    ("unset whole", "declare -A m=([a]=1); unset m; m[x]=1; declare -p m"),
    ("default on missing key", "declare -A m; echo \"${m[z]:-dflt} ${m[z]-unset}\""),
    ("length of an element", "declare -A m=([k]=hello); echo ${#m[k]}"),
    ("pattern operators", "declare -A m=([k]=hello); echo ${m[k]^^} ${m[k]#h} ${m[k]/l/L}"),
    ("keys with arithmetic look are strings", "declare -A m; m[1+1]=a; m[2]=b; echo \"${m[1+1]}${m[2]}\"; declare -p m"),
    ("subscript expansion", "declare -A m; k='a b'; m[$k]=1; m[\"$k\"]+=2; declare -p m"),
    ("subscript with command substitution", "declare -A m; m[$(echo key)]=v; declare -p m"),
    ("local assoc in a function", "f() { local -A lm=([k]=v); echo ${lm[k]}; }; f; declare -p lm 2>&1; echo status=$?"),
    ("declare -A in a function is local", "f() { declare -A fm=([k]=v); }; f; echo \"[${fm[k]-unset}]\""),
    ("declare -gA in a function", "f() { declare -gA gm=([k]=v); }; f; echo ${gm[k]}"),
    ("convert indexed to assoc", "a=(1); declare -A a; echo status=$?"),
    ("convert assoc to indexed", "declare -A m; declare -a m; echo status=$?"),
    ("readonly assoc", "declare -rA m=([k]=v); m[k]=w; echo status=$?"),
    ("integer assoc", "declare -Ai m=([k]=1+2); m[j]=3*3; declare -p m"),
    ("lowercase assoc", "declare -Al m=([K]=VAL); declare -p m"),
    ("assoc in arithmetic", "declare -A m=([k]=5); echo $(( m[k] * 2 )); (( m[j] = 7 )); echo ${m[j]}"),
    ("assoc slice", "declare -A m=([a]=1); echo \"${m[@]:0:1}\""),
    ("assoc with nameref", "declare -A m=([k]=v); declare -n r=m; r[j]=w; echo ${m[k]}${m[j]}"),
    ("nameref to an assoc element", "declare -A m=([k]=v); declare -n r='m[k]'; r=changed; echo ${m[k]}"),
    ("assoc key with a closing bracket in a variable", "declare -A m; k='a]b'; m[$k]=1; declare -p m"),
    ("assoc key quoting in declare -p", "declare -A m=([\"a'b\"]='c\"d' ['$x']='`y`'); declare -p m"),
    ("assoc @A transform", "declare -A m=([k]='v w'); echo ${m@A}"),
    ("assoc @K transform", "declare -A m=([k]='v w'); echo ${m[@]@K}"),
    ("assoc keys @k transform", "declare -A m=([k]='v w'); echo ${m[@]@k}"),
    ("assoc in a for loop over keys", "declare -A m=([only]=1); for k in \"${!m[@]}\"; do echo \"$k\"; done"),
    ("assoc exported is not", "declare -Ax m=([k]=v); bash -c 'echo \"[${m-unset}]\"'"),
    ("mapfile into assoc", "declare -A m; mapfile -t m <<< x; echo status=$?"),
    ("read -a into assoc", "declare -A m; read -ra m <<< 'a b'; echo status=$?"),
    ("assoc with an empty key via variable", "declare -A m; e=; m[$e]=v; echo status=$?"),
    ("assoc unset with a glob-looking key", "cd /tmp; : > mk; declare -A m=([mk]=1 ['m*']=2); unset 'm[m*]'; declare -p m"),
]:
    _add("assoc " + label, script, ["param.array.assoc"])

# --- Namerefs --------------------------------------------------------------------------------------

REF_TARGETS = [
    ("scalar", "t=value", "t"),
    ("unset scalar", "unset t", "t"),
    ("array", "t=(a b c)", "t"),
    ("array element", "t=(a b c)", "t[1]"),
    ("assoc", "declare -A t=([k]=v)", "t"),
    ("assoc element", "declare -A t=([k]=v)", "t[k]"),
    ("integer", "declare -i t=5", "t"),
    ("readonly", "readonly t=ro", "t"),
    ("another nameref", "t2=deep; declare -n t=t2", "t"),
    ("positional", "set -- p1", "1"),
    ("invalid name", "", "'a b'"),
    ("itself", "", "r"),
    ("empty", "", "''"),
]
REF_OPS = [
    ("read", "echo \"[${r}]\""),
    ("assign", "r=new; echo \"status=$?\"; declare -p t 2>&1"),
    ("append", "r+=X; declare -p t 2>&1"),
    ("unset", "unset r; declare -p t 2>&1; declare -p r 2>&1"),
    ("unset -n", "unset -n r; declare -p r 2>&1; declare -p t 2>&1"),
    ("name", "echo \"${!r}\""),
    ("length", "echo \"${#r}\" \"${#r[@]}\""),
    ("elements", "echo \"${r[@]}\" \"${!r[@]}\""),
    ("declare -p", "declare -p r"),
    ("arithmetic", "(( r += 1 )); echo \"status=$? ${r}\""),
]
for (tname, setup, target), (oname, op) in itertools.product(REF_TARGETS, REF_OPS):
    if oname not in ("read", "assign", "declare -p") and (REF_TARGETS.index((tname, setup, target)) + REF_OPS.index((oname, op))) % 3:
        continue
    _add("nameref " + oname + ": " + tname,
         (setup + "; " if setup else "") + "declare -n r=" + target + "; echo \"decl=$?\"; " + op,
         ["param.nameref"])

for label, script in [
    ("unset nameref assignment sets the reference", "declare -n r; r=t; t=through; echo \"$r\"; declare -p r"),
    ("nameref to a nameref chain assignment", "declare -n a=b b=c; a=end; echo \"$c\""),
    ("circular reference warning", "declare -n a=b; declare -n b=a; echo \"[$a]\"; echo status=$?"),
    ("self reference", "declare -n s=s; echo status=$?"),
    ("change target with declare -n", "x=1 y=2; declare -n r=x; declare -n r=y; echo $r"),
    ("assignment to a nameref retargets only with -n", "x=1 y=2; declare -n r=x; r=y; echo $x $y $r"),
    ("declare +n", "t=v; declare -n r=t; declare +n r; echo \"$r\"; declare -p r"),
    ("nameref in a for loop over names", "a=1 b=2; declare -n r; for r in a b; do echo \"$r\"; done"),
    ("local -n to a caller variable", "f() { local -n out=$1; out=\"from f\"; }; f result; echo \"$result\""),
    ("local -n shadowing its target name", "f() { local -n v=v; v=x; }; v=orig; f; echo \"$v\"; echo status=$?"),
    ("local -n to a local of the caller", "g() { local -n gr=$1; gr=set-by-g; }; f() { local l=orig; g l; echo \"$l\"; }; f"),
    ("nameref to an array element with an expression index", "a=(x y z); i=1; declare -n r='a[i+1]'; echo $r"),
    ("nameref to an unset array element then assign", "a=(x); declare -n r='a[3]'; r=new; declare -p a"),
    ("nameref to whole array appended", "a=(x); declare -n r=a; r+=(y z); declare -p a"),
    ("nameref element access", "a=(x y); declare -n r=a; echo ${r[1]}; r[2]=z; declare -p a"),
    ("nameref with -i attribute", "declare -in r=t; t=1+1; echo $r"),
    ("nameref to a special variable", "declare -n r=PIPESTATUS; true | false; echo ${r[@]}"),
    ("nameref to a readonly nameref", "t=1; declare -rn r=t; declare -n r=u; echo status=$?"),
    ("nameref exported", "t=exp; declare -nx r=t; bash -c 'echo \"[${r-unset}] [${t-unset}]\"'"),
    ("nameref to a variable named by expansion", "name=target; declare -n r=$name; target=ok; echo $r"),
    ("nameref in a subshell", "t=1; declare -n r=t; ( r=2; echo $t ); echo $t"),
    ("nameref array keys", "declare -A m=([k]=v); declare -n r=m; echo ${!r[@]}"),
    ("indirect expansion of a nameref", "t=v; declare -n r=t; echo ${!r}"),
    ("nameref invalid target at use", "declare -n r=t; t=1; unset t; r=2; echo $t"),
    ("nameref to positional is refused", "declare -n r=1; echo status=$?"),
    ("typeset -n", "t=v; typeset -n r=t; echo $r"),
    ("nameref in arithmetic context", "x=3; declare -n r=x; echo $(( r * 2 )); (( r++ )); echo $x"),
    ("nameref to an assoc key with spaces", "declare -A m=(['a b']=v); declare -n r='m[a b]'; echo \"$r\""),
    ("nameref declared without -n is plain", "t=v; declare r=t; echo $r"),
    ("nameref -p listing", "t=v; declare -n r=t; declare -p r; declare -n"),
]:
    _add("nameref " + label, script, ["param.nameref"])
