---
name: moonbit-agent-guide
description: Guide for writing, refactoring, and testing MoonBit projects. Use when working in MoonBit modules or packages, organizing MoonBit files, using moon tooling (build/check/run/test/doc/ide etc.), or following MoonBit-specific layout, documentation, and testing conventions.
---

# Agent Workflow

For fast, reliable task execution, follow this order:

1. **Clarify goal and constraints**
   - Confirm expected behavior, non-goals, and compatibility constraints (target backend, public API stability, performance limits).

2. **Locate module/package boundaries**
   - Find `moon.mod` (module root) and relevant `moon.pkg` files (package boundaries and imports).

3. **Discover APIs before coding**
   - Prefer `moon ide doc` queries to discover existing functions/types/methods before adding new code.
   - Use `moon ide outline`, `moon ide peek-def`, and `moon ide find-references` for semantic navigation.

4. **Edit minimally and package-locally**
   - Keep changes inside the correct package, use `///|` top-level delimiters, and split code into cohesive files.
   - For refactors, use `moon ide rename`; add `--loc filename:line:col` when names are ambiguous.
   - Preserve compatibility with `#alias(old_api, deprecated)` when required.

5. **Validate in a tight loop**
   - Run `moon check` after edits, adding `--warn-list +unnecessary_annotation` to enable warning 73 for redundant annotations and over-qualified constructors (`--warn-list +73` is equivalent).
   - Run targeted tests with `moon test [dirname|filename] --filter 'glob'` and use `moon test --update` for snapshot changes.

6. **Finalize before handoff**
   - Run `moon fmt`.
   - Run `moon info` to verify whether public APIs changed (`pkg.generated.mbti` diff).
   - Report changed files, validation commands, and any remaining risks.


## Fast Task Playbooks

Use the smallest playbook that matches the request.

### Bug Fix (No API Change Intended)

1. Reproduce or identify the failing behavior.
2. Locate symbols with `moon ide outline`, `moon ide peek-def`, `moon ide find-references`.
3. Implement minimal fix in the current package.
4. Validate with:
   - `moon check`
   - `moon test [dirname|filename] --filter 'glob'` (or closest targeted test scope)
   - `moon fmt`
   - `moon info` (confirm `pkg.generated.mbti` unchanged)

### Refactor (Behavior Preserving)

1. Confirm behavior/API invariants first.
2. Prefer semantic rename/navigation tools:
   - `moon ide rename`
   - `moon ide find-references`
   - `moon ide peek-def`
   - If multiple symbols share a name, use `moon ide rename <symbol> <new_name> --loc filename:line:col`.
3. Keep edits package-local and file-organization-focused.
4. Validate with:
   - `moon check`
   - `moon test [dirname|filename]`
   - `moon fmt`
   - `moon info` (API should remain unchanged unless requested)

### New Feature or Public API

1. Discover existing idioms with `moon ide doc` before introducing new names.
2. Add implementation in cohesive files with `///|` delimiters.
3. Add/extend black-box tests and docstring examples for public APIs.
4. Validate with:
   - `moon check`
   - `moon test [dirname|filename]` (use `--update` for snapshots when needed)
   - `moon fmt`
   - `moon info` (review and keep intended `pkg.generated.mbti` changes)


# MoonBit Project Layouts

MoonBit uses the `.mbt` extension for source code files and interface files with the `.mbti` extension. At
the top-level of a MoonBit project there is a `moon.mod` file specifying
the metadata of the project. The project may contain multiple packages, each
with its own `moon.pkg`. Subdirectories may also contain `moon.mod`
files indicating that a different set of dependencies can be used for that subdir.
Legacy projects may still contain `moon.mod.json`; treat it as the old module
metadata format and migrate/update guidance to `moon.mod` instead of creating
new `moon.mod.json` files.

## Example layout

```
my_module
├── moon.mod                  # Module metadata; source option can specify the source directory
├── moon.pkg             # Package metadata (each directory is a package like Golang)
├── README.mbt.md             # Markdown with tested code blocks (`test "..." { ... }`)
├── README.md -> README.mbt.md
├── cmd                       # Command line directory
│   └── main
│       ├── main.mbt
│       └── moon.pkg     # executable package with `options("is-main": true)`
├── liba/                     # Library packages
│   └── moon.pkg         # Referenced by other packages as `@username/my_module/liba`
│   └── libb/                 # Library packages
│       └── moon.pkg     # Referenced by other packages as `@username/my_module/liba/libb`
├── user_pkg.mbt              # Root packages, referenced by other packages as `@username/my_module`
├── user_pkg_wbtest.mbt       # White-box tests (only needed for testing internal private members, similar to Golang's package mypackage)
└── user_pkg_test.mbt         # Black-box tests
└── ...                       # More package files, symbols visible to current package (like Golang)
```

- **Module**: characterized by a `moon.mod` file in the project root directory.
  A MoonBit *module* is like a Go module; it is a collection of packages in subdirectories, usually corresponding to a repository or project.
  Module boundaries matter for dependency management and import paths.

- **Package**: characterized by a `moon.pkg` file in each directory.
  All subcommands of `moon` will
  still be executed in the directory of the module (where `moon.mod` is
  located), not the current package.
  A MoonBit *package* is the actual compilation unit (like a Go package).
  All source files in the same package are concatenated into one unit and
  thereby share all definitions throughout that package.
  The `name` in the `moon.mod` file combined with the relative path to
  the package source directory defines the package name, not the file name.
  Imports refer to module + package paths, NEVER to file names.

- **Files**:
  A `.mbt` file is just a chunk of source code inside a package.
  File names do NOT create modules, packages, or namespaces.
  You may freely split/merge/move declarations between files in the same package.
  Any declaration in a package can reference any other declaration in that package, regardless of file.


## Coding/layout rules you MUST follow:

1. Prefer many small, cohesive files over one large file.
   - Group related types and functions into focused files (e.g. http_client.mbt, router.mbt).
   - If a file is getting large or unfocused, create a new file and move related declarations into it.

2. You MAY freely move declarations between files inside the same package.
   - Each block is separated by `///|`. Moving a function/struct/trait between files does not change semantics, as long as its name and pub-ness stay the same. The order of each block is irrelevant too.
   - It is safe to refactor by splitting or merging files inside a package.

3. File names are purely organizational.
   - Do NOT assume file names define modules, and do NOT use file names in type paths.
   - Choose file names to describe a feature or responsibility, not to mirror type names rigidly.

4. When adding new code:
   - Prefer adding it to an existing file that matches the feature.
   - If no good file exists, create a new file under the same package with a descriptive name.
   - Avoid creating giant "impl", “misc”, or “util” files.

5. Tests:
   - Place tests in dedicated test files (e.g. `*_test.mbt`) within the appropriate package.
     For a package (besides `*_test.mbt`files), `*.mbt.md` files are also blackbox test files in addition to Markdown files.
     The code blocks (separated by triple backticks) `mbt check` are treated as test cases and serve both purposes: documentation and tests.
     You may have `README.mbt.md` files with `mbt check` code examples. You can also symlink `README.mbt.md` to `README.md`
     to make it integrate better with GitHub.
   - It is fine — and encouraged — to have multiple small test files.

