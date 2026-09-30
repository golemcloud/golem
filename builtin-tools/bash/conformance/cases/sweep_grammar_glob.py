"""Grammar sweep: pathname expansion on files the script creates, glob options, and pattern
matching in [[ ]] and case, generated from fixed tables (see sweep_grammar_param.py)."""

TIER = "sweep"

CASES = []
_NAMES = []


def _add(name, script, tags=()):
    name = "gram glob " + name
    if name in _NAMES:
        raise ValueError("duplicate case name " + name)
    _NAMES.append(name)
    CASES.append((name, script, list(tags)))


# A fixed tree under /tmp/g. Names cover case, dots, spaces, brackets and glob characters.
TREE = (
    "mkdir -p /tmp/g/d/e /tmp/g/D2 && cd /tmp/g && for f in a b c ab abc a.txt b.txt c.log .hid "
    ".hid.txt Q Z Xy 'sp ace' '[x]' 'a*' 'q?' ']' - d/f d/.g d/e/h D2/i; do : > \"$f\"; done; "
)
P = "p() { printf '%s' \"$#\"; printf ' <%s>' \"$@\"; echo; }; "

PATTERNS = [
    "*", "?", "??", "a*", "*c", "*.txt", "[ab]", "[!a]", "[^a]", "[a-c]", "[]]", "[!]a]", "[[:upper:]]",
    "[[:lower:]]*", "[[:alpha:]][[:alpha:]]", "*[[:space:]]*", "*[[:punct:]]*", ".*", "*/", "*/*", "d/*",
    "d/*/*", "\\*", "a\\*", "'a*'", "\"a\"*", "[x]", "\\[x]", "[[]x]", "[", "[a", "*[", "/tmp/g/a*", "./a*",
    "../g/a*", "a*/", "nomatch*", "d*/", "*.{txt,log}", "[a-", "[z-a]", "[[:nosuch:]]", "*\\?", "q[?]",
    "[-]", "[a-]", "*/.*", ".", "..", "*.", "[.]hid", "?hid",
]
GLOB_OPTIONS = [
    ("default", ""),
    ("nullglob", "shopt -s nullglob; "),
    ("dotglob", "shopt -s dotglob; "),
    ("nocaseglob", "shopt -s nocaseglob; "),
]
for pattern in PATTERNS:
    for oname, opt in GLOB_OPTIONS:
        if oname != "default" and (len(pattern) + len(oname)) % 3:
            continue
        _add("pattern " + pattern + ": " + oname, TREE + P + opt + "p " + pattern,
             ["expansion.glob"] + (["expansion.glob.options"] if opt else []))

# failglob stops the command, and in a non-interactive shell the rest of the line.
for pattern in ["nomatch*", "a*", "[z]", "*.none", "d/*", "x?y", "\\*", "nomatch*.txt"]:
    _add("failglob " + pattern,
         TREE + "shopt -s failglob; echo before; echo " + pattern + "; echo \"after status=$?\"\necho next line",
         ["expansion.glob.options"])
for label, script in [
    ("in a for list", "shopt -s failglob; for f in nomatch*; do echo $f; done; echo status=$?"),
    ("in an array assignment", "shopt -s failglob; a=(nomatch*); echo status=$? ${#a[@]}"),
    ("in a subshell", "shopt -s failglob; ( echo nomatch* ); echo status=$?"),
    ("with nullglob", "shopt -s failglob nullglob; echo x nomatch* y; echo status=$?"),
    ("in a redirection", "shopt -s failglob; echo x > nomatch*; echo status=$?"),
    ("in a function", "shopt -s failglob; f() { echo nomatch*; echo in-f; }; f; echo status=$?"),
]:
    _add("failglob " + label, TREE + script, ["expansion.glob.options"])

# globstar.
for pattern in ["**", "**/", "**/h", "d/**", "d/**/", "**/*.txt", "**/e/*", "a**", "**x", "d/**/h", "**/.g", "*/**"]:
    for dot in ("", "shopt -s dotglob; "):
        if dot and len(pattern) % 2:
            continue
        _add("globstar " + pattern + (" dotglob" if dot else ""),
             TREE + P + "shopt -s globstar; " + dot + "p " + pattern, ["expansion.glob.options"])
    _add("globstar off " + pattern, TREE + P + "p " + pattern, ["expansion.glob"])

