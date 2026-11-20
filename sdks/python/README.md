# Golem Python SDK

Work in progress. This directory holds the Python SDK and the inputs of its
prepared runtime:

- `wit/main.wit`: the guest world, `golem:python/agent-guest`. `wit/deps` is a
  mirror of the top-level `wit/deps` (`cargo make wit`).
- `src/golem`: the `golem` package. `golem._bootstrap` is the module that
  pre-initialization imports. It registers the component's exports, imports
  the application's entry modules and finalizes the agent registry.
  Discovery constructs no agents.

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

- Applications declare no agents yet. Discovery returns an empty list.
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