6. Interface files (`pkg.generated.mbti`)
   `pkg.generated.mbti` files are compiler-generated summaries of each package's public API surface.
   They provide a formal, concise overview of all exported types, functions, and traits without implementation details.
   They are generated using `moon info` and useful for code review. When you have a commit that does not change public APIs, `pkg.generated.mbti` files will remain unchanged, so it is recommended to put `pkg.generated.mbti` in version control when you are done.
   Do not modify `pkg.generated.mbti` directly, including whitespace-only cleanup; regenerate it with `moon info` and review its diff as the public API signal.

   For IDE navigation and symbol lookup commands, see the dedicated `moon ide` section below.

# Common Pitfalls to Avoid

- **Don't use uppercase for variables/functions** - compilation error
- **Don't forget `mut` for mutable record fields** - immutable by default (note that Arrays typically do NOT need `mut` unless completely reassigning to the variable - simple push operations, for example, do not need `mut`)
- **Don't ignore error handling** - either handle errors explicitly, or declare `raise` on the caller and let checked errors propagate
- **Don't use `return` unnecessarily** - the last expression is the return value
- **Don't create methods without Type:: prefix** - methods need explicit type prefix
- **Don't forget to handle array bounds** - use `get()` for safe access
- **Don't forget @package prefix when calling functions from other packages**
- **Don't use ++ or -- (not supported)** - use `i = i + 1` or `i += 1`
- **Don't add explicit `try` for error propagation** - inside a `raise` function, call error-raising functions normally; use `catch` to handle locally and `try!` only when aborting is intended
- **Legacy syntax**: Legacy code may use `function_name!(...)` or `function_name(...)?` - these are deprecated; use normal calls for propagation.
- **Don't write an empty parameter list for `main`** - use `fn main { ... }` or `fn main raise { ... }`, not `fn main() { ... }` or `fn main() raise ... { ... }`
- **Don't write record-style enum or error constructor fields** - labeled constructor fields use `label~ : Type`, e.g. `InvalidNumber(input~ : String)`, not `InvalidNumber(input: String)`
- **Prefer range `for` loops over C-style** - `for i in 0..<(n-1) {...}` and `for j in 0..=6 {...}` are more idiomatic in MoonBit
- **Don't use `for { ... }` for infinite loops** - write `for ;; { ... }` instead
- **Don't `derive(Show)` for debugging** - derive `Debug` and use `debug_inspect()` for test/diagnostic output (`\{Repr(value)}` for interpolation of composed values). Reserve a manual `impl Show` for specialized display formats (JSON, XML, domain text)
- **Don't call `@json.inspect()`** - use the prelude `json_inspect(value, ...)` without a package prefix
- **Async** - MoonBit has no `await` keyword; do not add it. Async functions default to raising, so do not add `raise`; add `noraise` only when the async body must not raise.
  Async functions and tests are characterized by those which call other async functions.
  To identify a function or test as async, simply add the `async` prefix (e.g. `[pub] async fn ...`, `async test ...`).

# `moon` Essentials

## Essential Commands

- `moon new my_project` - Create new project
- `moon run cmd/main` - Run main package
- `moon run - < hello.mbt` - Run code from stdin (useful for quick experiments)
- `moon run -e "code snippet"` - Run code from command line argument (good for one-liners)
  Example:
  ```bash
  cat hello.mbt | moon run -
  ```
  This allows you to quickly test small snippets of MoonBit code without creating a full project.
  It can also be used with heredoc syntax for multi-line snippets:
  ```bash
  moon run - <<'EOF'
  fn main {
    println("Hello, MoonBit!")
  }
  EOF
  ```
  ```
  moon run -e 'fn main { println("Hello, MoonBit!") }'
  ```
  For multi-line `-e` snippets, especially snippets with `import { ... }`,
  pass real newlines. Do not put literal `\n` escapes inside single quotes;
  MoonBit will see backslash characters, not line breaks. Use command
  substitution with a quoted heredoc:
  ```bash
  moon run --target native -e "$(cat <<'EOF'
  import {
    "moonbitlang/x/sys"
  }
  fn main {
    println(@sys.get_cli_args().join("|"))
  }
  EOF
  )"
  ```
- `moon build` - Build project
  (`moon run` and `moon build` both support `--target`; `moon build` also supports `--diagnostic-limit <N>`)
- `moon check` - Type check without building, use it REGULARLY, it is fast
  (`moon check` also supports `--target` and `--diagnostic-limit <N>`)
- `moon info` - Type check and generate `mbti` files.
  Run it to see if any public interfaces changed.
  (`moon info` also supports `--target`.)
- `moon check --target all` - Type check for all backends
  moon check --output-json can be used with `jq` to filter the output, e.g,
  ```
  moon check --output-json 2>&1 | jq -R 'fromjson? | select(.message |
      contains("unused"))'
  ```
  or, for richer post-processing, pipe into a small MoonBit program via
  `moon run -e`. Use `--target native` (the default `wasm-gc` does not support
  `async fn main` or `@stdio.stdin`), a quoted heredoc (`<<'EOF'`) so the shell
  does not expand `$`/backticks in the source, and a de-indented closing `EOF`:
  ````
moon check --output-json 2>&1 | moon run --target native -e "$(cat <<'EOF'
import {
  "moonbitlang/async",
  "moonbitlang/async/stdio",
  "moonbitlang/core/json",
}

async fn main {
  let seen = {}
  while @stdio.stdin.read_until("\n") is Some(line) {
    try @json.parse(line.trim()) catch {
      _ => ()
    } noraise {
      {"level": "warning", "path": String(p), ..} =>
        if !seen.contains(p) {
          seen[p] = ()
          println(p)
        }
      _ => ()
    }
  }
}
EOF
)"
  ````
  Get the diagnostics with "unused" in the message, which can be used to find unused code.
- `moon explain` - Show built-in documentation for compiler diagnostics and language topics.
  - `moon explain --diagnostic` lists warning mnemonics and IDs.
  - `moon explain --diagnostic 31` explains warning 31 (`unused_optional_argument`).
  - `moon explain --diagnostic unused_optional_argument` explains the same warning by mnemonic.
  - `moon explain --attribute` lists supported attributes such as `#deprecated`, `#alias`, `#cfg`, `#coverage.skip`, and `#warnings`.
  - `moon explain --attribute deprecated` explains the `#deprecated` attribute and its supported forms.
- `moon add package` - Add dependency
- `moon remove package` - Remove dependency
- `moon fmt` - Format code - should be run periodically - note that the files may be rewritten
Note you can also use `moon -C dir check` to run commands in a specific directory.

### Profiling Hot Paths (`moon run --profile`)

