"""Tilde expansion, with and without a home directory, and in assignment-like words.

A Golem agent starts with no HOME and has no passwd entry. Bash then expands `~` to `/`, and so
does this shell; `cd` with no operand still fails with `HOME not set`.

Outside POSIX mode, bash also expands a tilde after the first `=` (and after each `:`) of any
command-line word that looks like an assignment, as in `make DESTDIR=~/out`.
"""

CASES = [
    (
        "tilde: no home directory expands to /",
        "unset HOME; echo ~ ~/x \"~\"; d=~/y; echo $d; p=a:~:b; echo $p; cd ~; echo cd=$? $PWD; "
        "echo \"${HOME-unset}\"",
    ),
    (
        "tilde: cd with no HOME fails and the script continues",
        "unset HOME; cd /tmp; cd 2>/dev/null; echo status=$? $PWD",
    ),
    (
        "tilde: HOME set",
        "HOME=/tmp/h; mkdir -p ~/d; cd ~/d; pwd; echo ~ ~/x; cd; pwd; echo ~+ ~-",
    ),
    ("tilde: empty HOME", "HOME=; echo \"[$(echo ~)]\" \"[~/x]\"; x=~/y; echo \"[$x]\""),
    ("tilde: unknown user stays literal", "echo ~nosuchuser ~nosuchuser/x"),
    ("tilde: quoted and mid-word tildes stay literal", "HOME=/tmp/h; echo \"~\" '~' a~ \"~/x\""),
    ("tilde: assignment-like arguments", "HOME=/tmp/h; echo x=~/y x=~ x+=~ a[1]=~/z x=a:~/b:~ y=x=~"),
    (
        "tilde: arguments that are not assignment-like stay literal",
        "HOME=/tmp/h; echo --prefix=~/x 1x=~ x-y=~ \"x=~\" x=\"~\" x=a~ x=a:\"~\" {x=~,y}",
    ),
    (
        "tilde: assignment-like for, case and test words",
        "HOME=/tmp/h; for w in x=~ y=~/z; do echo \"$w\"; done; "
        "case x=/tmp/h in x=~) echo case-matched;; esac; "
        "[[ x=~ == x=/tmp/h ]] && echo double; [ x=~ = x=/tmp/h ] && echo single; "
        "f() { echo \"$1\"; }; f x=~; bash -c 'echo x=~/child'",
    ),
    (
        "tilde: values, array elements, parameter words and here-strings",
        "HOME=/tmp/h; y=x=~:~; echo \"$y\"; a=(x=~ q); echo \"${a[0]}\"; echo \"${u:-x=~}\"; "
        "cat <<< x=~",
    ),
    (
        "tilde: assignment-like redirect target",
        "HOME=/tmp/h; cd /tmp; mkdir -p x=/tmp/h; echo hi > x=~/f; cat x=/tmp/h/f",
    ),
    ("tilde: POSIX mode keeps arguments literal", "HOME=/tmp/h; set -o posix; echo x=~/y; export Z=~/z; echo $Z"),
]
