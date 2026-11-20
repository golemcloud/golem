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

## Prepared assets

`cargo make python-sdk-prepare` installs componentize-py from the
[golemcloud fork](https://github.com/golemcloud/componentize-py) at the
revision that the workspace pins. It then prepares the runtime and bindings for
`wit/main.wit` in `target/prepared`. Building componentize-py compiles CPython
3.14.10 for WASI and needs `WASI_SDK_PATH`.

`golem-python-assembler` links the prepared assets with the SDK and an
application and pre-initializes the result on Golem's shared Wasmtime engine.
It does not process WIT, generate bindings or compile CPython. Its tests read
`target/prepared`:

```shell
cargo make python-sdk-prepare
cargo test -p golem-python-assembler
```

## Current limits

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
