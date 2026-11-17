# Golem Python SDK

Work in progress. This directory holds the Python SDK and the inputs of its
prepared runtime:

- `wit/main.wit`: the guest world, `golem:python/agent-guest`. `wit/deps` is a
  mirror of the top-level `wit/deps` (`cargo make wit`).
- `src/golem`: the `golem` package. `golem._bootstrap` is the module that
  pre-initialization imports. It registers the component's exports, imports
  the application's entry modules and finalizes the agent registry.
  Discovery constructs no agents.
- `tests`: native tests of agent declarations and dispatch.

## Getting started

The Golem CLI builds Python components from the `golem-py-sdk` wheel. The
wheel is not published yet, so build it in a Golem checkout first and point
the CLI at it:

```shell
cargo make python-sdk-wheel
export GOLEM_PYTHON_SDK_WHEEL="$PWD/sdks/python/target/wheel/golem_py_sdk-0.0.0-py3-none-any.whl"
```

Then create, deploy and call an application with a local server:

```shell
golem new counter --template python
cd counter
golem deploy --yes
golem agent invoke --yes 'CounterAgent("c")' add 7
golem agent invoke --yes 'CounterAgent("c")' value
```

The `python` template has one component. Its `src/counter_agent.py` declares
`CounterAgent`, and `src/main.py`, the entry module, imports it. The
component's `assemblePython` build step names the source directory, the
entry modules and the `pyproject.toml`:

```yaml
build:
- assemblePython: "{{ componentDir }}/src"
  modules:
  - main
  pyproject: "{{ componentDir }}/pyproject.toml"
  into: "{{ golemTempDir }}/agents/{{ component_name | to_snake_case }}.wasm"
```

The step runs no Python, uv or C compiler on the host. It skips the assembly
when the sources, the `pyproject.toml`, the wheel and the step itself did not
change; adding, editing, removing or renaming a source file, or changing the
entry modules, assembles again. A failed
build keeps the last component that it assembled. The build fails, with the
cause, when:

- `GOLEM_PYTHON_SDK_WHEEL` is not set or names a wheel that does not match
  the CLI (another version, another prepared runtime or a corrupt file);
- `project.dependencies` lists anything other than the exact
  `golem-py-sdk==<version>` that the CLI builds with;
- `pyproject.toml` declares a package source, such as a `[tool.uv.sources]`
  path or Git entry or a uv index. The SDK always comes from the wheel that
  `GOLEM_PYTHON_SDK_WHEEL` names;
- an entry module or a module that it imports raises, with the original
  traceback;
- a declaration is invalid, with the file and line of each one;
- the component declares agent or tool `dependencies`. Python guest bridges
  are not generated yet.

`cargo make cli-integration-tests-python` builds the wheel and runs the CLI
integration tests of the template. No CI job runs them yet.

## Agents

```python
from golem import agent, method


@agent
class Counter:
    def __init__(self, name: str) -> None:
        self.name = name
        self.value = 0

    @method
    def increment(self, amount: int) -> int:
        self.value += amount
        return self.value
```

`@agent` declares an agent type with the class name. The ordinary `__init__`
is the constructor, and its parameters are the constructor parameters.
`@method` declares a method with the function name. Other methods are not
exported. An instance hosts one agent, which keeps its object state between
calls.

The decorators only record the declarations. After pre-initialization imports
every entry module, the SDK resolves the annotations and checks the
declarations. One invalid declaration or more fails the build, and the error
names each one with its file and line.

Parameters and results have these types:

| Python  | Schema type                                   |
|---------|-----------------------------------------------|
| `bool`  | `bool`                                        |
| `int`   | `s64`; a value outside its range is an error  |
| `float` | `f64`                                         |
| `str`   | `string`                                      |
| `bytes` | `binary`                                      |
| `None`  | no result (methods only)                      |

`bool` is not an `int`, and `int` is not a `float`, in either direction.