`moon run --profile --target native --release cmd/<main>` runs a native release build under a sampling profiler and prints ranked **self-time** and **inclusive-time** tables plus a "runtime leaf costs attributed to MoonBit callers" section (which maps allocation, reference-counting, and string-equality costs back to *your* functions), alongside a `profile.json` and a `.trace` you can open in Instruments. On macOS it needs Xcode's `xcrun xctrace`, so install the full Xcode (not just the command-line tools) first. A single parse or compute is far too short to sample meaningfully, so point the profiled `main` at a loop that exercises the hot path a few hundred times over a representative fixture; this loop harness is throwaway and should never be committed.

Read **self-time** for *which function burns cycles* and **inclusive-time** for *which call subtree dominates*, then work a tight loop: profile, fix the top item, re-profile. Always re-baseline before trusting a delta — sampled timings drift with machine load, so build and benchmark the branch and `main` back-to-back (interleaved) rather than comparing against a number from an earlier session.

### Test Commands

- `moon test` - Run all tests
  (`moon test` also supports `--target`)
- `moon test --update` - Update snapshots
- `moon test -v` - Verbose output with test names
- `moon test [dirname|filename]` - Test specific directory or file
- `moon coverage analyze` - Analyze coverage
- `moon test [dirname|filename] --filter 'glob'` - Run tests matching filter
  ```
  moon test float/float_test.mbt --filter "Float::*"
  moon test float -F "Float::*" // shortcut syntax
  ```

## `README.mbt.md` Generation Guide

- Output `README.mbt.md` in the package directory.
  `*.mbt.md` file and docstring contents treats `mbt check` specially.
  `mbt check` block will be included directly as code and also run by `moon check` and `moon test`.  If you don't want the code snippets to be checked, explicit `mbt nocheck` is preferred.
  If you are only referencing types from the package, you should use `mbt nocheck` which will only be syntax highlighted.
  Symlink `README.mbt.md` to `README.md` to adapt to systems that expect `README.md`.

## Testing Guide

Use snapshot tests as it is easy to update when behavior changes.

- **Snapshot Tests**: write `inspect(value)` / `debug_inspect(value)` / `json_inspect(value)`, then run `moon test --update` (or `moon test -u`) to fill in `content=`.
  - Use `inspect()` for values that implement `Show` (primitives, or types with a manual `impl Show`).
  - Use `debug_inspect()` for any type that derives `Debug` — the default for your own data types.
  - Use `json_inspect()` for complex nested structures (uses the `ToJson` trait, produces more readable output).
  - It is encouraged to inspect the whole return value of a function if it is not huge; this keeps the test simple. Derive `Debug` and/or `ToJson` (or `impl Show`) on `YourType` accordingly.
- **Update workflow**: After changing code that affects output, run `moon test --update` to regenerate snapshots, then review the diffs in your test files (the `content=` parameter will be updated automatically).
- **Validation order**: Follow the canonical sequence in `Agent Workflow` and `Fast Task Playbooks`.

- Black-box by default: Call only public APIs via `@package.fn`. Use white-box tests only when private members matter.
- Grouping: Combine related checks in one `test "..." { ... }` block for speed and clarity.
- Panics: Name tests with prefix `test "panic ..." {...}`; if the call returns a value, wrap it with `ignore(...)` to silence warnings.
- Errors: For expected success, call error-raising functions directly. If a call unexpectedly raises, the test fails with the actual error. For expected failure, use `try ... catch ... noraise`, inspect the error in `catch`, and fail explicitly in `noraise`.
  Default expected-failure shape: `try f() catch { err => inspect(err) } noraise { _ => fail("expected to fail") }`.

### Docstring tests

Public APIs are encouraged to have docstring tests.

````mbt check
///|
/// Return the sum of an `Array`.
///
/// # Example
/// ```mbt check
/// test {
///   inspect(sum_array([1, 2, 3, 4, 5, 6]), content="21")
/// }
/// ```
pub fn sum_array(xs : Array[Int]) -> Int {
  xs.fold(init=0, (a, b) => a + b)
}
````

The MoonBit code in a docstring will be type checked and tested automatically
(using `moon test --update`). In docstrings, `mbt check` should only contain `test` or `async test`.

## Spec-driven Development

- The spec can be written in a readonly `spec.mbt` file (name is conventional, not mandatory) with stub code marked as declarations:

```mbt check
///|
declare pub type Yaml

///|
declare pub fn Yaml::to_string(y : Yaml) -> String raise

///|
declare pub impl Eq for Yaml

///|
declare pub fn parse_yaml(s : String) -> Yaml raise
```

- Add `spec_easy_test.mbt`, `spec_difficult_test.mbt`, etc. to test the spec functions; everything will be type-checked(`moon check`).
- The AI or users can implement the `declare` functions in different files thanks to our package organization.
- Run `moon test` to check everything is correct.

- `declare` is supported for functions, methods, and types.
- The `pub type Yaml` line is an intentionally opaque placeholder; the implementer chooses its representation.
- Note the spec file can also contain normal code, not just declarations.

## `moon ide [doc|peek-def|outline|find-references|hover|rename|analyze]` for code navigation and refactoring

For project-local symbols and navigation, use:
- `moon ide doc <query>` to discover available APIs, functions, types, and methods in MoonBit. Always prefer `moon ide doc` over other approaches when exploring what APIs are available, it is **more powerful and accurate** than `grep_search` or any regex-based searching tools.
- `moon ide outline .` to scan a package,
- `moon ide find-references <symbol>` to locate usages, and
- `moon ide peek-def` for inline definition context and to locate toplevel symbols.
- `moon ide hover sym --loc filename:line:col` to get type information at a specific location.
- `moon ide rename <symbol> <new_name> [--loc filename:line:col]` to rename a symbol project-wide. Prefer `--loc` when symbol names are ambiguous.
- `moon ide analyze [path]` to inspect public API usage of a package or module when planning safe refactors.
These tools save tokens and are more precise than grepping (`grep` displays results in both definitions and call sites including comments too).

### `moon ide doc` for API Discovery

`moon ide doc` uses a specialized query syntax designed for symbol lookup:
- **Empty query**: `moon ide doc ''`

  - In a module: shows all available packages in current module, including dependencies and moonbitlang/core
  - In a package: shows all symbols in current package
  - Outside package: shows all available packages

- **Function/value lookup**: `moon ide doc "[@pkg.]value_or_function_name"`

- **Type lookup**: `moon ide doc "[@pkg.]Type_name"` (builtin type does not need package prefix)

- **Method/field lookup**: `moon ide doc "[@pkg.]Type_name::method_or_field_name"`

- **Package exploration**: `moon ide doc "@pkg"`
  - Show package `pkg` and list all its exported symbols
  - Example: `moon ide doc "@json"` - explore entire `@json` package
  - Example: `moon ide doc "@encoding/utf8"` - explore nested package
- **Multiple queries**: `moon ide doc "query1" "query2" ...`
  - Run multiple queries in one invocation and combine results
  - Example: `moon ide doc "String" "Array" "@json"` to explore multiple types and a package at once
- **Globbing**: Use `*` wildcard for partial matches, e.g. `moon ide doc "String::*rev*"` to find all String methods with "rev" in their name

#### `moon ide doc` Examples

````bash
# search for String methods in standard library:
$ moon ide doc "String"

