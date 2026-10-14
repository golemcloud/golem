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

RustPython statically links `malachite-base`, `malachite-bigint`, `malachite-nz`,
and `malachite-q` 0.9.2, under LGPL-3.0-only. Their notices and both the LGPLv3
and GPLv3 texts are in `licenses/THIRD_PARTY_LICENSES.txt`. Each source retains
its own license. The Golem license applies to the adapter and the Golem schema
crates, not to the Malachite libraries. It does not restrict modification of the
library portions or reverse engineering to debug modifications to those portions.

## Source and relinking

The same release as `python.wasm` provides `python-source.tar.gz` and its SHA-256.
This archive contains the exact Golem source identified in `provenance.json`,
the locked Cargo dependencies, and Cargo's offline source configuration. The
RustPython `Lib` tree is materialized in full, including files omitted by
`cargo vendor` that the Git build freezes into the component.

To rebuild, install Rust 1.99.0 with the `wasm32-wasip2` target, a native C
toolchain, and wasm-tools 1.261.0. Extract the archive and run from `python-source`:

```shell
cargo build --offline --locked --release --target wasm32-wasip2 \
  --manifest-path builtin-tools/python/Cargo.toml --lib
wasm-tools metadata add --name golem:python --clear-version \
  builtin-tools/python/target/wasm32-wasip2/release/golem_python.wasm \
  -o python.wasm
wasm-tools validate --features all python.wasm
```

No Golem CLI build is needed for this route. The metadata command performs the
same final name/version step as `golem build`. The source archive provides the
inputs needed to rebuild and relink; build paths can affect the exact WASM bytes.

To use modified Malachite libraries, copy the required crates out of `vendor`
to a writable directory such as `modified-libraries`. Do not edit the
checksum-protected vendor files. Add local overrides to
`builtin-tools/python/Cargo.toml`, for example:

```toml
[patch.crates-io]
malachite-base = { path = "../../modified-libraries/malachite-base-0.9.2" }
malachite-bigint = { path = "../../modified-libraries/malachite-bigint-0.9.2" }
malachite-nz = { path = "../../modified-libraries/malachite-nz-0.9.2" }
malachite-q = { path = "../../modified-libraries/malachite-q-0.9.2" }
```

Modify the copied sources and repeat the build above without `--locked` to let
Cargo record the local overrides. The resulting component incorporates the
modified libraries. Retain the original lockfile to reproduce the baseline.

The Golem tool adapter and CLI dispatch are implemented here. The Python runner
in `golemcloud/golem-ai` was a reference for the runtime choice, not a dependency
or a source of the tool's sandbox or durability contract.
