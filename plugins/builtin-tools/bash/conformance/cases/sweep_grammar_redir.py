"""Grammar sweep: redirections (files, descriptors, duplication, closing, {var}, noclobber,
errors) and here-documents and here-strings (see sweep_grammar_param.py for the scheme)."""
import itertools

TIER = "sweep"

CASES = []
_NAMES = []


def _add(name, script, tags=()):
    name = "gram redir " + name
    if name in _NAMES:
        raise ValueError("duplicate case name " + name)
    _NAMES.append(name)
    CASES.append((name, script, list(tags)))


# --- Operators against targets ----------------------------------------------------------------

SETUP = "cd /tmp; echo old > ex; mkdir -p dir; "
OPERATORS = [("out", ">"), ("append", ">>"), ("in", "<"), ("read write", "<>"), ("clobber", ">|"),
             ("both", "&>"), ("both append", "&>>"), ("dup out", ">&"), ("fd3 out", "3>"), ("fd0 out", "0>")]
TARGETS = [("new file", "new"), ("existing file", "ex"), ("directory", "dir"),
           ("missing directory", "no/such"), ("empty variable", "$e"), ("spaced variable", "$sp"),
           ("quoted spaced variable", "\"$sp\""), ("dev null", "/dev/null"), ("glob one", "e*"),
           ("glob two", "*x*"), ("brace", "{p,q}"), ("tilde", "~/t")]
for (oname, op), (tname, target) in itertools.product(OPERATORS, TARGETS):
    if oname not in ("out", "in", "append") and (len(oname) + len(tname)) % 3:
        continue
    cmd = "cat" if op in ("<", "<>") else "echo data"
    _add("operator " + oname + " to " + tname,
         SETUP + "HOME=/tmp; e=; sp='a b'; " + cmd + " " + op + " " + target + "; echo \"status=$?\"; for f in new ex; do [[ -f $f ]] && printf '%s=[%s]\\n' \"$f\" \"$(< $f)\"; done; echo *",
         ["redirection.output"])

# noclobber.
for label, script in [
    ("refuses an existing file", "set -C; echo new > ex; echo status=$?; cat ex"),
    ("allows a new file", "set -C; echo n > fresh; cat fresh"),
    ("allows append", "set -C; echo more >> ex; cat ex"),
    ("clobber operator overrides", "set -C; echo forced >| ex; cat ex"),
    ("allows dev null", "set -C; echo x > /dev/null; echo status=$?"),
    ("read write is allowed", "set -C; echo rw 1<> ex; cat ex"),
    ("both operator refuses", "set -C; echo x &> ex; echo status=$?"),
    ("off again", "set -C; set +C; echo again > ex; cat ex"),
    ("set -o noclobber", "set -o noclobber; echo x > ex; echo status=$?"),
    ("with exec", "set -C; exec 3> ex; echo status=$?"),
    ("in a subshell", "set -C; ( echo x > ex ); echo status=$?"),
    ("empty file is refused too", ": > empty; set -C; echo x > empty; echo status=$?"),
]:
    _add("noclobber " + label, SETUP + script, ["redirection.noclobber"])

# --- Descriptor duplication and order ---------------------------------------------------------

EMIT = "{ echo out; echo err >&2; }"
ORDERS = [
    ("stderr to stdout then file", "2>&1 > f"), ("file then stderr to stdout", "> f 2>&1"),
    ("stdout to stderr", ">&2"), ("both to file", "&> f"), ("stderr to file", "2> f"),
    ("stderr closed", "2>&-"), ("stdout closed", ">&-"), ("swap through fd 3", "3>&1 1>&2 2>&3 3>&-"),
    ("stderr to dev null", "2>/dev/null"), ("stdout to dev stderr", ">/dev/stderr"),
    ("stderr to dev stdout", "2>/dev/stdout"), ("fd 3 unopened", ">&3"), ("fd 9 then use", "9>f >&9"),
    ("move fd", "3>f 1>&3-"), ("append both", "&>> f"), ("stderr append", "2>> f"),
    ("stdout to stdout", ">&1"), ("dup with variable", ">&$fd"), ("dup to word", ">& f"),
    ("stderr to closed stdout", ">&- 2>&1"), ("dup out of range", ">&99999999999"),
    ("dup a negative", ">&-1"),
]
for label, redir in ORDERS:
    _add("order " + label, "cd /tmp; fd=2; " + EMIT + " " + redir + "; echo \"status=$?\"; [[ -f f ]] && { echo file:; cat f; }; true",
         ["redirection.duplicate"])
    if (len(label) % 3) == 0:
        _add("order in a pipeline " + label,
             "cd /tmp; fd=2; " + EMIT + " " + redir + " | while read -r l; do echo \"piped:$l\"; done; [[ -f f ]] && cat f; true",
             ["redirection.duplicate", "compound.pipeline"])