type String

  pub fn String::add(String, String) -> String
  # ... more methods omitted ...

$ moon ide doc "@buffer" # list all symbols in package buffer:
moonbitlang/core/buffer

fn from_array(ArrayView[Byte]) -> Buffer
# ... omitted ...

$ moon ide doc "@buffer.new" # list the specific function in a package:
package "moonbitlang/core/buffer"

pub fn new(size_hint? : Int) -> Buffer
  Creates ... omitted ...


$ moon ide doc "String::*rev*"  # globbing
package "moonbitlang/core/string"

pub fn String::rev(String) -> String
  Returns ... omitted ...
  # ... more

pub fn String::rev_find(String, StringView) -> Int?
  Returns ... omitted ...
````

**Best practice**: Treat this section as command reference; execution order is defined in `Agent Workflow`.

### `moon ide rename sym new_name [--loc filename:line:col]` example

When the user asks: "Can you rename the function `compute_sum` to `calculate_sum`?"

```
$ moon ide rename compute_sum calculate_sum --loc math_utils.mbt:2

*** Begin Patch
*** Update File: cmd/main/main.mbt
@@
 ///|
 fn main {
-  println(@math_utils.compute_sum(1, 2))
+  println(@math_utils.calculate_sum(1, 2))
 }
*** Update File: math_utils.mbt
@@
 ///|
-pub fn compute_sum(a: Int, b: Int) -> Int {
+pub fn calculate_sum(a: Int, b: Int) -> Int {
   a + b
 }
*** Update File: math_utils_test.mbt
@@
 ///|
 test {
-  inspect(@math_utils.compute_sum(1, 2))
+  inspect(@math_utils.calculate_sum(1, 2))
 }
*** End Patch
```

### `moon ide hover sym --loc filename:line:col` example

When the user asks: "What is the signature and docstring of `filter`? at line 14 of hover.mbt"

```
$ moon ide hover  filter --loc hover.mbt:14
test {
  let a: Array[Int] = [1]
  inspect(a.filter((x) => {x > 1}))
            ^^^^^^
            ```moonbit
            fn[T] Array::filter(self : Array[T], f : (T) -> Bool raise?) -> Array[T] raise?
            ```
            ---
            
             Creates a new array containing all elements from the input array that satisfy
             ... omitted ...
}
```


### `moon ide peek-def sym [--loc filename:line:col]` example

When the user asks: "Can you check if `Parser::read_u32_leb128` is implemented correctly?"
you can run `moon ide peek-def Parser::read_u32_leb128` to get the definition context
(this is better than `grep` since it searches the whole project by semantics):

``` file src/parse.mbt
L45:|///|
L46:|fn Parser::read_u32_leb128(self : Parser) -> UInt raise ParseError {
L47:|  ...
...:| }
```

Now if you want to see the definition of the `Parser` struct, you can run:

```bash
$ moon ide peek-def Parser --loc src/parse.mbt:46:4
Definition found at file src/parse.mbt
  | ///|
2 | priv struct Parser {
  |             ^^^^^^
  |   bytes : Bytes
  |   mut pos : Int
  | }
  |
```

For the `--loc` argument, the line number must be precise; the column can be approximate since
the positional argument `Parser` helps locate the position.

If the "sym" is a toplevel symbol, the location can be omitted:

````bash
$ moon ide peek-def String::rev
Found 1 symbols matching 'String::rev':

`pub fn String::rev` in package moonbitlang/core/builtin at /Users/usrname/.moon/lib/core/builtin/string_methods.mbt:1039-1044
1039 | ///|
     | /// Returns a new string with the characters in reverse order. It respects
     | /// Unicode characters and surrogate pairs but not grapheme clusters.
     | pub fn String::rev(self : String) -> String {
     |   self[:].rev()
     | }
````

### `moon ide outline [dir|file]` and `moon ide find-references <sym>` for Package Symbols

Use `moon ide outline` to scan a package or file for top-level symbols and locate usages without grepping.

- `moon ide outline dir` outlines the current package directory (per-file headers)
- `moon ide outline parser.mbt` outlines a single file
  This is useful when you need a quick inventory of a package, or to find the right file before `peek-def`.
- `moon ide find-references TranslationUnit` finds all references to a symbol in the current module

```bash
$ moon ide outline .
spec.mbt:
 L003 | pub(all) enum CStandard {
        ...
 L013 | pub(all) struct Position {
        ...
```

```bash
$ moon ide find-references TranslationUnit
```

## Package Management

### Adding Dependencies

```sh
moon add moonbitlang/x        # Add latest version
moon add moonbitlang/x@0.4.6  # Add specific version
```

### Updating Dependencies

```sh
moon update                   # Update package index
```

### Browsing Third-Party Source (`moon fetch`)

`moon fetch <author>/<module>[@<version>]` downloads a package's source into `.repos/<author>/<module>/<version>/` for offline reading (examples, internals, generated `.mbti`). It does NOT add the package to `moon.mod` — use `moon add` for that. Add `.repos/` to `.gitignore`.

```sh
moon fetch moonbitlang/async@0.18.1   # browse source/examples without taking a dependency
```

### Typical Module configurations (`moon.mod`)
```
name = "username/hello"
version = "0.1.0"
readme = "README.mbt.md"
repository = ""
license = "Apache-2.0"
keywords = []
description = "..."

import {
  "moonbitlang/x@0.4.6",
}

options(
  // source: "src", // Optional; default is "."
  "preferred-target": "native",
)
```

Use `moon add moonbitlang/x@0.4.6` and `moon remove moonbitlang/x` to manage the `import` block instead of editing dependency versions by hand.

### Typical Package configuration (`moon.pkg`)

moon.pkg for simplicity
```
import {
  "username/hello/liba",
  "moonbitlang/x/encoding" @libb,
}
import {
  "username/hello/test_helpers",
} for "test"
import {
  "username/hello/internal_test_helpers",
} for "wbtest"
options(
  "is-main": true,
)
```

Use `supported_targets = "native"` or another target-set expression at top level when the whole package only supports selected backends.

```
supported_targets = "native"
options(
  "is-main": true,
)
```

Packages are per directory and packages without a `moon.pkg` file are not recognized.

### Package Importing (used in moon.pkg)

- **Import format**: `"module_name/package_path"`
- **Usage**: `@alias.function()` to call imported functions
- **Default alias**: Last part of path (e.g., `liba` for `username/hello/liba`)
- **Package reference**: Use `@packagename` in test files to reference the
  tested package

**Package Alias Rules**:

- Import `"username/hello/liba"` → use `@liba.function()` (default alias is the last path segment)
- Import with custom alias `import { "moonbitlang/x/encoding" @enc}` → use `@enc.function()`
  (Note that this is unnecessary when the last path segment is identical to the alias name.)
- In `_test.mbt` or `_wbtest.mbt` files, the package being tested is auto-imported

Example:

```mbt nocheck
///|
/// In main.mbt after importing "username/hello/liba" in `moon.pkg`
fn main {
  println(@liba.hello()) // Calls hello() from liba package
}
```

