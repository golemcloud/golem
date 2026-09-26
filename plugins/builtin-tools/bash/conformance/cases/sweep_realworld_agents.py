"""Real-world usage: the shell command shapes coding agents ran in public, permissively licensed
trajectories, replayed against a fixture repository.

Sources (see ../NOTICE): SWE-bench/SWE-smith-trajectories (MIT, Hugging Face revision
08e109b4a59eaeebf80e4675cd125d42e7ac99a4, split `tool`, 800 trajectories sampled at offsets
0-21099) and SWE-Gym/OpenHands-SFT-Trajectories (MIT, revision
4aaa5a4a4b5861f4799d2336908760c190ac3b17, all 491 trajectories). About 4,500 shell commands were
extracted and grouped by shape (options and pipeline structure, with paths, patterns and numbers
abstracted). Each shape that uses only commands this shell has is replayed here once per binding:
a hit, a miss, a missing path, and where it matters a pattern with regex metacharacters. Repository
names, paths and identifiers from the trajectories are replaced by a small invented Python project
under /testbed (the SWE-bench convention); no code, data or text from the trajectories is kept.
Commands for tools this shell lacks (python, pip, git, ps, chmod, awk) are left out.
"""

TIER = "sweep"

REPO = (
    "mkdir -p /testbed/src/pkg/__pycache__ /testbed/src/pkg/sub /testbed/tests /testbed/docs && cd /testbed\n"
    "printf 'from setuptools import setup\\nsetup(name=\"pkg\")\\n' >setup.py\n"
    "printf '# pkg\\n\\nA small package. TODO: docs\\n' >README.md\n"
    "printf '\"\"\"Package.\"\"\"\\nfrom .core import Widget\\n' >src/pkg/__init__.py\n"
    "printf 'import os\\n\\n\\nclass Widget:\\n    def __init__(self, value):\\n        self._value = value\\n\\n"
    "    def decompress(self, value):\\n        if value is None:\\n            return [None, None]\\n"
    "        return [value.amount, value.currency]\\n\\n    def parse_form(self, data):\\n"
    "        # TODO: validate \"%s\" * 2\\n        return data\\n\\n\\ndef helper(x):\\n    return x * 2\\n' >src/pkg/core.py\n"
    "printf 'def slugify(s):\\n    return s.lower().replace(\" \", \"-\")\\n\\n\\ndef forward_port(port):\\n"
    "    listen_sock = None\\n    return port\\n' >src/pkg/utils.py\n"
    "printf 'CONSTANT = 1\\n' >src/pkg/sub/consts.py\n"
    "printf '\\000\\001pyc\\000Widget' >src/pkg/__pycache__/core.cpython-311.pyc\n"
    "printf 'from pkg.core import Widget\\n\\n\\ndef test_decompress():\\n    assert Widget(1).decompress(None) == [None, None]\\n' >tests/test_core.py\n"
    "printf 'from pkg.utils import slugify\\n\\n\\ndef test_slugify():\\n    assert slugify(\"A B\") == \"a-b\"\\n' >tests/test_utils.py\n"
    "printf '# Docs\\n\\nUse Widget.decompress.\\n' >docs/index.md"
)

CASES = []


def add(slug, body, tags=()):
    CASES.append(("real traj: " + slug, REPO + "\n" + body, tags))