# Input duplication and closing.
for label, script in [
    ("read from fd 3", "exec 3< /tmp/in; read -r a <&3; read -r b <&3; echo \"$a $b\"; exec 3<&-"),
    ("close stdin then read", "read x <&-; echo status=$?"),
    ("close stdin for cat", "cat <&-; echo status=$?"),
    ("read from closed fd", "exec 3<&-; read x <&3; echo status=$?"),
    ("read write descriptor", "exec 4<> /tmp/in; read -r l <&4; echo \"$l\"; echo appended >&4; exec 4>&-; cat /tmp/in"),
    ("input from dev stdin", "echo via | cat /dev/stdin"),
    ("input dup of stdin", "echo dup | cat <&0"),
    ("here-string to fd 5", "cat <&5 5<<< five"),
    ("input from a missing file stops the command", "cat < /tmp/missing; echo status=$?"),
    ("input redirect before command", "< /tmp/in cat"),
    ("redirect only command creates a file", "> /tmp/created; [[ -f /tmp/created ]] && echo exists"),
    ("redirect only command missing input", "< /tmp/missing; echo status=$?"),
    ("redirect in the middle of arguments", "echo a > /tmp/mid b c; cat /tmp/mid"),
    ("multiple output redirects", "echo m > /tmp/m1 > /tmp/m2; cat /tmp/m1; echo --; cat /tmp/m2"),
    ("multiple input redirects", "cat < /tmp/in < /tmp/in2"),
    ("fd above 9", "exec 10> /tmp/ten; echo ten >&10; exec 10>&-; cat /tmp/ten"),
    ("fd 255", "echo x >&255; echo status=$?"),
    ("redirect expansion happens once", "n=0; echo x > /tmp/r$((n+=1)); echo $n; cat /tmp/r1"),
    ("redirect target with command substitution", "echo sub > \"/tmp/$(echo sub)\"; cat /tmp/sub"),
    ("stdout to a file and back", "exec 3>&1; exec > /tmp/cap; echo captured; exec 1>&3 3>&-; cat /tmp/cap"),
    ("stderr of a builtin", "cd /nonexistent 2> /tmp/e; echo status=$?; cat /tmp/e"),
    ("stderr of a syntax error in eval", "eval 'if' 2> /tmp/e; echo status=$?; cat /tmp/e"),
    ("stderr of an expansion error", "( echo ${u?gone} ) 2> /tmp/e; echo status=$?; cat /tmp/e"),
    ("command not found redirected", "nosuchcmd_zz 2> /tmp/e; echo status=$?; cat /tmp/e"),
    ("redirect on an assignment", "v=1 > /tmp/asg; echo $v; [[ -f /tmp/asg ]] && echo created"),
    ("redirect failure skips an assignment", "v=1 > /tmp/no/x; echo \"[${v-unset}] $?\""),
    ("redirect failure on a function call", "f() { echo called; }; f > /tmp/no/x; echo status=$?"),
    ("redirect failure on a group", "{ echo in-group; } > /tmp/no/x; echo status=$?"),
    ("redirect failure on a loop", "for i in 1; do echo body; done > /tmp/no/x; echo status=$?"),
    ("redirect failure on a subshell", "( echo sub ) > /tmp/no/x; echo status=$?"),
    ("redirect failure on exec", "exec 3> /tmp/no/x; echo \"after status=$?\""),
    ("redirect failure on a special builtin in posix mode", "set -o posix; : > /tmp/no/x; echo after"),
    ("redirect failure with set -e", "set -e; echo x > /tmp/no/x; echo not"),
    ("ambiguous redirect status", "v='a b'; echo x > $v; echo status=$?"),
    ("unset variable redirect", "echo x > $unset_zz; echo status=$?"),
    ("redirect to a quoted empty string", "echo x > ''; echo status=$?"),
    ("redirect to an fd variable name", "exec {fdv}> /tmp/fv; echo \"fd ok $((fdv >= 10))\"; echo into >&$fdv; exec {fdv}>&-; cat /tmp/fv"),
    ("fd variable for input", "exec {rin}< /tmp/in; read -r -u $rin line; echo \"$line\"; exec {rin}<&-"),
    ("fd variable close then use", "exec {c}> /tmp/fc; exec {c}>&-; echo x >&$c; echo status=$?"),
    ("fd variable on a command", "echo on-cmd {w}> /tmp/fw; echo \"$((w >= 10))\"; cat /tmp/fw"),
    ("fd variable invalid name", "exec {1bad}> /tmp/x; echo status=$?"),
    ("fd variable with an array element", "exec {a[1]}> /tmp/fa; echo \"$(( a[1] >= 10 ))\"; echo el >&${a[1]}; cat /tmp/fa"),
    ("fd variable readonly", "readonly ro=1; exec {ro}> /tmp/x; echo status=$?"),
    ("varredir_close", "shopt -s varredir_close; echo x {v}> /tmp/vc; echo y >&$v; echo status=$?"),
    ("dev fd path of an open descriptor", "exec 3< /tmp/in; cat /dev/fd/3; exec 3<&-"),
    ("dev fd of a closed descriptor", "cat /dev/fd/7; echo status=$?"),
    ("dev stderr in a redirect", "echo to-err > /dev/stderr"),
    ("redirect inside command substitution", "x=$(echo inner > /tmp/ic; echo out); echo \"$x\"; cat /tmp/ic"),
    ("redirect of a pipeline stage", "echo a 2>/dev/null | cat > /tmp/ps; cat /tmp/ps"),
    ("pipe and stderr operator", "{ echo o; echo e >&2; } |& while read -r l; do echo \"<$l>\"; done"),
    ("pipe stderr only", "{ echo o; echo e >&2; } 2>&1 >/dev/null | while read -r l; do echo \"<$l>\"; done"),
]:
    _add("fd " + label, "printf 'line1\\nline2\\n' > /tmp/in; printf 'other\\n' > /tmp/in2; " + script,
         ["redirection.duplicate"])