The SDK checks the arguments of a call against the declaration before the
constructor or the method runs. Invalid arguments fail with `invalid-input`
and change nothing. An exception from user code fails the call with a
`custom-error` that names the exception. A result of the wrong type also fails
the call with a `custom-error`, but only after the method ran: its effects
stay. A second `initialize` fails and keeps the agent.

Run the native tests with CPython 3.14:

```shell
cd sdks/python
python3.14 -B -m unittest discover -s tests
```

## Prepared assets and the wheel

`cargo make python-sdk-prepare` installs componentize-py from the
[golemcloud fork](https://github.com/golemcloud/componentize-py) at the
revision that the workspace pins. It then prepares the runtime and bindings for
`wit/main.wit` in `target/prepared`, and writes `target/compatibility.json`,
which names that revision. Building componentize-py compiles CPython 3.14.10
for WASI and needs `WASI_SDK_PATH`.

`cargo make python-sdk-wheel` builds
`target/wheel/golem_py_sdk-<version>-py3-none-any.whl` with `uv`. The wheel,
`golem-py-sdk`, contains:

- the `golem` package with `py.typed`, which imports on CPython 3.14 or later
  without the guest runtime;
- `golem/_prepared`, the prepared runtime and bindings;
- `golem/_compatibility.json`, the componentize-py revision that prepared them.

The build fixes `SOURCE_DATE_EPOCH`, so the same inputs give the same wheel
bytes in any directory and at any time.

`golem-python-assembler` reads the wheel without running Python
(`read_sdk_wheel`). It fails before any application code runs, and names the
expected and the found values, when:

- a file does not match the wheel's `RECORD`, `RECORD` lists a missing file or
  misses a file, or a path leaves the wheel;
- the wheel's version is not the assembler's version;
- the componentize-py revision is not the one that the assembler links;
- the prepared assets fail their own checks;
- the identity of the prepared assets, the SHA-256 of their manifest, is not
  the assembler's `PREPARED_IDENTITY`. The manifest names the digests of the
  world bindings, the standard library, the adapter and every runtime library,
  so assets for another world or runtime fail this check even when they come
  from the same componentize-py revision.

`read_prepared` applies the same identity check to a `target/prepared`
directory. When the WIT world, the runtime or the componentize-py revision
changes, run `cargo make python-sdk-prepare` again and set
`PREPARED_IDENTITY` to the new identity.

It never regenerates assets or compiles CPython. The assembler then links the
prepared assets with the SDK and an application and pre-initializes the
result on Golem's shared Wasmtime engine. It does not process WIT or generate
bindings.

The assembler's tests read `target/prepared` and the wheel:

```shell
cargo make python-sdk-wheel
cargo test -p golem-python-assembler
```

## Current limits

- The wheel does not yet include the license notices of CPython and the other
  parts of the prepared runtime, so it is not published.
- Methods are synchronous. Async methods, compound types, default, variadic
  and keyword-only parameters and an agent `config` fail the build with a
  named error.
- `@agent` and `@method` take no options yet. Docstrings are not exported as
  descriptions, and agents have no HTTP mounts or declared errors.
- Third-party packages and native extensions in applications are not
  supported.
- Pre-initialization output is limited to 64 KiB each for stdout and stderr.
  More output fails the build.
- The C stack is 96 KiB. Deep recursion in `repr`, `json`, `pickle` and
  `__repr__` raises `RecursionError` at a lower depth than in native CPython.
- During pre-initialization, file timestamps read as the Unix epoch, and
  inode numbers follow the order in which files are first seen, so that the
  saved component does not depend on the host. Two builds of the same input in
  different directories give the same bytes on the same host. Builds on
  different file systems or hosts are not yet checked: directory listing order
  and link counts still come from the host.
- Because directory timestamps do not change during pre-initialization, call
  `importlib.invalidate_caches()` before importing a module that a module
  wrote during pre-initialization.
