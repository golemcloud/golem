"""Extended globs built at run time and nested deeply, and `globstar` over a deep tree. A pattern
nested past the limit fails wherever it is matched, rather than matching wrongly.

Each case name starts with `pattern depth: `.
"""

CASES = [
    (
        "pattern depth: extended globs nested 24 deep",
        "shopt -s extglob; cd /tmp; touch a\n"
        "for kind in @ + '?' '*'; do p=$(printf \"$kind(%.0s\" {1..24})a$(printf ')%.0s' {1..24}); [[ a == $p ]]; "
        "echo \"$kind [[ $?\"; case a in $p) echo \"$kind case\";; esac; x=aa; echo \"$kind ${x#$p} ${x%%$p} ${x/$p/b}\"; "
        "echo \"$kind\" $p; done\n"
        "p=$(printf '@(%.0s' {1..12})a$(printf '|b)%.0s' {1..12}); [[ b == $p ]]; echo \"alt $?\"",
    ),
    (
        "pattern depth: nested negations",
        "shopt -s extglob; for n in 1 2 3 8 13; do p=$(printf '!(%.0s' $(seq $n))a$(printf ')%.0s' $(seq $n)); "
        "for s in a b ab ''; do [[ $s == $p ]]; printf %s $?; done; echo \" $n\"; done",
    ),
    (
        "pattern depth: extended globs nested deeper are refused",
        "shopt -s extglob; cd /tmp; touch a\n"
        "p=$(printf '@(%.0s' {1..25})a$(printf ')%.0s' {1..25})\n"
        "[[ a == $p ]]; echo \"[[ $?\"\n"
        "case a in $p) echo case;; esac\n"
        "x=aa; echo \"${x#$p}\"\n"
        "echo $p\n"
        "p=$(printf '@(%.0s' {1..1500})a$(printf ')%.0s' {1..1500})\n"
        "[[ a == $p ]]; echo \"[[ $?\"\n"
        "p=$(printf '!(%.0s' {1..20})abcdefgh$(printf ')%.0s' {1..20})\n"
        "[[ b == $p ]]; echo \"[[ $?\"\n"
        "echo after",
    ),
    (
        "pattern depth: globstar walks a deep tree",
        "cd /tmp && d=$(printf 'd/%.0s' {1..40}) && mkdir -p \"$d\" d/e d/.h && touch d/f \"${d}g\" d/e/x d/.h/y\n"
        "shopt -s globstar\n"
        "n=0; for f in d/**; do n=$((n+1)); done; echo \"all $n\"\n"
        "n=0; for f in d/**/; do n=$((n+1)); done; echo \"dirs $n\"\n"
        "set -- d/**/g; echo \"g $# ${#1}\"\n"
        "echo d/**/x d/**/y d/**/f\n"
        "mkdir -p t/a/b t/c && touch t/a/f t/c/g t/.z t/a/b/h; echo t/**; echo t/**/; echo t/**/h\n"
        "shopt -s dotglob; echo d/**/y; echo t/**",
    ),
]

NESTING = b"maximum nesting level exceeded: deeper nesting is unsupported in bash-tool\n"
EXPECTED = {
    "pattern depth: extended globs nested deeper are refused": (
        0,
        b"after\n",
        b"".join(b"bash: line %d: " % line + NESTING for line in (3, 4, 5, 6, 8, 10)),
        "bash-tool's limit: an extended glob is translated to a regular expression recursively, one or "
        "two groups per level, so one nested more than 24 deep, or a negation whose translation grows "
        "past 512 KiB, fails where it is matched and ends that command line (README, Limits); bash "
        "matches it",
    ),
}
