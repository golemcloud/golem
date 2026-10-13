# Runtime provenance

The runtime is RustPython, built directly from
https://github.com/RustPython/RustPython at revision
`d79b41ba289844be0fe391bab5a083f6688af648`. Its standard library is frozen into
the component by RustPython's `freeze-stdlib` feature. The component does not
install a Python distribution in the owner filesystem.

`licenses/RustPython-LICENSE` is the unmodified runtime license.
`licenses/PSF-LICENSE` is the unmodified license of the frozen `Lib` sources.
`licenses/Golem-LICENSE` is the repository license for the tool implementation.
`licenses/THIRD_PARTY_LICENSES.txt` is generated from the locked WASM dependency
graph with cargo-about. Regenerate it when the lockfile changes:

```shell
cargo about generate about.hbs --target wasm32-wasip2 --locked \
  --output-file licenses/THIRD_PARTY_LICENSES.txt
```

The independent lockfile also pins RustPython's compiler/parser source and its
transitive dependencies. In particular, the pinned runtime needs compatible
libc, mac_address, mt19937 and malachite-bigint versions. Updating them requires
building and testing both native and WASM targets.

RustPython's Malachite dependencies are LGPL-3.0-only. The license inventory is
not a legal approval. Before publishing, review the distribution obligations
for the statically linked WASM, including corresponding source and relinking.
The source checkout, lockfile and manifest are the reproducible build inputs;
release provenance must identify the exact clean source checkout.

The Golem tool adapter and CLI dispatch are implemented here. The Python runner
in `golemcloud/golem-ai` was a reference for the runtime choice, not a dependency
or a source of the tool's sandbox or durability contract.