# noglob and GLOBIGNORE.
for pattern in ["*", "a?", "[ab]", "*.txt", "d/*"]:
    _add("noglob " + pattern, TREE + P + "set -f; p " + pattern + "; set +f; p " + pattern,
         ["option.noglob"])
for ignore in ["*.txt", "a*:b*", ".*", "", "d/*", "*", "?", "[ab]"]:
    for pattern in ["*", "a*", "d/*"]:
        if ignore == "" and pattern != "*":
            continue
        _add("GLOBIGNORE '" + ignore + "': " + pattern,
             TREE + P + "GLOBIGNORE='" + ignore + "'; p " + pattern,
             ["expansion.glob.options"])
_add("GLOBIGNORE unset restores dot files", TREE + P + "GLOBIGNORE=x; p .h*; unset GLOBIGNORE; p .h*", ["expansion.glob.options"])
_add("GLOBIGNORE set implies dotglob", TREE + P + "GLOBIGNORE=a; p *hid*", ["expansion.glob.options"])

# Glob results in different contexts.
for label, script in [
    ("in a for list", "for f in a*; do printf '<%s>' \"$f\"; done; echo"),
    ("in an array", "arr=(*.txt); echo ${#arr[@]} \"${arr[1]}\""),
    ("in an assignment is literal", "v=a*; echo \"$v\""),
    ("in declare is literal", "declare v=a*; echo \"$v\""),
    ("in a case word is literal", "case a* in 'a*') echo literal;; *) echo other;; esac"),
    ("in [[ is literal", "[[ a* == 'a*' ]] && echo literal"),
    ("in a here-string is literal", "cat <<< a*"),
    ("after variable expansion", "v='*.txt'; echo $v \"$v\""),
    ("after command substitution", "echo $(echo '*.log')"),
    ("in a redirect with one match", "echo hi > c.l*; cat c.log"),
    ("in a redirect with two matches", "echo hi > *.txt; echo status=$?"),
    ("in a redirect with no match", "echo hi > zz*; cat 'zz*'"),
    ("space in a match", "for f in sp*; do echo \"[$f]\"; done"),
    ("dash file name", "echo - *-*"),
    ("sorted order", "echo *"),
    ("directories only", "echo */"),
    ("absolute with dotdot", "echo /tmp/g/d/../a*"),
    ("in a function argument", "f() { echo $#; }; f *"),
    ("quoted parts", "echo \"a\"* 'a'\\* a\"*\""),
    ("brace then glob", "echo {a,b}*"),
    ("tilde then glob", "HOME=/tmp/g; echo ~/a*"),
    ("escaped in variable", "v='a\\*'; echo $v"),
    ("bracket in variable", "v='[ab]'; echo $v \"$v\""),
    ("unset variable pattern", "unset u; echo ${u}*"),
    ("parameter default pattern", "unset u; echo ${u:-a}*"),
    ("glob in a loop over a directory", "for d in */; do echo \"dir:$d\"; done"),
    ("match count", "set -- *; echo $#"),
    ("hidden with dot star", "set -- .*; echo $# \"$@\""),
    ("nullglob array empty", "shopt -s nullglob; arr=(zz*); echo ${#arr[@]}"),
    ("nullglob unquoted variable", "shopt -s nullglob; v='zz*'; set -- $v; echo $#"),
    ("nocaseglob brackets", "shopt -s nocaseglob; echo [a]*"),
    ("nocaseglob lower matches upper", "shopt -s nocaseglob; echo q* x? [z] D*"),
    ("globasciiranges", "echo [A-b]; shopt -u globasciiranges; echo [A-b]; shopt -s globasciiranges"),
    ("GLOBSORT by name reversed", "GLOBSORT=-name; echo a*"),
    ("GLOBSORT nosort is stable", "GLOBSORT=name; echo [abc]"),
    ("nested directory", "echo d/e/*"),
    ("pattern with slash in brackets", "echo d[/]f"),
    ("star matches no leading dot in subdir", "echo d/*"),
    ("dot slash prefix is kept", "echo ./[ab]"),
    ("double slash is kept", "echo d//f"),
]:
    _add("context " + label, TREE + script, ["expansion.glob"])

# --- extglob, enabled on an earlier line -----------------------------------------------------