# Redirections attached to compound commands and functions.
COMPOUNDS = [
    ("group", "{ echo g; }"), ("subshell", "( echo s )"), ("for", "for i in 1; do echo f; done"),
    ("while", "while read -r l; do echo \"w:$l\"; done"), ("if", "if true; then echo i; fi"),
    ("case", "case x in x) echo c;; esac"), ("arith", "(( 1 ))"), ("cond", "[[ -n x ]]"),
    ("function call", "fn"), ("select", "select s in a; do echo \"$s\"; break; done"),
]
REDIRS = [("output", "> /tmp/o"), ("input", "< /tmp/in"), ("both", "&> /tmp/o"), ("append twice", ">> /tmp/o >> /tmp/o"),
          ("here-string", "<<< hs"), ("stderr", "2> /tmp/o")]
for (cname, comp), (rname, red) in itertools.product(COMPOUNDS, REDIRS):
    if (len(cname) + len(rname)) % 2:
        continue
    _add("compound " + cname + " with " + rname,
         "printf 'line1\\n' > /tmp/in; fn() { echo fn; read -r l && echo \"fn:$l\"; }; " + comp + " " + red + "; echo \"status=$?\"; [[ -f /tmp/o ]] && cat /tmp/o; true",
         ["redirection.output"])

# exec redirections persist.
for label, script in [
    ("exec output to a file", "exec > /tmp/eo; echo hidden; exec >&2; cat /tmp/eo >&2"),
    ("exec stderr to stdout", "exec 2>&1; echo e >&2"),
    ("exec input", "printf 'a\\nb\\n' > /tmp/ei; exec < /tmp/ei; read x; read y; echo \"$x$y\""),
    ("exec closes", "exec 3> /tmp/c3; exec 3>&-; echo x >&3; echo status=$?"),
    ("exec in a subshell does not persist", "( exec > /tmp/es; echo inner ); echo outer; cat /tmp/es"),
    ("exec in a function persists", "f() { exec 4> /tmp/ef; }; f; echo via4 >&4; cat /tmp/ef"),
    ("exec with no command and no redirect", "exec; echo still"),
    ("exec numbered duplicate", "exec 5>&1; echo five >&5"),
    ("exec move descriptor", "exec 6>&1; exec 7>&6-; echo seven >&7; echo x >&6; echo status=$?"),
]:
    _add("exec " + label, script, ["redirection.exec"])