# grep -n PATTERN FILE: the single most common shape.
for _label, _cmd in (
    ("a class", 'grep -n "class Widget" /testbed/src/pkg/core.py'),
    ("no match", 'grep -n "class Missing" /testbed/src/pkg/core.py; echo "status=$?"'),
    ("a missing file", 'grep -n "class Widget" /testbed/src/pkg/nope.py; echo "status=$?"'),
    ("a directory", 'grep -n "Widget" /testbed/src/pkg; echo "status=$?"'),
    ("a relative path", 'cd /testbed && grep -n "def " src/pkg/core.py'),
    ("an escaped dot", 'grep -n "self\\._value" /testbed/src/pkg/core.py'),
    ("an alternation", 'grep -n "decompress\\|parse_form" /testbed/src/pkg/core.py'),
    ("quotes and a star", "grep -n '\".*\" \\* ' /testbed/src/pkg/core.py"),
    ("a bracket expression", 'grep -n "%[rs]" /testbed/src/pkg/core.py'),
    ("a single-quoted apostrophe", "grep -n \"can't\" /testbed/src/pkg/core.py; echo \"status=$?\""),
    ("a glob of files", 'grep -n "import" /testbed/src/pkg/*.py'),
    ("a binary file", 'grep -n "Widget" /testbed/src/pkg/__pycache__/core.cpython-311.pyc'),
):
    add("grep -n with " + _label, _cmd, ("cmd.grep",))

# grep -n ... | head / | grep -i
add("grep -n piped to head", 'grep -n "self" /testbed/src/pkg/core.py | head -2')
add("grep -n piped to head -n", 'grep -n "return" /testbed/src/pkg/core.py | head -n 1')
add("grep -n piped to grep -i", 'grep -n "_" /testbed/src/pkg/utils.py | grep -i forward')
add("grep -n piped to grep -i with no match", 'grep -n "_" /testbed/src/pkg/utils.py | grep -i tcp; echo "status=$?"')
add("grep -n piped to grep -E and head", 'grep -n "def" /testbed/src/pkg/core.py | grep -E "parse|help" | head -5')

# Recursive greps.
for _label, _cmd in (
    ("-rn in the repo", 'grep -rn "decompress" /testbed | sort'),
    ("-rn in a subdirectory", 'grep -rn "slugify" /testbed/tests'),
    ("-rn with no match", 'grep -rn "nothing_here" /testbed; echo "status=$?"'),
    ("-r in the repo", 'grep -r "def helper" /testbed'),
    ("-r with a trailing slash", 'grep -r "Widget" /testbed/tests/ | sort'),
    ("-nr", 'grep -nr "import" /testbed/src | sort'),
    ("-r -n", 'grep -r -n "TODO" /testbed | sort'),
    ("-nR", 'grep -nR "CONSTANT" /testbed'),
    ("-Rn", 'grep -Rn "listen_sock" /testbed/src'),
    ("-rl", 'grep -rl "Widget" /testbed/src | sort'),
    ("-rnw with -e", "cd /testbed && grep -rnw /testbed/src -e 'helper'"),
    ("-rnw matching only whole words", "grep -rnw /testbed/src -e 'help'; echo \"status=$?\""),
    ("-n -r after the path", 'grep -n "port" /testbed/src -r | sort | head -10'),
    ("--include", 'cd /testbed && grep -r "Widget" --include="*.py" . | sort'),
    ("--include with a path", 'grep -r "TODO" --include="*.md" /testbed'),
    ("--include then grep -v", 'grep -r "Widget" --include="*.py" /testbed | grep -v "tests/" | sort'),
    ("--include then two grep -v", 'grep -r "import" /testbed --include="*.py" | grep -v "__init__" | grep -v "test" | sort'),
    ("--include with context", 'cd /testbed && grep -r "def slugify" --include="*.py" ./src -A 1 -B 1'),
    ("--include and grep -i and head", 'grep -r "def" --include="*.py" /testbed | grep -i "slug\\|port" | head -20 | sort'),
    ("-Ern with alternation", 'grep -Ern "slugify|helper" /testbed/src | sort'),
    ("-En with alternation", 'grep -En "^(class|def) " /testbed/src/pkg/core.py'),
    ("-r in a missing directory", 'grep -rn "x" /testbed/nope; echo "status=$?"'),
    ("-r over the current directory", 'cd /testbed/tests && grep -r "assert" . | sort'),
    ("-r with piped head", 'grep -r "return" /testbed/src/ | sort | head -3'),
):
    add("grep " + _label, _cmd, ("cmd.grep",))