### Using the Standard Library (moonbitlang/core)

The `moonbitlang/core` module is always available without adding it to `moon.mod` dependencies. Ordinary core packages still need explicit `moon.pkg` imports for package aliases such as `@utf8`, `@json`, or `@strconv`; add imports like `"moonbitlang/core/encoding/utf8"` when the compiler reports a missing or implicit core package.


### Creating Packages

To add a new package `fib` under `.`:

1. Create directory: `./fib/`
2. Add `./fib/moon.pkg`
3. Add `.mbt` files with your code
4. Import in dependent packages:

   ```
     import {
       "username/hello/fib",
     }
   ```

For more advanced topics like `conditional compilation`, `link configuration`, `warning control`, and `pre-build commands`, see `references/advanced-moonbit-build.md`.

## Async IO

Asynchronous programming uses compiler support plus the `moonbitlang/async` runtime. The runtime supports the native backend best, has limited JavaScript support for IO-independent APIs, and does not support WebAssembly yet. For async IO examples, prefer native. Use `moon add moonbitlang/async@<version>` and `moon ide doc "@async"` to explore the API.

User-facing subpackages include `@async` (tasks, timers, cancellation), `@async/aqueue`, `@async/fs`, `@async/stdio`, and `@async/websocket`.
Each must be imported separately in `moon.pkg`.

1. Add the dependency and pin the native target in `moon.mod`:
   ```
   import {
     "moonbitlang/async@0.18.1",
   }

   options(
     "preferred-target": "native",
   )
   ```
2. In the executable's `moon.pkg`, set `is-main`, restrict to native, and import what you need:
   ```
   import {
     "moonbitlang/async",
     "moonbitlang/async/stdio",
   }
   supported_targets = "native"
   options(
     "is-main": true,
   )
   ```
3. Define `async fn main` (not `fn main`). Spawn concurrent tasks via `with_task_group` for structured concurrency:
   ```mbt nocheck
   ///|
   async fn main {
     @async.with_task_group(group => {
       group.spawn_bg(() => {
         @async.sleep(50)
         @stdio.stdout.write("A\n")
       })
       group.spawn_bg(() => {
         @async.sleep(20)
         @stdio.stdout.write("B\n")
       })
     })
   }
   ```

- Async functions have a raising effect by default. Write `async fn main { ... }` or `async fn f(...) { ... }`, not `async fn main raise { ... }`.
- Use `async fn f(...) noraise { ... }` only when the async body must not raise. A `noraise` async function cannot call fallible APIs unless it handles their errors locally.

**Structured-concurrency contract for `with_task_group`:**

- When `with_task_group` returns, every task spawned in the group is guaranteed to have terminated — no orphan tasks, no resource leaks.
- If any spawned task fails (and was spawned without `allow_failure=true`), the whole group fails: every other task in the group is cancelled, and the error propagates out of `with_task_group`.
- Cancelled tasks are not considered failures; they raise a cancellation error but don't trigger peer cancellation.

**Closure syntax for `spawn_bg` / `spawn`:**

- ✅ `() => { ... }` — idiomatic; async-ness is inferred from context.
- ✅ `async fn() { ... }` — explicit annotation; equivalent to the arrow form.
- ⚠️ `fn() { ... }` — triggers `Warning [0027] deprecated_syntax`: "this `fn` is asynchronous but not annotated with `async`". Don't use.
- ❌ `async () => ...`, `fn() async { ... }`, `fn(args) async { ... }` — all parse errors. `async` only goes before `fn`, never before an arrow lambda or after a parameter list.

### Async tests

Use `async test` for tests that call async functions. The package containing the test must import `moonbitlang/async` for the test mode; import any async subpackages used by the test in the same `for "test"` block.

```
import {
  "moonbitlang/async",
  "moonbitlang/async/stdio",
} for "test"
```

```mbt nocheck
///|
async test "sleep completes" {
  @async.sleep(1)
  inspect("done", content="done")
}
```

- There is no `await` keyword (similar to functions that raise errors). Inside an `async test`, call async functions normally.
- `async test` also has the async raising effect by default; do not add `raise`.
- Async tests run in parallel by default. Avoid shared ports, files, environment variables, and global mutable state unless each test isolates its resources.
- Run with `moon test --target native` unless `moon.mod` sets `"preferred-target": "native"`. Use `moon test -v` when checking test names or async scheduling behavior.
- In `README.mbt.md` and docstrings, `mbt check` blocks may contain `async test` blocks; make sure the package imports `moonbitlang/async` for the relevant test mode.

# MoonBit Language Tour

## Core facts

- **Expression‑oriented**: `if`, `match`, loops return values; the last expression is the return value.
- **References by default**: Arrays/Maps/structs mutate via reference; use `Ref[T]` for primitive mutability.
- **Blocks**: Separate top‑level items with `///|`. Generate code block‑by‑block.
  If a blank line is desired within a block (enclosed by curly braces), add a comment line after the blank line (with or without comment text).
- **Visibility**: `fn` is private by default; `pub` exposes read/construct as allowed; `pub(all)` allows external construction.
- **Naming convention**: lower_snake for values/functions; UpperCamel for types/enums; enum variants start UpperCamel.
- **Packages**: No `import` in code files; call via `@alias.fn`. Configure imports in `moon.pkg`.
- **Placeholders**: `...` is a valid placeholder in MoonBit code for incomplete implementations.
- **Global values**: immutable by default and generally require type annotations.
- **Garbage collection**: MoonBit has a GC, there is no lifetime annotation, there's no ownership system.
  Unlike Rust, like F#, `let mut` is only needed when you want to reassign a variable, not for mutating fields of a struct or elements of an array/map.

## MoonBit Error Handling (Checked Errors)

MoonBit uses checked error-throwing functions, not unchecked exceptions. All errors are a subtype of `Error`, and you can declare your own error types using `suberror`.

Checked errors are tracked in function signatures, not marked at every call site. A function that may raise declares `raise` or `raise SomeError`. If the caller only wants to pass that error upward, the caller also declares a compatible `raise` and calls the raising function normally.

- Plain call inside a `raise` function: propagate automatically.
- `fn main raise { ... }` is valid for synchronous command-line probes and small examples that should propagate errors. For async entry points, use `async fn main { ... }`; async functions can raise by default.
- In `suberror` constructors, labeled payloads use `label~ : Type`; call and pattern-match them with `label=value`.
- `expr catch { ... }` or `try { ... } catch { ... }`: handle explicitly.
- `try! expr`: abort if an error is raised.

Do not add Swift-style `try` for propagation. Do not use legacy `function_name!(...)` or `function_name(...)?` syntax for new code.

