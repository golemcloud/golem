# Including C Library Sources

How to include C library sources in MoonBit native builds.

## Constraint

The `moon` toolchain compiles `native-stub` files from the package directory
only — it does not recurse into subdirectories. All C source files must be in
the same directory as `moon.pkg`.

## Strategies

Choose the strategy that matches your library, listed by increasing complexity:

### Flat source directory

Libraries with a flat source layout (e.g., Lua) need no special handling — copy
`.c` and `.h` files directly into the package directory:

```python
for file in lua_src_dir.iterdir():
    if file.suffix in (".c", ".h"):
        shutil.copy2(file, package_dir / file.name)
```

List them in `native-stub` as-is.

### Header-only

`#define` the implementation macro and `#include` the header in your stub file.
No copying needed.

```c
#define STB_IMAGE_IMPLEMENTATION
#include "stb_image.h"
#include <moonbit.h>
```

### System library linking

Include only headers in the stub and supply linker flags in `moon.pkg`:

```moonbit
link(
  native(
    "cc-flags": "-I/path/to/include",
    "cc-link-flags": "-L/path/to/lib -lmylib",
  )
)
```

> **Portability warning:** `-I`/`-L`/`-l` flags are GCC/Clang conventions.
> MSVC's `cl.exe` does not accept them.

### Nested source tree (flattening)

Libraries with nested directories (e.g., `src/unix/async.c`, `lib/src/parser.c`)
must be **flattened** into the package directory. See below.

## Flattening Nested Source Trees

A flattening script (typically a shell or Python script) should handle three
concerns:

### 1. Filename mangling

Copy each source file into the package directory, encoding the original path
into a flat filename. A common convention is replacing `/` with `#`:

| Original path | Flattened filename |
|---|---|
| `lib/src/lib.c` | `tree-sitter#lib#src#lib.c` |
| `src/unix/async.c` | `uv#src#unix#async.c` |

The `#` has no special meaning to `moon` — it simply makes it easy to trace a
file back to its original location. Use a library-name prefix (e.g.,
`tree-sitter`, `uv`) to avoid collisions.

### 2. Include rewriting

After flattening, `#include "relative/path.h"` directives become invalid because
the directory structure is gone. The script should rewrite quoted includes to
reference the mangled filenames:

```c
// Before (original source)
#include "unix/internal.h"

// After (flattened)
#include "uv#src#unix#internal.h"
```

Only rewrite quoted includes (`#include "..."`), not system includes (`#include
<...>`). The script should also recursively copy and flatten any discovered
headers.

### 3. Platform-conditional guards

For cross-platform libraries, different source files are needed per OS. Wrap
platform-specific file contents in preprocessor guards so all variants can
coexist in a single `native-stub` list:

```c
// uv#src#win#async.c — Windows-only source
#if defined(_WIN32)
/* ... original source with rewritten includes ... */
#endif
```

`moon` compiles all `.c` files on every platform, but the guards ensure only
relevant code is included.

### Updating `moon.pkg`

When a library has many source files, the script should also update the
`native-stub` array in `moon.pkg.json` programmatically to stay in sync with the
files on disk.