# Context greps.
for _label, _cmd in (
    ("-n -A", 'grep -n -A 3 "def decompress" /testbed/src/pkg/core.py'),
    ("-n with -A after the pattern", 'grep -n "def parse_form" -A 2 /testbed/src/pkg/core.py'),
    ("-A", 'grep -A 2 "class Widget" /testbed/src/pkg/core.py'),
    ("-A10 attached", 'grep -A10 "def forward_port" /testbed/src/pkg/utils.py'),
    ("-B", 'grep -B 2 "return data" /testbed/src/pkg/core.py'),
    ("-n -B", 'grep -n -B 1 "def helper" /testbed/src/pkg/core.py'),
    ("-A -B", 'grep -A 1 -B 1 "_value = value" /testbed/src/pkg/core.py'),
    ("-B -A", 'grep -B 1 -A 1 "return x" /testbed/src/pkg/core.py'),
    ("-n -A -B", 'grep -n -A 2 -B 2 "if value is None" /testbed/src/pkg/core.py'),
    ("-n with -B and -A after the pattern", 'grep -n "return" -B 1 -A 1 /testbed/src/pkg/core.py'),
    ("-A -n", 'grep -A 1 -n "def " /testbed/src/pkg/utils.py'),
    ("context with overlapping groups", 'grep -n -A 1 "def" /testbed/src/pkg/core.py'),
    ("context piped to head", 'grep -A 15 "class Widget" /testbed/src/pkg/core.py | head -n 5'),
    ("context piped to grep -A", 'grep -n -A 10 "class Widget" /testbed/src/pkg/core.py | grep -A 2 "decompress"'),
    ("context piped to grep -n", 'grep -n -A 20 "class Widget" /testbed/src/pkg/core.py | grep -n "def"'),
    ("context piped to context and head", 'grep -A 10 "def " /testbed/src/pkg/core.py | grep -B 1 -A 2 "if value" | head -20'),
    ("context of a missing pattern", 'grep -A 3 "nope" /testbed/src/pkg/core.py; echo "status=$?"'),
    ("large context piped to head", 'grep -n "class Widget" -A 200 /testbed/src/pkg/core.py | head -8'),
):
    add("grep context " + _label, _cmd, ("cmd.grep",))

# find | grep | sort shapes.
for _label, _cmd in (
    ("files minus pycache sorted", 'find /testbed -type f -name "*.py" | grep -v "__pycache__" | sort'),
    ("files minus tests", 'find /testbed -type f -name "*.py" | grep -v "test" | sort'),
    ("files minus test_ prefix", 'find /testbed -type f -name "*.py" | grep -v "test_" | sort'),
    ("files minus pycache and tests sorted", 'find /testbed -type f -name "*.py" | grep -v "__pycache__" | grep -v "test_" | sort'),
    ("files minus pycache and tests with head", 'find /testbed -type f -name "*.py" | grep -v "__pycache__" | grep -v "test_" | sort | head -2'),
    ("files grep -i", 'find /testbed -type f -name "*.py" | grep -i "CORE"'),
    ("files grep -i with alternation", 'find /testbed -type f -name "*.py" | grep -i "core\\|util" | sort'),
    ("files grep -i with no match or true", 'find /testbed -type f -name "*.py" | grep -i "nomatch" || true'),
    ("files grep -i then grep -v", 'find /testbed -type f -name "*.py" | grep -i "test" | grep -v utils | sort'),
    ("by name", 'find /testbed -name "core.py"'),
    ("by name grep -E", 'find /testbed -name "*.py" | grep -E "src/pkg/[a-z]+\\.py" | sort'),
    ("by name with a missing root", 'find /testbed/nope -name "*.py"; echo "status=$?"'),
    ("by name with no match", 'find /testbed -name "nothing.py" | grep -i x; echo "status=$?"'),
    ("all files", 'find /testbed -type f | sort'),
    ("all files sorted in a subdirectory", 'find /testbed/src/pkg -type f -name "*.py" | sort'),
    ("directories by name", 'find /testbed -name "pkg" -type d'),
    ("directory type first", 'find /testbed -type d -name "tests"'),
    ("path pattern directories", 'find /testbed -path "*/src/*" -type d | sort'),
    ("path pattern and name", 'find /testbed -path "*pkg*" -name "*.py" | grep -i sub'),
    ("directories with -o", 'find /testbed -type d -name "tests" -o -name "docs" | sort'),
    ("relative find", 'cd /testbed && find . -name "*.md" | sort'),
    ("relative find by type", 'cd /testbed && find src -type f -name "*.py" | grep -i init'),
):
    add("find " + _label, _cmd, ("cmd.find",))