```mbt check
///|
/// Declare error types with 'suberror'
suberror ValueError {
  ValueError(String)
}

///|
/// Tuple struct to hold position info
struct Position(Int, Int) derive(ToJson, Debug, Eq)

///|
/// ParseError is subtype of Error
pub(all) suberror ParseError {
  InvalidChar(pos~ : Position, Char) // pos is labeled
  InvalidEof(pos~ : Position)
  InvalidNumber(pos~ : Position, String)
  InvalidIdentEscape(pos~ : Position)
} derive(Eq, ToJson, Debug)

///|
/// Functions declare what they can throw
fn parse_int(s : String, position~ : Position) -> Int raise ParseError {
  // 'raise' throws an error
  if s is "" {
    raise ParseError::InvalidEof(pos=position)
  }
  ... // parsing logic
}

///|
/// Declare a specific error type when callers should handle it precisely
fn div(x : Int, y : Int) -> Int raise ValueError {
  if y is 0 {
    raise ValueError::ValueError("Division by zero")
  }
  x / y
}

///|
test "expected success calls directly" {
  inspect(div(6, 3), content="2")
}

///|
test "expected failure handles the raised error" {
  try div(1, 0) catch {
    ValueError::ValueError(message) => inspect(message, content="Division by zero")
  } noraise {
    _ => fail("expected to fail")
  }
}

// Three ways to handle errors:

///|
/// Propagate automatically
fn use_parse(s : String, position~ : Position) -> Int raise ParseError {
  // This plain call is the correct propagation syntax.
  // `try! parse_int(...)` would abort instead of propagating.
  let x = parse_int(s, position~) // label punning, equivalent to position=position
  // Error auto-propagates by default.
  // Unlike Swift, you do not need to mark `try` for functions that can raise
  // errors; the compiler infers it automatically. This keeps error handling
  // explicit but concise.
  x * 2
}

///|
/// Use try! to abort if it raises, no raise in the signature 
fn use_parse2(position~ : Position) -> Int {
  let x = try! parse_int("123", position~) // label punning
  x * 2
}

///|
/// Handle with try-catch
fn handle_parse(s : String, position~ : Position) -> Int {
  parse_int(s, position~) catch {
    ParseError::InvalidEof(pos=_) => {
      println("Parse failed: InvalidEof")
      -1 // Default value
    }
    _ => 2
  }
}
```

Important: When calling a function that can raise errors, if you only want to
propagate the error, you do not need any marker; the compiler infers it.
Async functions automatically can raise errors without explicitly stating this. Do not add `raise` to async functions for propagation; add `noraise` only when the async function must reject unhandled errors.

## Integer, Char and overloaded literals

MoonBit supports `Byte`, `Int16`, `Int`, `UInt16`, `UInt`, `Int64`, `UInt64`, etc.
When the type is known, the literal can be overloaded:

```mbt check
///|
test "integer and char literal overloading disambiguation via type in the current context" {
  let (int, uint, uint16, int64, byte) : (Int, UInt, UInt16, Int64, Byte) = (
    1, 1, 1, 1, 1,
  )
  // The literal `1` is overloaded based on the expected type in the current context.
  // compile time error if the literal cannot be represented in the target type, 
  // e.g. let a7 : Byte = 256 // ❌ won't compile, 256 exceeds Byte max value 255
  assert_eq(int, uint16.to_int())
  let (a1, a2, a3) : (Int, Char, UInt16) = ('b', 'b', 'b')
  // char literal overloading, `a1` will be the unicode value of 'b', 
  // compile time error when the literal cannot be represented in the target type 
  // e.g, let a6 : UInt16 = '𐍈' // ❌ won't compile, '𐍈' is U+10348, which exceeds UInt16 max value 0xffff  
  let a4 : Byte = b'b' // Byte literal
}
```
## Bytes (Immutable)

```mbt check
///|
test "bytes literals" {
  let b0 : Bytes = b"abcd"
  let b1 : Bytes = [0xff, 0x00, 0x01] // Array literal overloading
  guard b0 is [b'a', ..] && b0[1] is b'b' else {
    // Bytes can be pattern matched as BytesView and indexed
    fail("unexpected bytes content")
  }
}
```
## Array (Resizable)

```mbt check
///|
test "array literals overloading: disambiguation via type in the current context" {
  let (a0, a1, a2, a3) : (
    Array[Int],
    FixedArray[Int],
    ReadOnlyArray[Int],
    ArrayView[Int],
  ) = ([1, 2, 3], [1, 2, 3], [1, 2, 3], [1, 2, 3])
  // The literal `[1, 2, 3]` is overloaded based on the expected type in the current context.
  // Defaults to Array[_]
}
```
## String (Immutable UTF-16)
`s[i]` returns a code unit (UInt16), `s.get_char(i)` returns `Char?`.
Since MoonBit supports char literal overloading, you can write code snippets like this:

```mbt check
///|
test "string indexing and utf8 encode/decode" {
  let s = "hello world"
  let b0 : UInt16 = s[0]
  guard b0 is ('\n' | 'h' | 'b' | 'a'..='z') && s is [.. "hello", .. rest] else {
    fail("unexpected string content")
  }
  guard rest is " world" else {
    fail("unexpected string suffix")
  }

  // In check mode (expression with explicit type), ('\n' : UInt16) is valid.

  // Using get_char for Option handling
  let b1 : Char? = s.get_char(0)
  assert_true(b1 is Some('a'..='z'))

  // ⚠️ Important: Variables won't work with direct indexing
  let eq_char : Char = '='
  // s[0] == eq_char // ❌ Won't compile - eq_char is not a literal, lhs is UInt while rhs is Char
  // Use: s[0] == '=' or s.get_char(0) == Some(eq_char)
  // Requires `"moonbitlang/core/encoding/utf8"` in `moon.pkg`.
  let bytes = @utf8.encode("中文")
  assert_true(bytes is [0xe4, 0xb8, 0xad, 0xe6, 0x96, 0x87])
  let s2 : String = @utf8.decode(bytes) // decode utf8 bytes back to String
  assert_true(s2 is "中文")
  for c in "中文" {
    let _ : Char = c // unicode safe iteration
    println("char: \{c}") // iterate over chars
  }
}
```

### String Interpolation && StringBuilder

MoonBit uses `\{}` for string interpolation, for custom types, they need to implement trait `Show`.

```mbt check
///|
test "string interpolation basics" {
  let name : String = "Moon"
  let config = { "cache": 123 }
  let version = 1.0
  println("Hello \{name} v\{version}") // "Hello Moon v1"
  // ✅ Quoted map keys are allowed inside interpolation expressions.
  println("  - Checking if 'cache' section exists: \{config["cache"]}")
  let sb = StringBuilder()
  sb <+ "[\{[ for x in [1, 2, 3] => "\{x}" ].join(",")}]"
  inspect(sb, content="[1,2,3]")
  let x = 42
  let streamed = StringBuilder()
  streamed <+ "hello \{x}"
  inspect(streamed, content="hello 42")
}
```

Expressions inside `\{}` must be single-line expressions.
Nested interpolations and string literals are supported, but line breaks inside `\{}` are not.
#### `<+` and `<?` macros for streaming interpolation
String interpolation can be streamed directly into a `Logger`/`StringBuilder`-style writer with `<+`, or conditionally through an optional writer with `<?`:

```mbt nocheck
writer <+ "hello \{x}"
writer <+ {"key1": value, "key2": value2}
lhs <? "hello \{x}"
lhs <? {"key1": value, "key2": value2}
```