# --- Here-documents --------------------------------------------------------------------------

HD_DELIMS = [("plain", "EOF", True), ("single quoted", "'EOF'", False), ("double quoted", "\"EOF\"", False),
             ("partly quoted", "E\"O\"F", False), ("backslashed", "\\EOF", False),
             ("dash plain", "-EOF", True), ("dash quoted", "-'EOF'", False)]
HD_BODIES = [
    ("variables", "v=$v braced=${v} default=${u:-d}"),
    ("command substitution", "sub=$(echo s) back=`echo b`"),
    ("arithmetic", "sum=$((1+2)) legacy=$[2*2]"),
    ("escapes", "dollar=\\$v backslash=\\\\ backquote=\\` quote=\\\" other=\\n"),
    ("quotes are literal", "'single' \"double\" $'ansi'"),
    ("line continuation", "first \\\ncontinued"),
    ("tabs", "\tone tab\n\t\ttwo tabs\n    spaces"),
    ("positional", "args=$# first=$1 all=$@"),
    ("array", "arr=${a[1]} all=${a[*]} count=${#a[@]}"),
    ("glob and tilde are literal", "* ~ {a,b}"),
    ("empty", ""),
    ("delimiter like lines", "EOF \n EOF\nEOFX"),
]
for (dname, delim, _), (bname, body) in itertools.product(HD_DELIMS, HD_BODIES):
    if (len(dname) + len(bname)) % 2 and bname not in ("variables", "escapes", "tabs"):
        continue
    dash = delim.startswith("-")
    op = "<<-" if dash else "<<"
    word = delim[1:] if dash else delim
    end = "\tEOF" if dash else "EOF"
    script = ("cd /tmp; HOME=/h; v=val; a=(x y z); set -- p1 p2; cat " + op + word + "\n"
              + (body + "\n" if body else "") + end + "\necho \"status=$?\"")
    _add("heredoc " + dname + ": " + bname, script, ["redirection.heredoc"])