# find -exec / xargs grep shapes.
for _label, _cmd in (
    ("exec grep -l", 'find /testbed -type f -exec grep -l "Widget" {} \\; | sort'),
    ("exec grep -l on python files", 'find /testbed -type f -name "*.py" -exec grep -l "slugify" {} \\; | sort'),
    ("exec grep -l relative", 'cd /testbed && find . -type f -exec grep -l "TODO" {} \\; | sort'),
    ("exec grep -l with no match", 'find /testbed -name "*.py" -exec grep -l "nomatch" {} \\;; echo "status=$?"'),
    ("exec grep -n", 'find /testbed/src -type f -name "*.py" -exec grep -n "return" {} \\; | sort'),
    ("exec grep -Hn", 'find /testbed -type f -name "*.py" -exec grep -Hn "def test" {} \\; | sort'),
    ("exec grep with context", 'find /testbed/src -type f -name "utils.py" -exec grep -A 1 -B 1 "listen" {} \\;'),
    ("exec file", 'find /testbed/src -type f -exec file {} \\; | grep "text" | sort'),
    ("xargs grep -l", 'find /testbed -name "*.py" | xargs grep -l "Widget" | sort'),
    ("xargs grep -l with head", 'find /testbed -type f -name "*.py" | xargs grep -l "def" | sort | head -5'),
    ("xargs grep -l on a class", 'find /testbed -name "*.py" | xargs grep -l "class Widget"'),
    ("xargs grep -l with a regex", 'find /testbed -type f -name "*.py" | xargs grep -l "class.*Widget\\|def slug" | sort'),
    ("xargs grep -l with no match", 'find /testbed -type f -name "*.py" | xargs grep -l "nomatch"; echo "status=$?"'),
    ("xargs grep -l with no match or true", 'find /testbed -type f -name "*.py" | grep -v "test_" | xargs grep -l "nomatch" || true'),
    ("minus pycache xargs grep -l sorted", 'find /testbed -type f -name "*.py" | grep -v "__pycache__" | xargs grep -l "import" | sort'),
    ("path xargs grep -l", 'find /testbed -path "*/pkg/*" -type f -name "*.py" | xargs grep -l "def" | sort'),
    ("xargs ls", 'find /testbed -name "utils.py" | xargs ls'),
    ("xargs cat", 'find /testbed/src/pkg/sub -type f -name "*.py" | xargs cat'),
    ("xargs cat grep -A", 'find /testbed/src/pkg -name "core.py" | xargs cat | grep -A 2 "def helper"'),
    ("xargs with no input", 'find /testbed -name "none.py" | xargs grep -l "x"; echo "status=$?"'),
    ("for loop over find exec", 'cd /testbed && for f in $(find src -type f -name "*.py" -exec grep -l "return" {} \\; | sort); do echo "=== $f ==="; grep -A 1 "return" "$f"; done'),
):
    add("find " + _label, _cmd, ("cmd.find",))

# ls shapes.
for _label, _cmd in (
    ("ls -R of the repo", "ls -R /testbed"),
    ("ls -R of a subdirectory", "ls -R /testbed/src"),
    ("ls -R relative", "cd /testbed && ls -R docs/"),
    ("ls -a", "ls -a /testbed"),
    ("ls of a directory", "ls /testbed"),
    ("ls of a subdirectory", "ls /testbed/tests"),
    ("ls of the current directory", "cd /testbed/src/pkg && ls"),
    ("ls of a missing directory with a fallback", 'ls -a /testbed/nope 2>/dev/null || echo "Directory not found"'),
    ("ls of files that exist", "cd /testbed && ls setup.py README.md"),
    ("ls of a missing file", "cd /testbed && ls setup.py missing.png; echo \"status=$?\""),
    ("ls glob of files", "cd /testbed/tests && ls test_*.py"),
    ("ls glob with no match", "cd /testbed && ls qr_*.png; echo \"status=$?\""),
    ("ls -R of a missing path", "ls -R /testbed/nope; echo \"status=$?\""),
):
    add(_label, _cmd, ("cmd.ls",))

