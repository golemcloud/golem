# Callbacks, FuncRef

Callbacks in C usually have the following two forms:

- Takes both a function pointer and callback data;
- Or takes an function pointer only.

## Function pointer with callback data.

In this case, you should use the "FuncRef + Callback" trick. By doing the
trampoline on the MoonBit side, you are free from managing the lifetime of the
closure yourself.

**C library signature:**

```c
void register_callback(void (*callback)(void*), void *data);
```

**MoonBit wrapper:**

```moonbit
extern "c" fn register_callback_ffi(
  call_closure : FuncRef[(() -> Unit) -> Unit],
  closure : () -> Unit
) = "register_callback"

pub fn register_callback(callback : () -> Unit) -> Unit {
  register_callback_ffi(fn(f) { f() }, callback)
}
```

**How it works:**

- The first parameter is a closed function (no captured variables) that takes a
  closure and invokes it
- The second parameter is the actual closure you want to pass
- The C function calls your closed function with the data parameter, which
  effectively performs partial application
- This works with any C API that supports callback data (e.g., `void
  *user_data`, `void *ctx`, `void *payload`)

**Example with parameters:**

```moonbit
// C signature: void process_items(int (*callback)(void*, int), void *data);
extern "c" fn process_items_ffi(
  call_closure : FuncRef[(Int -> Int, Int) -> Int],
  closure : Int -> Int
) = "process_items"

pub fn process_items(callback : Int -> Int) -> Unit {
  process_items_ffi(fn(f, x) { f(x) }, callback)
}
```

## Function pointer only

For callback API that accepts only a function pointer, use `FuncRef[(...) ->
ReturnType]` instead of closures.

**C side:**

```c
void (*signal(int sig, void (*func)(int)))(int);
```

**MoonBit side:**

```moonbit
pub extern "c" fn signal(
  signal : Int,
  callback : FuncRef[(Int) -> Unit]
) -> FuncRef[(Int) -> Unit] = "moonbit_signal"
```

Use `FuncRef` for plain function references; use the closure-as-callback pattern
when the callback needs captured state.