for label, script in [
    ("two on one line", "cat <<A; cat <<B\none\nA\ntwo\nB"),
    ("two on one command", "cat <<A <<B\none\nA\ntwo\nB"),
    ("to a numbered fd", "cat <&3 3<<EOF\nfd3\nEOF"),
    ("to a while loop", "while read -r l; do echo \"<$l>\"; done <<EOF\na\n b\nEOF"),
    ("in a function called twice", "f() {\ncat <<EOF\nin f $1\nEOF\n}\nf one; f two"),
    ("in a loop", "for i in 1 2; do\ncat <<EOF\niter $i\nEOF\ndone"),
    ("in a command substitution", "x=$(cat <<EOF\ninside\nEOF\n); echo \"[$x]\""),
    ("in a pipeline", "cat <<EOF | while read -r l; do echo \"p:$l\"; done\nx\ny\nEOF"),
    ("followed by more commands on the line", "cat <<EOF && echo after\nbody\nEOF"),
    ("in an if condition", "if cat <<EOF\ncond\nEOF\nthen echo yes; fi"),
    ("delimiter with trailing space does not end it", "cat <<EOF\nbody\nEOF \nEOF"),
    ("unterminated with dash", "cat <<-EOF\n\tno end"),
    ("empty delimiter", "cat <<''\nbody\n\necho after"),
    ("delimiter with a dollar", "cat <<$x\nbody\n$x"),
    ("delimiter with spaces quoted", "cat <<'E F'\nbody\nE F"),
    ("delimiter is a number", "cat <<1\nbody\n1"),
    ("body with a lone backslash at the end", "cat <<EOF\nend\\\nEOF"),
    ("unquoted body keeps single quotes", "v=x; cat <<EOF\n'$v'\nEOF"),
    ("dash strips only tabs", "cat <<-EOF\n\t\ttabbed\n  spaced\n\tEOF"),
    ("dash with a tab before delimiter", "cat <<-EOF\n\tbody\n\t\tEOF"),
    ("here-document into a variable with read", "read -r -d '' v <<EOF\nmulti\nline\nEOF\necho \"$v\""),
    ("here-document with nested command substitution heredoc", "cat <<A\n$(cat <<B\ninner\nB\n)\nA"),
    ("here-document to a function", "f() { cat; }; f <<EOF\nto-f\nEOF"),
    ("here-document on a group", "{ read a; read b; } <<EOF\n1\n2\nEOF\necho $a$b"),
    ("here-document with a very long line", "cat <<EOF | { IFS= read -r l; echo ${#l}; }\n" + "x" * 3000 + "\nEOF"),
    ("here-document body expansions run once", "n=0; cat <<EOF\n$((n+=1))\nEOF\necho $n"),
    ("here-document with unset under set -u", "set -u; cat <<EOF\n${u:-fine}\nEOF"),
    ("here-document with a comment line", "cat <<EOF\n# not a comment\nEOF"),
    ("here-document in eval", "eval 'cat <<EOF\nevaled\nEOF'"),
    ("here-document after a semicolon list", "echo a; cat <<EOF; echo b\nmid\nEOF"),
    ("here-document with CRLF delimiter", "cat <<EOF\nbody\nEOF\r\necho after\nEOF"),
    ("here-document and xtrace", "set -x; cat <<EOF\ntraced\nEOF"),
]:
    _add("heredoc " + label, script, ["redirection.heredoc"])

# --- Here-strings ----------------------------------------------------------------------------

for label, word in [
    ("literal", "word"), ("empty", "''"), ("variable with spaces", "$sp"), ("quoted variable", "\"$sp\""),
    ("array at", "${a[@]}"), ("quoted array at", "\"${a[@]}\""), ("array star", "\"${a[*]}\""),
    ("dollar at", "$@"), ("glob", "*"), ("tilde", "~"), ("brace", "{a,b}"), ("newline value", "$nl"),
    ("trailing newline value", "$tn"), ("command substitution", "$(printf 'x\\n\\n')"),
    ("ansi-c", "$'a\\tb'"), ("arithmetic", "$((6*7))"), ("unset", "$unset_zz"),
]:
    _add("herestring " + label,
         "cd /tmp; HOME=/h; sp='a  b'; a=(x 'y z'); set -- p 'q r'; nl=$'l1\\nl2'; tn=$'t\\n'; "
         "while IFS= read -r l; do printf '<%s>' \"$l\"; done <<< " + word + "; echo",
         ["redirection.herestring"])
for label, script in [
    ("to read -a", "read -ra arr <<< 'a b  c'; echo ${#arr[@]}"),
    ("to a function", "f() { read -r x; echo \"[$x]\"; }; f <<< in-f"),
    ("to a numbered fd", "read -r -u 4 x 4<<< fdfour; echo $x"),
    ("twice on one command", "cat <<< first <<< second"),
    ("with a here-document", "cat <<< hs <<EOF\nhd\nEOF"),
    ("in a loop condition", "while read -r l; do echo $l; break; done <<< loop"),
    ("large", "v=$(printf '%0500d' 0); cat <<< \"$v\" | { IFS= read -r l; echo ${#l}; }"),
]:
    _add("herestring " + label, script, ["redirection.herestring"])

EXPECTED_STDERR = {
    "gram redir fd close stdin for cat": (
        b"cat: -: Bad file descriptor\n",
        "as in tool_layer.py: GNU utilities report a closed standard input a second time "
        "when they close it at exit (`cat: closing standard input: …`); bash-tool reports the "
        "failed read once",
    ),
}