# rm, cp, mv, mkdir shapes.
REPRO = "cd /testbed && printf 'print(1)\\n' >reproduce_error.py && printf 'print(2)\\n' >test_edge_cases.py"
for _label, _cmd in (
    ("rm one file", REPRO + " && rm /testbed/reproduce_error.py && ls /testbed"),
    ("rm two files", REPRO + " && rm /testbed/reproduce_error.py /testbed/test_edge_cases.py && ls /testbed"),
    ("rm a file that is gone", REPRO + " && rm /testbed/reproduce_error.py /testbed/reproduce_issue.py; echo \"status=$?\"; ls /testbed"),
    ("rm -f several", REPRO + " && rm -f reproduce_error.py test_edge_cases.py missing.png && ls"),
    ("rm -f with a glob", REPRO + " && : >a.png && : >b.png && rm -f *.png reproduce_error.py && ls"),
    ("rm -rf several", REPRO + " && mkdir -p build/x test-dir && rm -rf reproduce_error.py build/ test-dir/ nope && ls"),
    ("rm -rf a directory", "rm -rf /testbed/src/pkg/__pycache__ && ls -a /testbed/src/pkg"),
    ("cp a backup", "cd /testbed && cp src/pkg/core.py src/pkg/core.py.bak && ls src/pkg"),
    ("cp a new file over the original", "cd /testbed && printf 'X = 2\\n' >/testbed/utils.py.new && cp /testbed/utils.py.new /testbed/src/pkg/utils.py && cat src/pkg/utils.py"),
    ("mv a file into place", "cd /testbed && printf 'X = 3\\n' >src/pkg/core.py.new && mv src/pkg/core.py.new src/pkg/core.py && cat src/pkg/core.py"),
    ("mkdir -p a nested directory", "mkdir -p /testbed/reproduce_bug/conf/platform && find /testbed/reproduce_bug | sort"),
    ("mkdir and cd", "cd /testbed && mkdir test_repo && cd test_repo && pwd"),
    ("mkdir an existing directory", "cd /testbed && mkdir tests; echo \"status=$?\""),
    ("rewrite a file with grep -v", "cd /testbed && cp src/pkg/core.py core.bak && grep -v \"^def helper\" core.bak > core.new && cat core.new | tail -n 3 && wc -l core.bak core.new"),
    ("rebuild a file from two parts", "cd /testbed && grep -A 2 \"def helper\" src/pkg/core.py > part.new && grep -v \"helper\" src/pkg/core.py | grep -v \"return x\" > rest.new && cat part.new >> rest.new && tail -n 4 rest.new"),
):
    add(_label, _cmd)

# cat, head, tail, wc shapes.
for _label, _cmd in (
    ("cat a file", "cat /testbed/setup.py"),
    ("cat two files", "cd /testbed && cat setup.py docs/index.md"),
    ("cat a missing file", "cat /testbed/reproduce_output.log; echo \"status=$?\""),
    ("cat a dotfile", "cd /testbed && printf 'KEY=value\\n' >.env && cat .env"),
    ("cat piped to grep -A", "cat /testbed/src/pkg/core.py | grep -A 2 \"def __init__\""),
    ("head -n", "head -n 5 /testbed/src/pkg/core.py"),
    ("head -n of a short file", "head -n 20 /testbed/src/pkg/sub/consts.py"),
    ("tail -n", "tail -n 3 /testbed/src/pkg/core.py"),
    ("tail -n of a log", "cd /testbed && seq 1 30 | sed 's/^/log line /' >repro_script.log && tail -n 20 /testbed/repro_script.log | head -n 2"),
    ("tail piped to grep and head", "tail -n 100 /testbed/src/pkg/core.py | grep -A 2 \"def \" | head -5"),
    ("wc -l", "wc -l /testbed/src/pkg/core.py"),
    ("wc -l of several", "cd /testbed && wc -l src/pkg/*.py"),
    ("ls and head", "cd /testbed && ls README.md && head -n 1 README.md"),
    ("cat -n", "cat -n /testbed/src/pkg/utils.py"),
    ("sed -n a range", "sed -n '4,8p' /testbed/src/pkg/core.py"),
):
    add(_label, _cmd)

