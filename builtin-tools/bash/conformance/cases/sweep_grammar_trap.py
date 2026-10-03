"""Grammar sweep: traps (EXIT, ERR, DEBUG, RETURN and the catchable signals), their scoping,
listing and interaction with errexit, functions and subshells (see sweep_grammar_param.py)."""
import itertools

TIER = "sweep"

CASES = []
_NAMES = []


def _add(name, script, tags=()):
    name = "gram trap " + name
    if name in _NAMES:
        raise ValueError("duplicate case name " + name)
    _NAMES.append(name)
    CASES.append((name, script, list(tags)))


# --- EXIT traps by where the shell ends -------------------------------------------------------

ENDINGS = [
    ("end of script", "echo body"),
    ("exit 3", "echo body; exit 3"),
    ("failing last command", "echo body; false"),
    ("errexit", "set -e; echo body; false; echo not"),
    ("nounset", "set -u; echo body; echo $undefined_zz; echo not"),
    ("error operator", "echo body; : ${u_zz?gone}; echo not"),
    ("exit in a function", "f() { exit 4; }; f"),
    ("kill TERM to self", "kill -TERM $$; echo after"),
    ("return at top level", "return 2; echo after"),
    ("exec builtin", "exec true"),
]
HANDLERS = [
    ("echo", "echo \"exit trap status=$?\""),
    ("exit in handler", "echo handler; exit 9"),
    ("failing handler", "echo handler; false"),
    ("reads a variable", "echo \"v=$v\""),
]
for (ename, ending), (hname, handler) in itertools.product(ENDINGS, HANDLERS):
    if hname != "echo" and (len(ename) + len(hname)) % 3:
        continue
    _add("exit " + hname + " at " + ename,
         "v=set; trap '" + handler.replace("'", "'\\''") + "' EXIT; " + ending, ["trap.exit"])

for label, script in [
    ("numeric zero", "trap 'echo zero' 0; echo body"),
    ("SIGEXIT name", "trap 'echo sigexit' SIGEXIT; echo body"),
    ("lower case name", "trap 'echo lower' exit; echo body"),
    ("reset with dash", "trap 'echo never' EXIT; trap - EXIT; echo body"),
    ("ignore with empty string", "trap '' EXIT; echo body"),
    ("replaced", "trap 'echo first' EXIT; trap 'echo second' EXIT"),
    ("in a subshell runs at subshell end", "( trap 'echo sub-exit' EXIT; echo in-sub ); echo after"),
    ("not inherited by a subshell", "trap 'echo main-exit' EXIT; ( echo in-sub ); echo after"),
    ("in a command substitution", "x=$(trap 'echo cs-exit' EXIT; echo in); echo \"[$x]\""),
    ("in a function stays global", "f() { trap 'echo from-f' EXIT; }; f; echo after-f"),
    ("in a pipeline stage", "trap 'echo pipe-exit' EXIT | cat; echo after"),
    ("handler output order", "trap 'echo handler' EXIT; echo body; echo err >&2"),
    ("handler sees the exit status", "trap 'echo \"status=$?\"' EXIT; (exit 5); exit"),
    ("handler changes status only with exit", "trap 'false' EXIT; true"),
    ("handler with a here-document", "trap 'cat <<EOF\nfrom trap\nEOF' EXIT; echo body"),
    ("handler defines and calls a function", "trap 'g() { echo g; }; g' EXIT; echo body"),
    ("handler with multiple commands", "trap 'echo a; echo b' EXIT"),
    ("handler with quotes", "trap \"echo 'single' \\\"double\\\"\" EXIT"),
    ("handler expands late", "v=early; trap 'echo $v' EXIT; v=late"),
    ("handler expands early with double quotes", "v=early; trap \"echo $v\" EXIT; v=late"),
    ("handler invoking exit trap again", "trap 'echo once; exit 2' EXIT; exit 1"),
    ("trap -p EXIT", "trap 'echo it'\\''s' EXIT; trap -p EXIT; trap - EXIT"),
    ("trap -p all", "trap 'echo e' EXIT; trap 'echo r' RETURN; trap -p; trap - EXIT RETURN"),
    ("trap with no arguments lists", "trap 'echo x' EXIT; trap; trap - EXIT"),
    ("trap -p of an unset trap", "trap -p EXIT; echo \"status=$?\""),
    ("trap -p of an ignored signal", "trap '' TERM; trap -p TERM"),
    ("trap -- form", "trap -- 'echo dashdash' EXIT"),
    ("trap with one argument resets", "trap 'echo x' EXIT; trap EXIT; trap -p EXIT; echo status=$?"),
    ("trap with a numeric first argument resets", "trap 'echo x' EXIT; trap 0; echo body"),
    ("invalid signal name", "trap 'echo x' NOSUCH; echo status=$?"),
    ("invalid signal number", "trap 'echo x' 999; echo status=$?"),
    ("missing handler", "trap; echo status=$?; trap -x; echo status=$?"),
    ("several signals at once", "trap 'echo multi' EXIT TERM; trap -p TERM; trap - TERM"),
    ("exit trap and errexit inside handler", "set -e; trap 'false; echo still-in-handler' EXIT; echo body"),
    ("exit trap after a syntax error in eval", "trap 'echo exit-trap' EXIT; eval 'if'; echo after"),
    ("exit trap with set -u error in handler", "trap 'echo $undefined_zz; echo not' EXIT; set -u; echo body"),
    ("exit trap and background job", "trap 'echo exit-trap' EXIT; { echo bg; } & wait"),
    ("exit trap in a sourced file", "printf \"trap 'echo src-exit' EXIT\\n\" > /tmp/t.sh; . /tmp/t.sh; echo body"),
]:
    _add("exit " + label, script, ["trap.exit"])