EXTGLOB = [
    "@(a|b)", "!(a*)", "*(a)", "+(ab)", "?(a)c", "@(a|b).txt", "!(*.txt|*.log)", "*(a|b)c", "+(a|b|c)",
    "a!(b)", "!(a)*", "@(ab|abc)", "*.@(txt|log)", "!(*)", "@()", "?(|a)b", "!(.hid)", "@(\\*|b)",
    "+([[:upper:]])", "*([!a])", "d/@(e|f)", "@(d|D2)/*",
]
for pattern in EXTGLOB:
    _add("extglob " + pattern, "shopt -s extglob\n" + TREE + P + "p " + pattern, ["expansion.glob.options"])
    _add("extglob match " + pattern,
         "shopt -s extglob\nfor s in a b ab abc a.txt c.log aab ''; do [[ $s == " + pattern + " ]] && printf '%s ' \"[$s]\"; done; echo",
         ["expansion.glob.options", "compound.cond"])
for pattern in ["@(a|b)", "!(a*)", "*.@(txt|log)", "+(a)"]:
    _add("extglob with dotglob " + pattern, "shopt -s extglob dotglob\n" + TREE + P + "p " + pattern,
         ["expansion.glob.options"])
    _add("extglob in trim " + pattern, "shopt -s extglob\nv=aab.txt; echo ${v##" + pattern + "} ${v%%" + pattern + "}",
         ["expansion.glob.options", "expansion.parameter.trim"])
    _add("extglob in replace " + pattern, "shopt -s extglob\nv='a b.txt ab'; echo \"${v//" + pattern + "/X}\"",
         ["expansion.glob.options", "expansion.parameter.replace"])
    _add("extglob in case " + pattern, "shopt -s extglob\nfor s in a ab x.txt; do case $s in " + pattern + ") echo \"$s yes\";; *) echo \"$s no\";; esac; done",
         ["expansion.glob.options", "compound.case"])
_add("extglob turned off again", "shopt -s extglob\nshopt -u extglob\n" + TREE + "echo *.txt", ["expansion.glob.options"])

# --- Pattern matching in [[ ]] and case -------------------------------------------------------

MATCH_PATTERNS = [
    "a*", "*b", "?b", "[ab]*", "[!a]*", "[[:digit:]]*", "*[[:space:]]*", "'a*'", "\"a\"*", "a\\*",
    "$pat", "\"$pat\"", "*", "", "[]a]*", "[a-]*", "*.*", "\\[*", "[[:alpha:][:digit:]]*", "[^[:alpha:]]*",
]
SUBJECTS = ["a*", "ab", "b", "1x", "a b", "", "[x", "-y", "a.b", "]z"]
for pattern in MATCH_PATTERNS:
    body = "; ".join(
        "[[ " + ("$s" if k % 2 else "\"$s\"") + " == " + (pattern or "''") + " ]] && m=\"$m|$s\""
        for k in range(1))
    _add("match [[ ]] " + (pattern or "empty"),
         "pat='a*'; m=; for s in " + " ".join("'" + s + "'" for s in SUBJECTS) + "; do " + body + "; done; echo \"$m\"",
         ["compound.cond"])
    _add("match case " + (pattern or "empty"),
         "pat='a*'; m=; for s in " + " ".join("'" + s + "'" for s in SUBJECTS) + "; do case $s in " + (pattern or "''") + ") m=\"$m|$s\";; esac; done; echo \"$m\"",
         ["compound.case"])
for pattern in ["A*", "[a]*", "?B", "'AB'"]:  # the subjects differ only in case: no files involved
    _add("match nocasematch [[ ]] " + pattern,
         "shopt -s nocasematch; for s in ab AB Ab x; do [[ $s == " + pattern + " ]] && echo \"$s\"; done",
         ["option.nocasematch"])
    _add("match nocasematch case " + pattern,
         "shopt -s nocasematch; for s in ab AB Ab x; do case $s in " + pattern + ") echo \"$s\";; esac; done",
         ["option.nocasematch"])
    _add("match nocasematch regex " + pattern,
         "shopt -s nocasematch; p=" + pattern.replace("*", ".*").replace("?", ".") + "; for s in ab AB x; do [[ $s =~ ^$p$ ]] && echo \"$s\"; done",
         ["option.nocasematch", "compound.cond.regex"])
