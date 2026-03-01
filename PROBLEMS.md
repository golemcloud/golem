# Problems that could be solved in the MoonBit ecosystem

- The `mbti` parser is not up-to-date, had to parse `mbt` for reexports
- The need to generate reexports
- `formatter` package not up-to-date
- Could not find a package to parse/print the new `pkg` format 
- mbti files in the source tree are noise
- another `pre-build` hook that runs earlier so it can alter `pkg` files (needed for reexports link section generation)
- `post-build` hook
- having to build a cli tool for 'derivation'
- hard to change and regenerate bindings with wit-bindgen moonbit. many leftovers 
- wit-bindgen still generating pkg.json that moon fmt then converts, leaving behind the old ones
- we need moonbit async runtime for wasi pollable
- `moon build --target wasm` (debug, without `--release`) crashes the compiler with ICE: `Moonc.Basic_hash_string.Key_not_found` during `link-core`. **Workaround**: `NEW_MOON=0 moon build --target wasm` uses the legacy build graph and avoids the ICE. Requires adding missing transitive imports to `moon.pkg` files (e.g., `golem_sdk/agents/types/moon.pkg` needed explicit `golem/rpc/types` import).
    - https://github.com/moonbitlang/moonbit-docs/issues/1137
- **golem CLI infinite recursion on debug WASM**: `generateAgentWrapper` stack-overflows when parsing debug-mode WASM components (even with 64MB+ stack or `--all` stripping). The debug binary is 716K vs 348K release, and the unoptimized code structure triggers infinite recursion in the CLI's WASM parser. Cross-composing a release-generated wrapper with a debug binary doesn't work — agents fail with "Agent type not found" due to incompatible function indices. **Result**: debug builds with named backtraces are currently not deployable.
- **wit-bindgen MoonBit backend use-after-free bug**: Generated code calls `mbt_ffi_free` (which does `$moonbit.decref`) on intermediate buffers BEFORE the FFI import is called. When `mbt_ffi_malloc` for `return_area` reuses the freed memory, it corrupts data that the host import reads, causing "list pointer/length out of bounds of memory" crashes. Workaround: make `mbt_ffi_free` a no-op (`(func (param i32))`) in `interface/golem/agent/host/ffi.mbt` and `interface/golem/rpc/types/ffi.mbt`. This causes minor memory leaks of temporary FFI buffers but prevents the crash. Affects any host import that takes nested list types (e.g., `make-agent-id` with non-empty `DataValue`, `invoke-and-await` with non-empty params).
