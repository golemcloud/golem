"""The features the conformance matrix must cover, and how a case is credited with one.

`conformance.py coverage` fails if a feature below has no case, or if a case names a tag that is
not a feature. A case covers a feature when:

- it lists the tag (in its `CASES` entry, or in the corpus's `TAGS`);
- its name starts with `<command>:` for a command in COMMANDS (credits `cmd.<command>`), and, when
  the case is an error path (tagged `error`, or its corpus sets `ERROR_PATHS = True`), also
  `cmd.<command>.error`;
- its script uses syntax a detector in DETECTORS recognises (quoting, expansions, redirections,
  compound commands), or runs a builtin or command in command position (credits `builtin.<name>`
  or `cmd.<name>`, never an error path).

Out-of-scope features are listed in NOT_APPLICABLE with the reason, and need no case.
"""
import re

# Commands this shell adds beyond bash's builtins. `coverage` checks this list against the
# commands `compgen -c` lists from the shell itself that are neither builtins (`compgen -b`) nor
# keywords, so a new command cannot be registered without appearing here.
COMMANDS = """
b2sum base32 base64 basename basenc cat cksum cmp comm cp csplit curl cut date diff dirname env
expand expr factor file find fmt fold grep head jq join link ln ls man md5sum mkdir mktemp mv nl
numfmt od paste patch printenv printf readlink realpath rev rm rmdir sed seq sha1sum sha256sum
sha512sum shuf sleep sort split stat tac tail tee timeout touch tr truncate tsort unexpand uniq
unlink wc wget which xargs yes sh bash chmod dd du hostname install nproc uname
""".split()

# Commands in COMMANDS that this shell implements in place of one of bash's builtins: bash, and
# `compgen -b`, list them as builtins.
BUILTIN_REPLACEMENTS = ["printf"]

# Commands registered only by the cooperative example, for tests.
FIXTURES = ["probe-tool", "legacy-stdin"]

# Bash's own builtins as Brush provides them; `coverage` checks this list against `compgen -b`.
BUILTINS = """
. : [ alias bg bind break builtin caller cd command compgen complete compopt continue declare dirs
disown echo enable eval exec exit export false fc fg getopts hash help history jobs kill let local
logout mapfile popd pushd pwd read readarray readonly return set shift shopt source suspend test
times trap true type typeset ulimit umask unalias unset wait
""".split()