# --- ERR traps -------------------------------------------------------------------------------

ERR_SITES = [
    ("simple command", "false"),
    ("and list head", "false && true"),
    ("and list tail", "true && false"),
    ("or list", "false || false"),
    ("negated", "! true"),
    ("if condition", "if false; then :; fi"),
    ("while condition", "while false; do :; done"),
    ("pipeline", "true | false"),
    ("subshell", "(false)"),
    ("group", "{ false; }"),
    ("function", "f"),
    ("function with errtrace", "set -E; f"),
    ("command substitution", "x=$(false)"),
    ("command substitution with errtrace", "set -E; x=$(false)"),
    ("arithmetic", "(( 0 ))"),
    ("conditional", "[[ -z x ]]"),
    ("command not found", "nosuchcmd_zz 2>/dev/null"),
    ("failed redirect", "echo > /tmp/no/x 2>/dev/null"),
    ("return nonzero", "g"),
    ("pipefail", "set -o pipefail; false | true"),
]
for sname, site in ERR_SITES:
    _add("err at " + sname,
         "f() { false; echo in-f; }; g() { return 3; }; trap 'echo \"ERR status=$? line=$LINENO\"' ERR; " + site + "; echo after",
         ["trap.err"])
for label, script in [
    ("with errexit", "set -e; trap 'echo err-trap' ERR; false; echo not"),
    ("reset", "trap 'echo err' ERR; trap - ERR; false; echo after"),
    ("not inherited by a function without -E", "trap 'echo err' ERR; f() { false; echo in-f; }; f; echo after"),
    ("inherited with set -o errtrace", "set -o errtrace; trap 'echo err' ERR; f() { false; }; f"),
    ("handler failing does not recurse", "trap 'echo err; false' ERR; false; echo after"),
    ("BASH_COMMAND in handler", "trap 'echo \"cmd=$BASH_COMMAND\"' ERR; false"),
    ("in a subshell", "trap 'echo err' ERR; ( false; echo in-sub )"),
    ("set in a function", "f() { trap 'echo f-err' ERR; }; f; false; echo after"),
    ("loop body", "trap 'echo err' ERR; for i in 1 2; do false; done"),
    ("case body", "trap 'echo err' ERR; case x in x) false;; esac"),
    ("trap -p ERR", "trap 'echo e' ERR; trap -p ERR"),
]:
    _add("err " + label, script, ["trap.err"])

# --- DEBUG and RETURN ----------------------------------------------------------------------------

for label, script in [
    ("counts simple commands", "n=0; trap '((n++))' DEBUG; echo a; echo b; trap - DEBUG; echo \"n=$n\""),
    ("BASH_COMMAND", "trap 'echo \"> $BASH_COMMAND\"' DEBUG; echo one; x=2; trap - DEBUG"),
    ("before compound commands", "trap 'echo \"> $BASH_COMMAND\"' DEBUG; for i in 1; do :; done; if true; then :; fi; trap - DEBUG"),
    ("before arithmetic and conditional", "trap 'echo \"> $BASH_COMMAND\"' DEBUG; (( 1 )); [[ x ]]; trap - DEBUG"),
    ("not inherited by functions", "f() { echo in-f; }; trap 'echo dbg' DEBUG; f; trap - DEBUG"),
    ("inherited with functrace", "set -T; f() { echo in-f; }; trap 'echo \"dbg $BASH_COMMAND\"' DEBUG; f; trap - DEBUG"),
    ("in a subshell", "trap 'echo dbg' DEBUG; ( echo sub ); trap - DEBUG"),
    ("in a pipeline", "trap 'echo \"dbg $BASH_COMMAND\"' DEBUG; echo p | cat; trap - DEBUG"),
    ("handler status", "trap 'false' DEBUG; echo still-runs; trap - DEBUG"),
    ("LINENO in handler", "trap 'echo \"line $LINENO\"' DEBUG\necho a\necho b\ntrap - DEBUG"),
    ("trap -p DEBUG", "trap 'echo d' DEBUG; trap -p DEBUG; trap - DEBUG"),
    ("RETURN after a function", "trap 'echo \"return from ${FUNCNAME[0]-main}\"' RETURN; f() { echo in-f; }; f"),
    ("RETURN not inherited without functrace", "f() { echo in-f; }; trap 'echo ret' RETURN; f; echo after"),
    ("RETURN with functrace", "set -T; trap 'echo ret' RETURN; f() { echo in-f; }; f"),
    ("RETURN after source", "echo 'echo sourced' > /tmp/r.sh; trap 'echo ret-src' RETURN; . /tmp/r.sh"),
    ("RETURN status", "f() { trap 'echo \"ret $?\"' RETURN; return 4; }; f; echo \"after $?\""),
    ("RETURN set inside the function persists", "f() { trap 'echo ret-f' RETURN; }; g() { :; }; f; g; echo after"),
]:
    _add("debug " + label, script, ["trap.debug-return"])