This expands to calls on the writer:

```mbt nocheck
writer.write_string("hello ")
writer.write(x)
writer.write_object_begin()
writer.write_object_field("key1", value)
writer.write_object_field("key2", value2)
writer.write_object_end()
if lhs is Some(l) { l <+ "hello \{x}" }
```

Literal string segments use `write_string`; interpolated expressions use `write`.
For `<?`, `None` performs no write; `Some(writer)` applies the same `<+` expansion to the wrapped writer.
The right-hand side of `<+` and `<?` must be a template string/multiline template string or a map object literal, not an arbitrary expression.
The expansion is macro-style: it depends on how the `writer` type implements `write_string` and `write` for template strings, plus `write_object_begin`, `write_object_field`, and `write_object_end` for map object literals. Types such as HTMLBuilder or JSONBuilder can support interpolation and streaming with the same syntax but different semantics.
Because MoonBit allows local methods on foreign types, a package can adapt an existing writer type to this syntax by adding those local writer methods.

### Multiple line strings

```mbt check
///|
test "multi-line string literals" {
  let multi_line_string : String =
    #|Hello "world"
    #|World
    #|
  let multi_line_string_with_interp : String =
    $|Line 1 ""
    $|Line 2 \{1+2}
    $|
  // no escape in `#|`,
  // only escape '\{..}` in `$|`
  assert_eq(multi_line_string, "Hello \"world\"\nWorld\n")
  assert_eq(multi_line_string_with_interp, "Line 1 \"\"\nLine 2 3\n")
}
```

## Map (Mutable, Insertion-Order Preserving)

```mbt check
///|
test "map literals and common operations" {
  // Map literal syntax
  let map : Map[String, Int] = { "a": 1, "b": 2, "c": 3 }
  let empty : Map[String, Int] = Map([]) // Empty map
  // From array of pairs
  let from_pairs : Map[String, Int] = Map::from_array([("x", 1), ("y", 2)])

  // Set/update value
  map["new-key"] = 3
  map["a"] = 10 // Updates existing key

  // Get value - returns Option[T]
  guard map is { "new-key": 3, "missing"? : None, .. } else {
    fail("unexpected map contents")
  }

  // Direct access (panics if key missing)
  let value : Int = map["a"] // value = 10

  // Iteration preserves insertion order
  for k, v in map {
    println("\{k}: \{v}") // Prints: a: 10, b: 2, c: 3, new-key: 3
  }

  // Other common operations
  map.remove("b")
  guard map is { "a": 10, "c": 3, "new-key": 3, .. } && map.length() == 3 else {
    // "b" is gone, only 3 elements left
    fail("unexpected map contents after removal")
  }
}
```

## View Types

**Key Concept**: View types (`StringView`, `BytesView`, `ArrayView[T]`) are zero-copy, non-owning read-only slices created with the `[:]` syntax. They don't allocate memory and are ideal for passing sub-sequences without copying data, for functions which take `String`, `Bytes`, `Array`, they also take `*View` (implicit conversion).

- `String` → `StringView` via `s[:]` or `s[start:end]` or `s[start:]` or `s[:end]`
- `Bytes` → `BytesView` via `b[:]` or `b[start:end]`, etc.
- `Array[T]`, `FixedArray[T]`, `ReadOnlyArray[T] → `ArrayView[T]` via `a[:]` or `a[start:end]`, etc.

**Important**: StringView slice is slightly different due to unicode safety:
`s[a:b]` may raise an error at surrogate boundaries (UTF-16 encoding edge case). You have two options:

- Use `try! s[a:b]` if you're certain the boundaries are valid (crashes on invalid boundaries)
- Let the error propagate to the caller for proper handling

**When to use views**:

- Pattern matching with rest patterns (`[first, .. rest]`)
- Passing slices to functions without allocation overhead
- Avoiding unnecessary copies of large sequences

Convert back with `.to_string()`, `.to_bytes()`, or `.to_array()` when you need ownership. (`moon ide doc StringView`)

## User defined types(`enum`, `struct`)

```mbt check
///|
enum Tree[T] {
  Leaf(T) // Unlike Rust, no comma here
  Node(left~ : Tree[T], T, right~ : Tree[T]) // enum can use labels
} derive(Debug, ToJson) // derive traits for Tree

///|
pub fn Tree::sum(tree : Tree[Int]) -> Int {
  match tree {
    Leaf(x) => x
    // we don't need to write Tree::Leaf, when `tree` has a known type
    Node(left~, x, right~) => left.sum() + x + right.sum() // method invoked in dot notation
  }
}

///|
struct Point {
  x : Int
  y : Int
} derive(Debug, ToJson) // derive traits for Point

///|
pub fn Point::Point(x~ : Int, y~ : Int) -> Point {
  { x, y }
}

///|
test "user defined types: enum and struct" {
  json_inspect(Point(x=10, y=20), content={ "x": 10, "y": 20 })
  debug_inspect(
    Point(x=10, y=20),
    content=(
      #|{ x: 10, y: 20 }
    ),
  )
}
```

## Functional `for` loop


```mbt check
///|
pub(all) enum SearchIndex {
  Found(Int)
  InsertionPoint(Int)
} derive(Debug, Eq)

///|
pub fn binary_search(arr : ArrayView[Int], value : Int) -> SearchIndex {
  let len = arr.length()
  // functional for loop:
  // initial state ; [predicate] ; [post-update] {
  // loop body with `continue` to update state
  //} nobreak { // exit block
  // }
  // predicate and post-update are optional
  for i = 0, j = len; i < j; {
    // post-update is omitted, we use `continue` to update state
    let h = i + (j - i) / 2
    if arr[h] < value {
      continue h + 1, j // functional update of loop state
    } else {
      continue i, h // functional update of loop state
    }
  } nobreak { // exit of for loop
    if i < len && arr[i] == value {
      Found(i)
    } else {
      InsertionPoint(i)
    }
  } where {
    proof_invariant: 0 <= i && i <= j && j <= len,
    proof_invariant: i == 0 || arr[i - 1] < value,
    proof_invariant: j == len || arr[j] >= value,
    proof_reasoning: (
      #|For a sorted array, the boundary invariants are witnesses:
      #|  - `arr[i-1] < value` implies all arr[0..i) < value (by sortedness)
      #|  - `arr[j] >= value` implies all arr[j..len) >= value (by sortedness)
      #|
      #|Preservation proof:
      #|  - When arr[h] < value: new_i = h+1, and arr[new_i - 1] = arr[h] < value ✓
      #|  - When arr[h] >= value: new_j = h, and arr[new_j] = arr[h] >= value ✓
      #|
      #|Termination: j - i decreases each iteration (h is strictly between i and j)
      #|
      #|Correctness at exit (i == j):
      #|  - By invariants: arr[0..i) < value and arr[i..len) >= value
      #|  - So if value exists, it can only be at index i
      #|  - If arr[i] != value, then value is absent and i is the insertion point
      #|
    ),
  }
}

///|
test "functional for loop control flow" {
  let arr : Array[Int] = [1, 3, 5, 7, 9]
  debug_inspect(binary_search(arr, 5), content="Found(2)") // Array to ArrayView implicit conversion when passing as arguments
  debug_inspect(binary_search(arr, 6), content="InsertionPoint(3)")
  // for iteration is supported too
  for i, v in arr {
    println("\{i}: \{v}") // `i` is index, `v` is value
  }
}
```