LANGUAGE = {
    # Quoting and syntax
    "syntax.quote.single": "single quotes",
    "syntax.quote.double": "double quotes",
    "syntax.quote.ansi-c": "$'...' quoting",
    "syntax.quote.escape": "backslash escapes",
    "syntax.comment": "comments",
    "syntax.error": "syntax errors and how much of the script runs",
    "syntax.alias": "alias expansion",
    # Commands
    "compound.pipeline": "pipelines",
    "compound.pipeline.negate": "! pipelines",
    "compound.list": "&&, || and ; lists",
    "compound.group": "{ ...; } groups",
    "compound.subshell": "( ... ) subshells",
    "compound.if": "if / elif / else",
    "compound.case": "case",
    "compound.case.fallthrough": "case ;& and ;;&",
    "compound.for": "for name in words",
    "compound.for-arith": "for (( ; ; ))",
    "compound.while": "while loops",
    "compound.until": "until loops",
    "compound.select": "select",
    "compound.arith": "(( ... )) commands",
    "compound.cond": "[[ ... ]]",
    "compound.cond.regex": "[[ =~ ]] and BASH_REMATCH",
    "compound.function": "function definitions",
    "compound.function.local": "local variables",
    "compound.function.recursion": "recursive functions",
    # Parameters and variables
    "param.positional": "$1, $@, $*, $# and shift / set --",
    "param.special": "$?, $$, $!, $0, $- and $_",
    "param.array.indexed": "indexed arrays",
    "param.array.assoc": "associative arrays",
    "param.nameref": "namerefs (declare -n)",
    "param.indirect": "${!name} indirection",
    "param.readonly": "readonly variables",
    "param.export": "exported variables reaching children",
    "param.attributes": "declare -i / -l / -u / -x attributes",
    "param.callstack": "FUNCNAME, BASH_SOURCE, LINENO, BASH_LINENO",
    "param.random": "RANDOM, SECONDS, EPOCHSECONDS and friends",
    # Expansions
    "expansion.brace": "brace expansion",
    "expansion.tilde": "tilde expansion",
    "expansion.parameter.default": "${x:-w} ${x-w} ${x:=w} ${x=w}",
    "expansion.parameter.alternative": "${x:+w} ${x+w}",
    "expansion.parameter.error": "${x:?w} ${x?w}",
    "expansion.parameter.length": "${#x}",
    "expansion.parameter.substring": "${x:offset:length}",
    "expansion.parameter.trim": "${x#p} ${x##p} ${x%p} ${x%%p}",
    "expansion.parameter.replace": "${x/p/r} and its anchored forms",
    "expansion.parameter.case": "${x^} ${x^^} ${x,} ${x,,}",
    "expansion.parameter.transform": "${x@Q} and the other @ operators",
    "expansion.parameter.names": "${!prefix*} and ${!array[@]}",
    "expansion.command": "$( ... ) and backquote substitution",
    "expansion.arithmetic": "$(( ... ))",
    "expansion.process": "process substitution: <(list) and >(list), buffered",
    "expansion.word-splitting": "word splitting and IFS",
    "expansion.glob": "pathname expansion",
    "expansion.glob.options": "extglob, globstar, nullglob, failglob, dotglob, nocaseglob",
    # Redirections
    "redirection.input": "< file",
    "redirection.output": "> file",
    "redirection.append": ">> file",
    "redirection.noclobber": "set -C and >|",
    "redirection.both": "&> and &>>",
    "redirection.duplicate": "n>&m and n<&m",
    "redirection.close": "n>&- and n<&-",
    "redirection.heredoc": "<< here documents",
    "redirection.heredoc.strip": "<<- here documents",
    "redirection.heredoc.quoted": "quoted here-document delimiters",
    "redirection.herestring": "<<< here strings",
    "redirection.fd-variable": "{var}> redirections",
    "redirection.dev-paths": "/dev/null, /dev/stdin, /dev/stdout, /dev/stderr, /dev/fd/N",
    "redirection.exec": "exec redirections",
    # Options
    "option.errexit": "set -e",
    "option.nounset": "set -u",
    "option.xtrace": "set -x",
    "option.pipefail": "set -o pipefail",
    "option.noglob": "set -f",
    "option.allexport": "set -a",
    "option.noexec": "set -n",
    "option.posix": "set -o posix",
    "option.lastpipe": "shopt -s lastpipe",
    "option.nocasematch": "shopt -s nocasematch",
    # Traps and jobs
    "trap.exit": "EXIT traps",
    "trap.err": "ERR traps",
    "trap.pipe": "PIPE traps and SIGPIPE",
    "trap.signal": "traps on TERM, INT, HUP",
    "trap.debug-return": "DEBUG and RETURN traps",
    "jobs.background": "& background jobs",
    "jobs.wait": "wait, wait -n, wait PID",
    "jobs.kill": "kill and signals to jobs",
    "jobs.end-of-run": "jobs left running when the script ends",
    # The environment a Golem agent has
    "env.home": "HOME unset, ~ and cd",
    "env.identity": "UID, EUID, USER, HOSTNAME, OSTYPE",
    "env.path": "PATH and command lookup",
    "env.golem": "GOLEM_* variables",
    "env.tmp": "a missing /tmp",
    "env.filesystem": "writing outside /tmp",
    # Tools and children
    "tool.sibling": "sibling tools as commands",
    "tool.refused": "refused Clank workflows",
    "child.shell": "sh -c and bash -c",
    "child.exec": "find -exec and xargs running shell commands",
    "state.cross-call": "separate calls: only the working directory carries",
}

NOT_APPLICABLE = {
    # `bind` and `history` both work now (see stateless.rs and Brush's own history.rs/bind.rs):
    # `history` is a faithful in-memory list (nothing auto-records into it, matching bash's own
    # non-interactive behavior, but -c/-d/-s/-p/-a/-n/-r/-w and N all work against it); `bind`'s
    # binding forms warn ("line editing not enabled") and succeed, matching bash's own
    # non-interactive behavior exactly. Only bind's *query* forms (-l/-p/-P/-s/-S/-v/-V/-q/-u/-X,
    # which would need readline's real default keymap and variable tables to answer faithfully)
    # are still refused.
    "builtin.fc": "history editing needs an interactive shell",
    "builtin.complete": "programmable completion needs an interactive shell",
    "builtin.compopt": "programmable completion needs an interactive shell",
    "builtin.logout": "there is no login shell: logout fails as bash's does outside one",
    "builtin.bg": "job control needs an interactive shell",
    "builtin.fg": "job control needs an interactive shell",
    "builtin.help": "help text is Brush's own",
    "compound.coproc": "coproc needs processes and is refused",
    "builtin.umask": "refused: an agent's files have no permission bits to mask",
    "builtin.ulimit": "refused: WASI has no resource limits",
}