# --- Signals ------------------------------------------------------------------------------------

SIGNALS = ["TERM", "INT", "HUP", "SIGTERM", "15", "2", "1"]
SENDS = [("kill default", "kill $$"), ("kill -s", "kill -s SIG $$"), ("kill -SIG", "kill -SIG $$"),
         ("kill -n", "kill -n NUM $$")]
NUMBERS = {"TERM": "15", "INT": "2", "HUP": "1"}
for sig in ["TERM", "INT", "HUP"]:
    for sname, send in SENDS:
        if sname == "kill default" and sig != "TERM":
            continue
        cmd = send.replace("SIG", sig).replace("NUM", NUMBERS[sig])
        _add("signal caught " + sig + " via " + sname,
             "trap 'echo \"caught " + sig + "\"' " + sig + "; " + cmd + "; echo \"after status=$?\"", ["trap.signal", "jobs.kill"])
        _add("signal default " + sig + " via " + sname, cmd + "; echo not-reached", ["trap.signal", "jobs.kill"])
        _add("signal ignored " + sig + " via " + sname,
             "trap '' " + sig + "; " + cmd + "; echo \"survived status=$?\"", ["trap.signal", "jobs.kill"])
for spec in ["SIGTERM", "15", "sigterm", "SIGINT", "2", "1", "SIGHUP"]:
    _add("signal spec " + spec,
         "trap 'echo \"handler " + spec + "\"' " + spec + "; trap -p | while read -r l; do echo \"$l\"; done; trap - " + spec,
         ["trap.signal"])
for label, script in [
    ("caught in a subshell", "( trap 'echo sub-caught' TERM; kill -TERM $BASHPID; echo sub-after ); echo \"status=$?\""),
    ("default in a subshell", "( kill -TERM $BASHPID; echo not ); echo \"status=$?\""),
    ("handler exits", "trap 'echo bye; exit 7' TERM; kill $$; echo not"),
    ("ignored inherited by a subshell", "trap '' TERM; ( kill -TERM $BASHPID; echo sub-survived ); echo \"status=$?\""),
    ("caught is reset in a subshell", "trap 'echo main' TERM; ( kill -TERM $BASHPID; echo not ); echo \"status=$?\""),
    ("trap -p in a subshell shows caught traps", "trap 'echo main' TERM; ( trap -p TERM ); trap - TERM"),
    ("trap -p in a command substitution", "trap 'echo main' TERM; x=$(trap -p TERM); echo \"[$x]\"; trap - TERM"),
    ("kill -0 self", "kill -0 $$; echo \"status=$?\""),
    ("kill with a bad signal", "kill -s NOSUCH $$; echo \"status=$?\""),
    ("kill with no arguments", "kill; echo \"status=$?\""),
    ("kill -l numbers", "kill -l 1 2 15 9; kill -l 130"),
    ("kill a background job with TERM", "{ sleep 5 & p=$!; kill -TERM $p; wait $p; echo \"status=$?\"; } 2>/dev/null"),
    ("kill a background job with a trap inside", "{ ( trap 'echo job-caught; exit 3' TERM; : > /tmp/ready; sleep 5 & wait ) & p=$!; until [[ -f /tmp/ready ]]; do sleep 0.01; done; kill -TERM $p; wait $p; echo \"status=$?\"; } 2>/dev/null"),
    ("signal and EXIT trap order", "trap 'echo exit-trap' EXIT; trap 'echo term-trap' TERM; kill $$; echo after"),
    ("signal during a loop", "trap 'echo caught; stop=1' TERM; stop=0; for i in 1 2 3; do (( i == 2 )) && kill $$; (( stop )) && break; echo $i; done"),
    ("USR1 trap", "trap 'echo usr1' USR1; kill -USR1 $$; echo after"),
    ("CHLD trap", "trap 'echo chld' CHLD; true & wait; trap - CHLD; echo done"),
]:
    _add("signal " + label, script, ["trap.signal"])


# Divergences that are decisions, not bugs.
EXPECTED = {
    'gram trap signal kill a background job with a trap inside': (
        0, b'job-caught\nstatus=3\n', b'bash: stopped job (pid 3, hangup): sleep 5\n',
        "fixture: a call ends with its jobs, so the sleep a subshell left "
        "running is stopped and reported when the call ends (README, background jobs), where bash "
        "leaves it running unreported",
    ),
}