# Writing files the way agents do.
for _label, _cmd in (
    ("echo with backslash-n into a file", 'cd /testbed && echo "import os\\n\\nprint(os.sep)" > reproduce.py && cat reproduce.py'),
    ("echo -e into a file", 'cd /testbed && echo -e "import os\\n\\nprint(os.sep)" > reproduce.py && cat reproduce.py'),
    ("echo a multi-line single-quoted string", "cd /testbed && echo '\nimport sys\n\ndef main():\n    print(\"hi\")\n' > reproduce.py && cat -A reproduce.py"),
    ("echo a script with a main guard", "cd /testbed && echo 'if __name__ == \"__main__\":\n    main()' > /testbed/run.py && cat /testbed/run.py"),
    ("cat heredoc into a file", "cat > /testbed/reproduce.py << 'EOF'\nfrom pkg.core import Widget\nprint(Widget(1).decompress(None))\nEOF\ncat /testbed/reproduce.py"),
    ("cat heredoc with expansion", "name=Widget\ncat > /testbed/r.py <<EOF\nfrom pkg.core import $name\nprint(\"\\$HOME\")\nEOF\ncat /testbed/r.py"),
    ("printf into a file", "printf '%s\\n' 'line one' 'line two' > /testbed/notes.txt && cat /testbed/notes.txt"),
    ("tee into a file", "echo 'X=1' | tee /testbed/config.env && cat /testbed/config.env"),
    ("append with echo", "echo 'extra = True' >> /testbed/setup.py && tail -n 2 /testbed/setup.py"),
):
    add(_label, _cmd)

# Control shapes around commands.
for _label, _cmd in (
    ("cd then grep", "cd /testbed && grep -n \"import\" setup.py"),
    ("cd into a missing directory then a command", "cd /testbed/nope && ls; echo \"status=$?\""),
    ("pwd", "cd /testbed/src && pwd"),
    ("sleep then a command", "sleep 0.1 && cd /testbed && cat setup.py | head -n 1"),
    ("command or echo", "grep -q \"nope\" /testbed/setup.py || echo \"not found\""),
    ("background job then kill", "sleep 5 & sleep 0.1; kill %1; wait; echo \"status=$?\""),
    ("background job then kill by pid", "sleep 5 & pid=$!; kill $pid; wait $pid 2>/dev/null; echo \"status=$?\""),
    ("an environment prefix", "cd /testbed && PYTHONPATH=/testbed/src env | grep '^PYTHONPATH='"),
    ("a missing tool", "cd /testbed && mypy reproduce.py; echo \"status=$?\""),
    ("a missing tool with a fallback", "cd /testbed && (pytest -q 2>/dev/null || echo 'pytest unavailable')"),
    ("a pipeline into a missing tool", "cd /testbed && cat setup.py | python3 -c 'import sys'; echo \"status=$?\""),
):
    add(_label, _cmd)


# Dropped after recording, with the reason for each group.
# GNU quotes the name with curly quotes under the oracle's UTF-8 locale and nothing else differs: the documented
# deliberate difference UTF8_QUOTES (env_facts.py, command_errors.py).
_DROPPED_Q = (
    'real traj: mkdir an existing directory',
)
_DROPPED = set(_DROPPED_Q)
CASES = [case for case in CASES if case[0] not in _DROPPED]
