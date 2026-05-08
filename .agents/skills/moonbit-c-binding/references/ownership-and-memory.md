# Ownership Semantics and Memory Management

Detailed reference for ownership transfer and `moonbit_incref`/`moonbit_decref`
rules. For basic `#borrow`/`#owned` usage and the external object / value-as-bytes
patterns, see SKILL.md Phase 2.

## `#owned(param)` Semantics

The `#owned` annotation explicitly declares that ownership of the parameter
transfers to C. C must call `moonbit_decref()` when done. Leaving parameters
without an ownership annotation is deprecated — new code should use
`#owned(param)` explicitly.

**Primitives** (`Int`, `UInt`, `Bool`, `Double`, `Int64`, `UInt64`, `Byte`,
`Float`): passed by value. No ownership concerns — no annotation needed.

**GC-managed objects** (`Bytes`, `String`, `FixedArray[T]`, external objects,
struct wrappers): use `#owned` to transfer ownership to C. C must call
`moonbit_decref()` when done.

**Objects allocated in C** via `moonbit_make_external_object`,
`moonbit_make_bytes`, etc.: the C code owns the newly created object. It must
either return it to MoonBit (transferring ownership) or call
`moonbit_decref()` when done. Treat these as `#owned` from the C side.

## Operation Tables

What refcount operations are required for each action on a parameter:

**`#borrow` parameters:**

| Operation | Required action |
|---|---|
| Read field/element | nothing |
| Store into data structure | `moonbit_incref` |
| Pass to MoonBit function | `moonbit_incref` |
| Pass to other C function | nothing |
| Return | `moonbit_incref` |
| End of scope | nothing |

**`#owned` parameters (default if no annotation):**

| Operation | Required action |
|---|---|
| Read field/element | nothing |
| Store into data structure | nothing (already owned) |
| Pass to MoonBit function | `moonbit_incref` |
| Pass to other C function | nothing |
| Return | nothing |
| End of scope (not returned) | `moonbit_decref` |

Practical rules for `#owned`:

- Call `moonbit_decref()` exactly once per owned parameter before the function returns.
- If storing the object longer-term, decref when the storage is torn down.
- Every early-return path must still decref all owned parameters.

Example:

```c
MOONBIT_FFI_EXPORT
int32_t
moonbit_process(void *handle, moonbit_bytes_t data) {
  size_t len = Moonbit_array_length(data);
  int32_t result = lib_process(handle, (const char *)data, len);
  moonbit_decref(handle);  // Decrement after use
  moonbit_decref(data);    // Decrement byte data
  return result;
}
```

## `Ref[T]` Output Parameters

`Ref[T]` cells let C write values back. Borrow them since C does not retain
the cell — it only writes into it.

```mbt nocheck
///|
#borrow(major, minor, patch)
extern "C" fn __llvm_get_version(
  major : Ref[UInt],
  minor : Ref[UInt],
  patch : Ref[UInt],
) = "LLVMGetVersion"

///|
pub fn llvm_get_version() -> (UInt, UInt, UInt) {
  let major = Ref::new(0U)
  let minor = Ref::new(0U)
  let patch = Ref::new(0U)
  __llvm_get_version(major, minor, patch)
  (major.val, minor.val, patch.val)
}
```

## `moonbit_incref` / `moonbit_decref` Pairing

When C holds a reference to a MoonBit object beyond a single call (e.g., storing
it in a C struct or passing it to a callback later), use `moonbit_incref` to
prevent GC collection:

```c
// Store a MoonBit object in a C struct
void store_callback(MyState *state, void *moonbit_closure) {
  moonbit_incref(moonbit_closure);  // prevent GC
  state->callback = moonbit_closure;
}

// Release when no longer needed
void clear_callback(MyState *state) {
  moonbit_decref(state->callback);  // allow GC
  state->callback = NULL;
}
```

Rules:

- Every `moonbit_incref` must have a matching `moonbit_decref`.
- Incref before storing; decref when the storage is torn down or overwritten.
- When C calls back into MoonBit, the GC may run. Ensure all C-held MoonBit
  objects are incref'd before the callback invocation.