You are *STRONGLY ENCOURAGED* to use functional `for` loops instead of imperative loops
*WHENEVER POSSIBLE*, as they are easier to read and reason about.

### Loop Invariants with `where` Clause

The `where` clause attaches **machine-checkable invariants** and **human-readable reasoning** to functional `for` loops. This enables formal verification thinking while keeping the code executable. Note for trivial loops, you are encouraged to convert it into `for .. in` so no reasoning is needed.

**Syntax:**
```mbt nocheck
for ... {
  ...
} where {
  invariant : <boolean_expr>,   // checked at runtime in debug builds
  invariant : <boolean_expr>,   // multiple invariants allowed
  reasoning : <string>         // documentation for proof sketch
}
```

**Writing Good Invariants:**

1. **Make invariants checkable**: Invariants must be valid MoonBit boolean expressions using loop variables and captured values.

2. **Use boundary witnesses**: For properties over ranges (e.g., "all elements in arr[0..i) satisfy P"), check only boundary elements. For sorted arrays, `arr[i-1] < value` implies all `arr[0..i) < value`.

3. **Handle edge cases with `||`**: Use patterns like `i == 0 || arr[i-1] < value` to handle boundary conditions where the check would be out of bounds.

4. **Cover three aspects in reasoning**:
   - **Preservation**: Why each `continue` maintains the invariants
   - **Termination**: Why the loop eventually exits (e.g., a decreasing measure)
   - **Correctness**: Why the invariants at exit imply the desired postcondition

## Label and Optional Parameters

Good example: use labeled and optional parameters

```mbt check
///|
fn g(
  positional : Int,
  required~ : Int,
  optional? : Int, // no default => Option
  optional_with_default? : Int = 42, // default => plain Int
) -> String {
  // These are the inferred types inside the function body.
  let _ : Int = positional
  let _ : Int = required
  let _ : Int? = optional
  let _ : Int = optional_with_default
  // `Repr` renders Option without relying on its deprecated `Show` implementation.
  "\{positional},\{required},\{Repr(optional)},\{optional_with_default}"
}

///|
test {
  inspect(g(1, required=2), content="1,2,None,42")
  inspect(g(1, required=2, optional=3), content="1,2,Some(3),42")
  inspect(g(1, required=4, optional_with_default=100), content="1,4,None,100")
}
```

Misuse: `arg : Type?` is not an optional parameter.
Callers still must pass it (as `None`/`Some(...)`).

```mbt check
///|
fn with_config(a : Int?, b : Int?, c : Int) -> String {
  "\{Repr(a)},\{Repr(b)},\{c}"
}

///|
test {
  inspect(with_config(None, None, 1), content="None,None,1")
  inspect(with_config(Some(5), Some(5), 1), content="Some(5),Some(5),1")
}
```

Anti-pattern: `arg? : Type?` (no default => double Option).
If you want a defaulted optional parameter, write `b? : Int = 1`, not `b? : Int? = Some(1)`.

```mbt check
///|
fn f_misuse(a? : Int?, b? : Int = 1) -> Unit {
  let _ : Int?? = a // rarely intended
  let _ : Int = b
}
// How to fix: declare `(a? : Int, b? : Int = 1)` directly.

///|
fn f_correct(a? : Int, b? : Int = 1) -> Unit {
  let _ : Int? = a
  let _ : Int = b
}

///|
test {
  f_misuse(b=3)
  f_misuse(a=Some(5), b=2) // works but confusing
  f_correct(b=2)
  f_correct(a=5)
}
```

Bad example: `arg : APIOptions` (use labeled optional parameters instead)

```mbt check
///|
/// Do not use struct to group options.
struct APIOptions {
  width : Int?
  height : Int?
}

///|
fn not_idiomatic(opts : APIOptions, arg : Int) -> Unit {

}

///|
test {
  // Hard to use in call site
  not_idiomatic({ width: Some(5), height: None }, 10)
  not_idiomatic({ width: None, height: None }, 10)
}
```

# MoonBit Package Organization Guideline

A package should own the public concrete types whose constructors, fields, pattern matching, and methods users are expected to
use. The owner can be the facade package itself, or a non-internal public package that the facade intentionally re-exports.

Public type ownership is more important than implementation locality. If users think of a type as `@foo.X`, then `X` should be
defined in package `foo` or in a public package re-exported by `foo`, especially if users call `X::method`, construct records,
match enum constructors, or rely on generated `.mbti` docs.

MoonBit can implicitly load the owning package for method lookup when a type is re-exported from a non-internal public package.
For example, if `@foo` re-exports `type X` from public package `@bar`, external users can write a value as `@foo.X` and still
call methods owned by `@bar.X`.

Use `internal/*` packages for implementation support:

- scanners
- parsers for sub-syntax
- escaping/encoding helpers
- validation helpers
- low-level algorithms
- private helper result types

Do not put public concrete API types in `internal/*` and expect a facade to recover the full API with re-exporting. External
users do not get the same implicit method-owner loading for internal packages, so `x.method()` can fail even when `x` is typed
as the facade's re-exported type. It also makes constructors, generated interfaces, and privacy boundaries harder to reason
about.

## Using `using` Correctly

Use `pub using` for facade ergonomics, not for type ownership.

Good use:

```mbt
// root package
pub using @parser { parse, parse_fragment }
pub using @dom { type Node, type NodeKind, to_markdown }
pub using @serializer { type HtmlContext }
```

This is good when `@parser`, `@dom`, and `@serializer` are public packages that already own those APIs.

Good value re-export from an internal package:

```mbt
pub using @impl { decode_entities }
```

This is acceptable if the exported function signature does not expose internal types and you intentionally want that value as
public API.

Risky use:

```mbt
pub using @internal_impl { type X }
```

Avoid this for public concrete types. If `X` is public, define it in the facade package or a non-internal public package. If
`X` is truly internal, do not expose it as a public concrete type.

Use an explicit wrapper instead of `pub using` when you need to:

- translate internal helper results into public types
- enforce public defaults
- hide internal helper types
- keep public API ownership clear
- make the `.mbti` easier to review

## Practical Rule

If a public function returns `X`, and users should inspect, construct, pattern match, or call methods on `X`, then `X` belongs in
the facade package that users name or a non-internal public package that the facade re-exports.

If a helper package only computes data for another package, it may live under `internal/*`, but its types should either stay
internal or be simple helper result types not exposed through the public facade.

A good package boundary looks like:

```text
foo/
  types.mbt        // public Foo, FooMode, FooResult
  api.mbt          // public functions and Foo::methods
  private_impl.mbt // private implementation files in same package

internal/foo/
  scanner.mbt
  escaping.mbt
  validation.mbt
```
