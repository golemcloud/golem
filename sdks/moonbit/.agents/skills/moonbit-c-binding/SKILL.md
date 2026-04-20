---
name: moonbit-c-binding
description: Guide for writing MoonBit bindings to C libraries using native FFI. Use when adding extern "c" declarations, writing C stubs with moonbit.h, configuring native-stub and link.native in moon.pkg or moon.pkg.json, choosing #borrow/#owned ownership annotations, designing callback trampolines, wrapping C pointers with external objects and finalizers, converting strings across FFI, or validating bindings with AddressSanitizer.
---

# MoonBit C Binding Guide

Step-by-step workflow for binding any C library to MoonBit using native FFI.

## When To Use

Use this skill when:

- Adding `extern "c" fn` declarations for a C library
- Writing C stub files (`moonbit.h`, `MOONBIT_FFI_EXPORT`)
- Configuring `moon.pkg` or `moon.pkg.json` for native builds (`native-stub`, `link.native`)
- Choosing `#borrow` vs ownership transfer for FFI parameters
- Wrapping C handles with external objects and finalizers
- Implementing callback trampolines (closures or `FuncRef`)
- Converting strings between MoonBit (UTF-16) and C (UTF-8)
- Running AddressSanitizer to catch memory bugs in bindings

## Plan Mode Instructions

**If you are in plan mode** and creating a plan for moonbit-c-binding work:

1. In your final plan file, include an explicit instruction at the beginning that tells the executing agent to read the moonbit-c-binding skill before starting implementation
2. The instruction should look like this:
   ```
   **Before starting implementation:** Use the Skill tool to load the moonbit-c-binding skill, which provides comprehensive guidance on FFI declarations, ownership annotations, C stubs, and AddressSanitizer validation.
   ```
3. This ensures the executing agent has access to all the critical patterns and workflows documented in this skill

## Type Mapping

Map C types to MoonBit types before writing any declarations.

| C Type | MoonBit Type | Notes |
|---|---|---|
| `int`, `int32_t` | `Int` | 32-bit signed |
| `uint32_t` | `UInt` | 32-bit unsigned |
| `int64_t` | `Int64` | 64-bit signed |
| `uint64_t` | `UInt64` | 64-bit unsigned |
| `float` | `Float` | 32-bit float |
| `double` | `Double` | 64-bit float |
| `bool` | `Bool` | Passed as `int32_t` in the C ABI (not C99 `_Bool`) |
| `uint8_t`, `char` | `Byte` | Single byte |
| `void` | `Unit` | Return type only |
| `void *` (opaque, GC-managed) | `type Handle` (opaque) | External object with finalizer |
| `void *` (opaque, C-managed) | `type Handle` with `#external` annotation | No GC tracking; C manages lifetime |
| `const uint8_t *`, `uint8_t *` | `Bytes` or `FixedArray[Byte]` | Use `#borrow` if C doesn't store it |
| `const char *` (UTF-8 string) | `Bytes` | Null-terminated by runtime; pass directly to C |
| `struct *` (small, no cleanup) | `struct Foo(Bytes)` | Value-as-Bytes pattern |
| `struct *` (needs cleanup) | `type Foo` (opaque) | External object with finalizer |
| `int` (enum/flags) | `UInt`, `Int`, or constant `enum` | `enum Foo { A = 0; B = 1; C = 10 }` maps to `int32_t` |
| callback function pointer | `FuncRef[...]` or closure | See @references/callbacks.md |
| output `int *` | `Ref[Int]` | Borrow the Ref |

## Workflow

Follow these 4 phases in order.

### Phase 1: Project Setup

Set up `moon.mod.json` and `moon.pkg` for native compilation.

**Module configuration (`moon.mod.json`):** Add `"preferred-target": "native"` so that `moon build`, `moon test`, and `moon run` default to the native backend:

```json
{
  "preferred-target": "native"
}
```

**Package configuration (`moon.pkg`):**

```moonbit
options(
  "native-stub": ["stub.c"],
  targets: {
    "ffi.mbt": ["native"]
  },
)
```

**Key fields:**

| Field | Purpose |
|---|---|
| `"native-stub"` | C source files to compile. Must be in the same directory as `moon.pkg`. |
| `targets` | Gate `.mbt` files to backends: `"ffi.mbt": ["native"]` |
| `link(native("cc-flags": ...))` | Compile flags (`-I`, `-D`). Only for system libraries. |
| `link(native("cc-link-flags": ...))` | Linker flags (`-L`, `-l`). Only for system libraries. |
| `link(native("stub-cc-flags": ...))` | Compile flags for stub files only |
| `link(native(exports: ...))` | Export MoonBit functions to C (reverse direction) |