def features():
    """Every feature tag, with a description or an N/A reason."""
    table = dict(LANGUAGE)
    for name in BUILTINS:
        table[f"builtin.{name}"] = f"the {name} builtin"
    for name in COMMANDS:
        table[f"cmd.{name}"] = f"the {name} command"
        table[f"cmd.{name}.error"] = f"an error path of {name}"
    for tag in NOT_APPLICABLE:
        table.setdefault(tag, "not applicable")
    return table


# Tags that are not features but qualify others.
QUALIFIERS = {"error"}

# Syntax detectors: (feature, regex). They credit what a script exercises; tags cover the rest.
DETECTORS = [
    ("syntax.quote.single", r"'"),
    ("syntax.quote.double", r'"'),
    ("syntax.quote.ansi-c", r"\$'"),
    ("syntax.quote.escape", r"\\[^\n]"),
    ("syntax.comment", r"(^|\s)#[^!{]"),
    ("compound.pipeline", r"[^|]\|[^|]"),
    ("compound.pipeline.negate", r"(^|[;&|(]\s*|\bthen\s+|\bdo\s+)!\s"),
    ("compound.list", r"&&|\|\|"),
    ("compound.group", r"(^|[;&|(]\s*)\{\s"),
    ("compound.subshell", r"(^|[;&|]\s*)\((?!\()"),
    ("compound.if", r"\bif\b.*\bthen\b"),
    ("compound.case", r"\bcase\b.*\bin\b"),
    ("compound.case.fallthrough", r";;&|;&"),
    ("compound.for", r"\bfor\s+\w+(\s+in\b|\s*;|\s*$)"),
    ("compound.for-arith", r"\bfor\s*\(\("),
    ("compound.while", r"\bwhile\b"),
    ("compound.until", r"\buntil\b"),
    ("compound.select", r"\bselect\s+\w+"),
    ("compound.arith", r"(^|[;&|]\s*)\(\("),
    ("compound.cond", r"\[\["),
    ("compound.cond.regex", r"=~"),
    ("compound.function", r"\w+\s*\(\)\s*\{|\bfunction\s+\w+"),
    ("compound.function.local", r"\blocal\b"),
    ("param.positional", r"\$[1-9#@*]|\bshift\b|\bset\s+--"),
    ("param.special", r"\$[?$!0_-]"),
    ("param.array.indexed", r"\w+=\(|\$\{\w+\[[@*0-9]"),
    ("param.array.assoc", r"declare\s+-\w*A|local\s+-\w*A"),
    ("param.nameref", r"(declare|local)\s+-\w*n"),
    ("param.indirect", r"\$\{!\w+\}"),
    ("param.readonly", r"\breadonly\b|declare\s+-\w*r"),
    ("param.export", r"\bexport\b"),
    ("param.attributes", r"declare\s+-\w*[ilux]"),
    ("param.callstack", r"FUNCNAME|BASH_SOURCE|LINENO"),
    ("param.random", r"\$\{?(RANDOM|SECONDS|EPOCHSECONDS|EPOCHREALTIME|SRANDOM)"),
    ("expansion.brace", r"\{[^{}\s,]*,[^{}\s]*\}|\{[^{}\s]+\.\.[^{}\s]+\}"),
    ("expansion.tilde", r"(^|[\s=:])~"),
    ("expansion.parameter.default", r"\$\{[^}]*:?[-=][^}]*\}"),
    ("expansion.parameter.alternative", r"\$\{[^}]*:?\+[^}]*\}"),
    ("expansion.parameter.error", r"\$\{\w+:?\?"),
    ("expansion.parameter.length", r"\$\{#\w"),
    ("expansion.parameter.substring", r"\$\{\w+:[-0-9 ]"),
    ("expansion.parameter.trim", r"\$\{\w+(\[[^]]*\])?(##?|%%?)"),
    ("expansion.parameter.replace", r"\$\{\w+(\[[^]]*\])?//?"),
    ("expansion.parameter.case", r"\$\{\w+(\^\^?|,,?)"),
    ("expansion.parameter.transform", r"\$\{\w+@[QEPAaULuK]\}"),
    ("expansion.parameter.names", r"\$\{![\w]+(\*|@|\[)"),
    ("expansion.command", r"\$\((?!\()|`"),
    ("expansion.arithmetic", r"\$\(\("),
    ("expansion.process", r"[<>]\("),
    ("expansion.word-splitting", r"\bIFS="),
    ("redirection.input", r"(^|[^<])<[^<&(]"),
    ("redirection.output", r"(^|[^>&])>[^>&(|]"),
    ("redirection.append", r">>"),
    ("redirection.noclobber", r">\||\bset\s+-\w*C|noclobber"),
    ("redirection.both", r"&>"),
    ("redirection.duplicate", r"\d?[<>]&\d"),
    ("redirection.close", r"[<>]&-"),
    ("redirection.heredoc", r"<<[^<-]"),
    ("redirection.heredoc.strip", r"<<-"),
    ("redirection.heredoc.quoted", r"<<-?\s*['\"]"),
    ("redirection.herestring", r"<<<"),
    ("redirection.fd-variable", r"\{\w+\}[<>]"),
    ("redirection.dev-paths", r"/dev/(null|stdin|stdout|stderr|fd/)"),
    ("redirection.exec", r"\bexec\s+\d*[<>]"),
    ("option.errexit", r"\bset\s+-\w*e|\bbash\s+-\w*e"),
    ("option.nounset", r"\bset\s+-\w*u|\bbash\s+-\w*u"),
    ("option.xtrace", r"\bset\s+-\w*x"),
    ("option.pipefail", r"pipefail"),
    ("option.noglob", r"\bset\s+-\w*f"),
    ("option.allexport", r"\bset\s+-\w*a"),
    ("option.noexec", r"\bset\s+-\w*n"),
    ("option.posix", r"-o\s+posix"),
    ("option.lastpipe", r"lastpipe"),
    ("option.nocasematch", r"nocasematch"),
    ("expansion.glob.options", r"extglob|globstar|nullglob|failglob|dotglob|nocaseglob"),
    ("trap.exit", r"\btrap\b.*\bEXIT\b"),
    ("trap.err", r"\btrap\b.*\bERR\b"),
    ("trap.pipe", r"\btrap\b.*\b(PIPE|SIGPIPE|13)\b"),
    ("trap.signal", r"\btrap\b.*\b(TERM|INT|HUP)\b"),
    ("trap.debug-return", r"\btrap\b.*\b(DEBUG|RETURN)\b"),
    ("jobs.background", r"[^&]&(\s|$)(?!&)"),
    ("jobs.wait", r"\bwait\b"),
    ("jobs.kill", r"\bkill\b"),
    ("child.shell", r"\b(sh|bash)\s+-\w*c\b"),
    ("child.exec", r"-exec\b|\bxargs\b"),
    ("tool.sibling", r"\bprobe-tool\b"),
]
_COMPILED = [(tag, re.compile(pattern, re.MULTILINE)) for tag, pattern in DETECTORS]

# Words that start a command: after a separator, a keyword, `!`, or at the start.
# The command word is matched ahead, not consumed, so a keyword such as `while` that is itself
# in command position still starts the next match (`; while getopts …` credits getopts).
_COMMAND_START = re.compile(
    r"(?:^|[;&|(){}`]|\$\(|\b(?:then|do|else|in|while|until|if|elif|time)\b|!)\s*"
    r"(?=(?:\w+=\S*\s+)*([.:\[]|[A-Za-z_][\w-]*)(?=\s|;|$|\)|&|\|))",
    re.MULTILINE,
)


def detect(script):
    """Features a script exercises: syntax detectors, plus builtins and commands it runs."""
    found = {tag for tag, pattern in _COMPILED if pattern.search(script)}
    builtins, commands = set(BUILTINS), set(COMMANDS)
    for match in _COMMAND_START.finditer(script):
        word = match.group(1)
        if word in builtins:
            found.add(f"builtin.{word}")
        elif word in commands:
            found.add(f"cmd.{word}")
    return found
