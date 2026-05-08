# AddressSanitizer (ASan) Validation for MoonBit C Bindings

Reference for detecting memory bugs in C stub code using AddressSanitizer.

## Why ASan Matters

C bindings introduce manual memory management invisible to MoonBit's GC. These bugs
silently corrupt memory or leak resources. ASan catches them at runtime:

| Bug Class | Typical Cause in Bindings |
|---|---|
| Use-after-free | Accessing a C object after its finalizer ran |
| Double-free | Calling `moonbit_decref` on an already-released object |
| Memory leaks | Missing finalizer via `moonbit_make_external_object` |
| Buffer overflow | Wrong size passed to `moonbit_make_bytes` |
| Use-after-return | Returning a pointer to a local C variable |

## Quick Start

The skill includes `scripts/run-asan.py` which automates the full workflow.
It also works in CI environments.

Single package:

```bash
python3 scripts/run-asan.py --repo-root <project-root> --pkg moon.pkg
```

Multiple packages:

```bash
python3 scripts/run-asan.py \
  --repo-root <project-root> \
  --pkg moon.pkg \
  --pkg main/moon.pkg
```

The `--pkg` argument accepts both `moon.pkg` (DSL format) and `moon.pkg.json`
(JSON format). If the specified file doesn't exist, the script tries the other
format automatically.

Note for multiple packages, you need to include all packages with `native-stub`
and all entry packages. A package is an entry package if:

1. It is configured to be a main package, that is `is-main` set to true in
   `moon.pkg`/`moon.pkg.json`.
2. It contains tests.

---

## How It Works

The script combines two mechanisms. Understanding them is also useful for
manual setup or debugging.

### 1. Disable mimalloc

MoonBit bundles mimalloc as its allocator via `libmoonbitrun.o`. mimalloc
intercepts `malloc`/`free`, preventing ASan from tracking allocations. The
script replaces `libmoonbitrun.o` with an empty compiled object and restores
it afterward. Pass `--no-disable-mimalloc` to skip this step.

### 2. Package config patching

ASan flags must be injected into package config files. The script snapshots,
patches, and restores them in a `try/finally` block:

| Field | How it's patched | Why |
|---|---|---|
| `cc-flags` | Set to `-g -fsanitize=address -fno-omit-frame-pointer` | Instruments MoonBit-generated C code |
| `stub-cc-flags` | **Append** the same flags to existing value | Instruments C stub files (preserves `-I`, `-D` flags) |

Patch `stub-cc-flags` on all packages with `native-stub` (unconditionally safe to
patch all packages). Patch `cc-flags` on all entry packages (`is-main` or test).

### Environment variables

The script sets:

- `ASAN_OPTIONS="detect_leaks=<0|1>:fast_unwind_on_malloc=0"` — enables ASan
  and (where supported) LSan leak detection. `fast_unwind_on_malloc=0`
  produces more accurate stack traces.
- `LSAN_OPTIONS="suppressions=<path>"` — if a `.lsan-suppressions` file exists
  at the project root, it is passed to LSan (see Leak Suppressions below).

---

## Platform Setup

### macOS

**Homebrew LLVM** (preferred) — supports both ASan and LSan (leak detection).
The script probes `llvm`, `llvm@18`, `llvm@19`, `llvm@15`, `llvm@13`
automatically. Install with `brew install llvm`.

**System clang (Xcode 15+)** (fallback) — supports ASan but **not** LSan.
Leak detection is disabled (`detect_leaks=0`).

**Compiler override via `MOON_CC` + `MOON_AR`:** On macOS, the script sets
`MOON_CC` and `MOON_AR` to use Homebrew LLVM explicitly, because Apple Clang
does not support LeakSanitizer. This override is only needed on macOS — on
Linux, the system compiler supports ASan natively and when `cc-flags` is
present, tcc is disabled automatically.

Key constraints:

- `MOON_CC` accepts a compiler **path only** (e.g., `/opt/homebrew/opt/llvm/bin/clang`).
  Flags like `-fsanitize=address` cannot be included — moon treats the value
  as a single executable path.
- `MOON_AR` is **ignored** unless `MOON_CC` is also set.
- On macOS, moon derives `ar` from the compiler path. Homebrew LLVM has
  `llvm-ar` but not `ar`, so `MOON_AR=/usr/bin/ar` is needed.

### Linux

System `gcc` or `clang` on most distributions includes ASan out of the box.
On minimal images, install `libasan` (e.g., `apt-get install libasan6`).

### Windows

Use `cl.exe` with `/Z7 /fsanitize=address` for compilation. To disable
mimalloc manually:

```powershell
echo "" >dummy_libmoonbitrun.c
$moon_home = if ($env:MOON_HOME) { $env:MOON_HOME } else { "$env:USERPROFILE\.moon" }
$out_path = Convert-Path "$moon_home\lib\libmoonbitrun.o"
cl.exe dummy_libmoonbitrun.c /c /Fo: $out_path
```

---

## Leak Suppressions

macOS system libraries (libobjc, libdispatch, dyld) have known leaks that
trigger false positives. Place `.lsan-suppressions` at the project root:

```plaintext
leak:_libSystem_initializer
leak:_objc_init
leak:libdispatch
```

Each `leak:<pattern>` is matched against stack traces. If any frame matches,
the leak is suppressed. The script passes the absolute path to
`LSAN_OPTIONS` automatically.

Only suppress leaks from system/third-party code you do not control. Never
suppress leaks in your own C stub functions.

---

## Interpreting Results

### heap-use-after-free

Object was freed but still accessed. Check finalizer order and that `moonbit_decref`
is not called too early. Verify raw C pointers do not outlive the MoonBit wrapper.

### double-free

Same pointer freed twice. Ensure each C resource has exactly one owner. Check that
`moonbit_decref` is not called on already-released objects.

### heap-buffer-overflow

Writing past allocated buffer. Check `moonbit_make_bytes` and `moonbit_make_int32_array`
size calculations, especially byte-count vs element-count conversions.

### detected memory leaks

C allocations not freed. Verify every C allocation is wrapped with
`moonbit_make_external_object` so a finalizer is registered for cleanup.

### Fix Workflow

1. Read the ASan stack trace to find the first frame in your C stub code.
2. Identify which external object or buffer is involved.
3. Trace its lifetime: creation, `incref`/`decref` calls, finalizer registration.
4. Fix the root cause and re-run under ASan to confirm.