> **Warning — `supported-targets`:** Avoid `supported-targets: ["native"]`. It prevents downstream packages from building on other targets. Use `targets` to gate individual files instead.

> **Warning — `cc`/`cc-flags` portability:** Setting `cc` disables TCC for debug builds. Setting `cc-flags` with `-I`/`-L` breaks Windows portability. Only set these for system libraries.

**Including library sources:** All files in `"native-stub"` must be in the same directory as `moon.pkg`. For inclusion strategies (flattening, header-only, system library linking), see @references/including-c-sources.md.

### Phase 2: FFI Layer

Write extern declarations and C stubs together. Keep externs private; expose safe wrappers in Phase 3. Both `extern "c"` and `extern "C"` are valid — choose one casing and be consistent (e.g., match `extern "js"` if also targeting JS).

**External object pattern** (C handle with cleanup, GC-managed):

```mbt nocheck
// ffi.mbt (gated to native in targets)

///|
type Parser  // opaque type backed by external object

///|
extern "c" fn ts_parser_new() -> Parser = "moonbit_ts_parser_new"

///|
#borrow(parser)
extern "c" fn ts_parser_language(parser : Parser) -> Language = "moonbit_ts_parser_language"
```

```c
// stub.c
#include "tree_sitter/api.h"
#include <moonbit.h>

typedef struct { TSParser *parser; } MoonBitTSParser;

static void moonbit_ts_parser_destroy(void *ptr) {
  ts_parser_delete(((MoonBitTSParser *)ptr)->parser);
  // Do NOT free ptr -- GC manages the container
}

MOONBIT_FFI_EXPORT
MoonBitTSParser *moonbit_ts_parser_new(void) {
  MoonBitTSParser *p = (MoonBitTSParser *)moonbit_make_external_object(
    moonbit_ts_parser_destroy, sizeof(TSParser *)
  );
  p->parser = ts_parser_new();
  return p;
}
```

**`#external` annotation pattern** (C pointer, C-managed lifetime):

When C fully manages the pointer's lifetime (no GC cleanup needed), annotate the type with `#external`. The pointer is passed as raw `void*` without reference counting:

```mbt nocheck
///|
#external
type RawPtr  // void*, not GC-tracked

///|
extern "c" fn raw_create() -> RawPtr = "lib_create"

///|
extern "c" fn raw_destroy(ptr : RawPtr) = "lib_destroy"
```

`#external` is an annotation (like `#borrow` and `#owned`) — it goes on its own line before the `type` declaration, not on the same line.

No C stub wrapper or `moonbit_make_external_object` is needed — the MoonBit extern calls the C function directly. Use this when the C API has explicit create/destroy functions and you want manual lifetime control.

**Ownership annotations:**

| Annotation | When to use |
|---|---|
| `#borrow(param)` | C only reads during the call, does not store a reference |
| `#owned(param)` | Ownership transfers to C; C must `moonbit_decref` when done |

Rules:

- Annotate every non-primitive parameter as `#borrow` or `#owned`.
- Primitives (`Int`, `UInt`, `Bool`, `Double`, etc.) are passed by value — no annotation needed.
- If unsure whether C stores a reference, do NOT use `#borrow`.
- Use `Ref[T]` with `#borrow` for output parameters where C writes a value back.

For detailed ownership semantics, see @references/ownership-and-memory.md.

**String conversion across FFI:**

MoonBit `Bytes` is null-terminated by the runtime, so it can be passed directly to C functions expecting `const char *`. For the reverse direction (C string to MoonBit), use `moonbit_make_bytes` + `memcpy`:

```c
// C side: return a C string as MoonBit Bytes
MOONBIT_FFI_EXPORT
moonbit_bytes_t moonbit_get_name(void *handle) {
  const char *str = lib_get_name(handle);
  int32_t len = strlen(str);
  moonbit_bytes_t bytes = moonbit_make_bytes(len, 0);
  memcpy(bytes, str, len);
  return bytes;  // if str was malloc'd, free(str) before returning
}
```

```mbt nocheck
// MoonBit side: decode UTF-8 Bytes to String
// Requires import "moonbitlang/core/encoding/utf8" in moon.pkg
///|
pub fn get_name(handle : Handle) -> String {
  @utf8.decode_lossy(get_name_ffi(handle))
}
```

