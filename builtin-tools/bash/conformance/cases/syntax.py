"""Syntax: comments, continuations, and how much of a script runs around a syntax error.

The tool checks the whole script before running it, so these cases show where that differs from
bash, which parses and runs one command at a time.
"""

CASES = [
    ("syntax: comments", 'echo a # trailing\n# whole line\necho b#not-a-comment; echo "#quoted"'),
    ("syntax: line continuation", 'echo a \\\nb; x=1\\\n2; echo $x'),
    ("syntax: empty script", ''),
    ("syntax: only comments", '# nothing\n  # here'),
    ("syntax: error on a later line", 'echo first\nif then\necho after', ["syntax.error"]),
    ("syntax: error on the same line", 'echo first; if then; echo after', ["syntax.error"]),
    ("syntax: unterminated quote", 'echo first\necho "open', ["syntax.error"]),
    ("syntax: unterminated substitution", 'echo first\necho $(echo x', ["syntax.error"]),
    ("syntax: unexpected token", 'echo first\n) oops\necho after', ["syntax.error"]),
    ("syntax: missing done", 'for i in 1; do echo $i', ["syntax.error"]),
    ("syntax: stray fi", 'echo a\nfi\necho b', ["syntax.error"]),
    ("syntax: error inside a function body", 'f() { echo in; }\nf\ng() { if; }\necho after', ["syntax.error"]),
    ("syntax: error inside eval", 'eval "echo ok; if"; echo status=$?; echo after', ["syntax.error"]),
    ("syntax: error in a trap body runs later", 'trap "if" EXIT; echo body', ["syntax.error", "trap.exit"]),
    ("syntax: deep nesting", 'echo $(echo $(echo $(echo $(echo $(echo $(echo deep))))))'),
    ("syntax: keywords as arguments", 'echo if then fi do done case esac in { }'),
    ("syntax: semicolons and newlines", 'echo a;echo b\n\n\necho c ;; echo d', ["syntax.error"]),
    ("syntax: reserved words need separators", 'if true; then echo t; fi; if true;then echo t2;fi'),
    ("syntax: function name validity", 'my-func() { echo dash; }; my-func; my.func() { echo dot; }; my.func'),
    ("syntax: long lines", 'x=' + 'a' * 5000 + '; echo ${#x}'),
]

WHOLE_SCRIPT = (
    "the tool checks the whole script before running any of it, "
    "so nothing runs; bash runs the commands before the error"
)
EXPECTED = {
    'syntax: error inside a function body': (
        2, b"", b"bash: -c: line 3: syntax error near unexpected token `;'\nbash: -c: line 3: `g() { if; }'\n", WHOLE_SCRIPT,
    ),
    'syntax: error on a later line': (
        2, b"", b"bash: -c: line 2: syntax error near unexpected token `then'\nbash: -c: line 2: `if then'\n", WHOLE_SCRIPT,
    ),
    'syntax: semicolons and newlines': (
        2, b"", b"bash: -c: line 4: syntax error near unexpected token `;;'\nbash: -c: line 4: `echo c ;; echo d'\n", WHOLE_SCRIPT,
    ),
    'syntax: stray fi': (
        2, b"", b"bash: -c: line 2: syntax error near unexpected token `fi'\nbash: -c: line 2: `fi'\n", WHOLE_SCRIPT,
    ),
    'syntax: unexpected token': (
        2, b"", b"bash: -c: line 2: syntax error near unexpected token `)'\nbash: -c: line 2: `) oops'\n", WHOLE_SCRIPT,
    ),
    'syntax: unterminated quote': (
        2, b"", b'bash: -c: line 2: unexpected EOF while looking for matching `"\'\n', WHOLE_SCRIPT,
    ),
    'syntax: unterminated substitution': (
        2, b"", b"bash: -c: line 3: unexpected EOF while looking for matching `)'\n", WHOLE_SCRIPT,
    ),
}
