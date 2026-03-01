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
- **wit-bindgen MoonBit backend use-after-free bug** (FIXED in https://github.com/vigoo/wit-bindgen/tree/moonbit-fixes-1, not yet upstream): Generated code was calling `mbt_ffi_free` on intermediate buffers before the FFI import was called, causing memory corruption. Fixed by reordering the generated code to defer frees until after the import call.
- having to explicitly `drop()` the RPC clients (because of having to explicitly drop the underlying WIT resource)
