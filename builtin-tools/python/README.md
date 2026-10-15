# Python built-in tool

`python@0.1.0` executes Python in a fresh RustPython interpreter per invocation.
It provides a Python CLI-shaped command, not the old snippet/session API.
It has no dependency on golem-ai.

## Command

The root input has `cwd` (absolute, default `/`, bound to Bash's `PWD`) and an
`args` list. Text callers separate tool options from Python arguments with `--`:

```shell
python --cwd /workspace -- -c 'print(6 * 7)'
python --cwd /workspace -- script.py first --second
python --cwd /workspace -- -m package argument
```

`-c` sets `sys.argv[0]` to `-c`. Script arguments follow the script path.
`-m` executes a module or package through `runpy`. A directory containing
`__main__.py` can be passed as a script. `-` and no arguments consume a
noninteractive script from stdin; this is not a REPL. Missing stdin is EOF.
The supported flags are `-u`, `-B`, `-S`, `-E`, `-s`, `-P`, `-I`, `-O`, `-OO`,
`-h`/`--help`, and `-V`/`--version`. Unsupported options fail with status 2.

The result is the shared `ProcessExitStatus { exit_code: u8 }`, with schema ID
`golem.tool.ProcessExitStatus`. Normal completion
and `sys.exit(None)` return 0. Integer `SystemExit` values are narrowed modulo
256. A string exit value prints on stderr and returns 1. Uncaught exceptions
print a traceback and return 1; `KeyboardInterrupt` returns 130. Output written
before an exception is preserved. The Bash source adapter uses this result for
`$?`, `&&`, and `||` without adding diagnostics for ordinary nonzero statuses.
Only adapter I/O failures return `PythonError`; a completed Python execution
returns a process status even when it exits with a nonzero code.

## Streaming and lifecycle

The tool declares optional stdin and required independent stdout and stderr
attachments. `sys.stdin/stdout/stderr`, their `sys.__std*__` references, and
their binary `.buffer` interfaces use those attachments. Reads wait on input;
writes use bounded chunks and wait on the host's backpressure. The tool itself
does not capture whole-invocation output. Ordinary stdout is buffered as for a
pipe; `flush=True`, explicit `flush()`, and `-u` make output available to the
host while Python is still running. Shutdown and `atexit` output are flushed
before attachment EOF.
Transport failures are tool errors, even if Python catches the corresponding
I/O exception.

External tool sessions expose live streams, including stdout before stdin EOF.
Calls from a guest use Golem's filesystem-capable attachment staging: stdin
reaches EOF before the body starts, and output becomes visible at completion.
Do not use a stdout-before-stdin handshake from a guest on this path. This
host scheduling rule is independent of Python's buffering.

Descriptor-level I/O (`os.read`/`os.write`, `open(0/1/2)`) is not redirected to
tool attachments. The stream wrappers do not advertise a usable `fileno()`.
Libraries that require terminal control or direct stdio descriptors are not
supported by this streaming contract.

Interpreter/module state ends with the call. Files written in the granted
owner filesystem persist. Golem records and reconstructs the sidecar through
the owner oplog; the tool adds no custom durable-call or snapshot layer.

## Sandboxing and limitations

The tool requires an explicit `filesystemAccess: allowed` binding. The WASM
sidecar and Golem's capability grants provide isolation, not Python filesystem
monkey patches. Granted code can access the entire owner filesystem; `cwd` and
`path-policy` middleware are not arbitrary-code sandboxes. The environment is
Golem's authorization-filtered environment; `-E`/`-I` disable Python startup
environment handling, not `os.environ` access.

The frozen standard library is embedded privately; it is not installed in the
owner filesystem. This is RustPython, not CPython. Native extension modules,
native wheels, pip installation, subprocesses, threads, sockets/SSL, SQLite and
interactive terminal sessions are not supplied. Some frozen library modules
depend on unavailable native modules. Files/scripts must be UTF-8. The tool
does not promise separate time, memory, process or file quotas; Golem owns
execution limits and interruption.

## Build and delivery

```shell
cargo make build-python-tool
cargo test --manifest-path builtin-tools/python/Cargo.toml --locked --lib
```

The manifest builds and adds metadata to `builtin-tools/python.wasm`. Generated
WASMs are ignored, never committed. Runtime provenance and license generation
are in [SOURCE.md](SOURCE.md).

The production descriptor provisions `python@0.1.0` through the same path as
the other built-ins. The artifact lock pins the immutable `python-v0.0.1`
release. The source build does not publish artifacts or bypass checksum validation.