**Value-as-Bytes pattern** (small struct, no cleanup):

```c
MOONBIT_FFI_EXPORT
void *moonbit_settings_new(void) {
  return moonbit_make_bytes(sizeof(settings_t), 0);
}
```

```mbt nocheck
///|
struct Settings(Bytes)  // backed by GC-managed Bytes, no finalizer
```

**`moonbit.h` core API:**

| API | Purpose |
|---|---|
| `moonbit_make_external_object(finalizer, size)` | GC-tracked object with cleanup finalizer |
| `moonbit_make_bytes(len, init)` | GC-managed byte array (MoonBit `Bytes`) |
| `moonbit_incref(ptr)` | Prevent GC collection of C-held object |
| `moonbit_decref(ptr)` | Release C's reference (pair with incref) |
| `Moonbit_array_length(arr)` | Length of GC-managed array or Bytes |
| `MOONBIT_FFI_EXPORT` | Required macro on all exported functions |

For the full API, read `$MOON_HOME/lib/moonbit.h` (default `MOON_HOME` is `~/.moon`).

### Phase 3: MoonBit API

Build safe public wrappers over the raw externs.

**Type declarations:**

```mbt nocheck
///|
type Parser          // opaque, backed by external object (has finalizer)

///|
struct Settings(Bytes)  // value type, backed by GC-managed Bytes

///|
struct Node(Bytes)      // small value struct
```

**Safe constructors and methods:**

```mbt nocheck
///|
pub fn Parser::new() -> Parser {
  ts_parser_new()
}

///|
pub fn Parser::set_language(self : Parser, language : Language) -> Bool {
  ts_parser_set_language(self, language)
}
```

**Error mapping:**

```mbt nocheck
///|
pub fn result_from_status(status : Int) -> Unit raise {
  if status < 0 {
    raise MyLibError(status)
  }
}
```

For callback patterns (FuncRef, closures, trampolines), see @references/callbacks.md.

### Phase 4: Testing

```bash
moon test --target native -v
```

Run with AddressSanitizer to catch memory bugs:

```bash
python3 scripts/run-asan.py \
  --repo-root <project-root> \
  --pkg moon.pkg \
  --pkg main/moon.pkg
```

See @references/asan-validation.md for details.

## Decision Table

| Situation | Pattern | Key Action |
|---|---|---|
| C reads pointer only during call | `#borrow(param)` | No decref in C |
| C takes ownership of pointer | `#owned(param)` | C must `moonbit_decref` |
| C handle needs cleanup on GC | External object + finalizer | `moonbit_make_external_object` |
| C pointer, C manages lifetime | `#external` annotation on `type` | No GC tracking; call C destroy explicitly |
| Small C struct, no cleanup | Value-as-Bytes | `moonbit_make_bytes` + `struct Foo(Bytes)` |
| C returns null on failure | Nullable wrapper | Check null, return `Option` or raise error |
| Callback with data parameter | FuncRef + Callback trick | See @references/callbacks.md |
| Callback without data parameter | FuncRef only | See @references/callbacks.md |
| C string (UTF-8) output | `Bytes` across FFI | `moonbit_make_bytes` + `memcpy` in C; `@utf8.decode_lossy` in MoonBit |
| Output parameter (`int *result`) | `Ref[T]` with `#borrow` | C writes into Ref, MoonBit reads `.val` |

## Common Pitfalls

1. **Using `#borrow` when C stores the pointer.** The GC may collect the object while C holds a stale reference. Only borrow for call-scoped access.

2. **Forgetting `moonbit_decref` on owned parameters.** Every non-borrowed, non-primitive parameter transfers ownership to C. Missing decrefs leak memory.

3. **Calling `free()` on external object containers.** The GC manages the container. Finalizers must only release the inner C resource.

4. **Using `moonbit_make_bytes` for structs with inner pointers.** Bytes have no finalizer, so inner heap allocations leak. Use external objects instead.

5. **Missing `moonbit_incref` before callback invocation.** When C calls back into MoonBit, the GC may run. Incref MoonBit-managed objects before the call; decref afterward.

6. **Forgetting the `MOONBIT_FFI_EXPORT` macro.** Without it, the function is invisible to the MoonBit linker.

## References

@references/ownership-and-memory.md
@references/callbacks.md
@references/including-c-sources.md
@references/asan-validation.md
